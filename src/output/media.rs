use std::{collections::HashSet, path::Path, time::Duration};

use futures_util::{StreamExt, stream};
use reqwest::{Client, Response, Url, header};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::{OutputFile, assets, settings::OutputOptions, write_json};
use crate::providers::media::emby::{Emby, list};

const FILE_LIMIT: u64 = 256 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Trailer,
    ThemeVideo,
    ThemeMusic,
    Extra,
}

impl Kind {
    fn enabled(self, options: &OutputOptions) -> bool {
        match self {
            Self::Trailer => options.trailers,
            Self::ThemeVideo => options.theme_videos,
            Self::ThemeMusic => options.theme_music,
            Self::Extra => options.extras,
        }
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Trailer => "trailers",
            Self::ThemeVideo => "backdrops",
            Self::ThemeMusic => "theme-music",
            Self::Extra => "extras",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Trailer => "trailer",
            Self::ThemeVideo => "theme_video",
            Self::ThemeMusic => "theme_music",
            Self::Extra => "extra",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Trailer => "预告片",
            Self::ThemeVideo => "主题视频",
            Self::ThemeMusic => "主题音乐",
            Self::Extra => "花絮",
        }
    }

    fn extension(self, path: &str) -> Option<&'static str> {
        self.container(path.rsplit_once('.')?.1)
    }

    fn container(self, ext: &str) -> Option<&'static str> {
        let accepted: &[&str] = if self == Self::ThemeMusic {
            &["mp3", "flac", "wav", "ogg", "opus", "m4a", "aac"]
        } else {
            &["mp4", "m4v", "mov", "mkv", "webm", "avi"]
        };
        accepted
            .iter()
            .copied()
            .find(|value| value.eq_ignore_ascii_case(ext))
    }
}

#[derive(Serialize)]
pub(super) struct Link {
    pub(super) kind: Kind,
    pub(super) url: String,
    title: Option<String>,
    provider: Option<String>,
}

pub(super) fn web_url(value: &Value) -> Option<&str> {
    let text = value.as_str()?.trim();
    let url = Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(text)
}

pub(super) fn links(record: &Value) -> Vec<Link> {
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    let fields = &record["fields"];
    let sources =
        list(&record["sources"]).map(|source| (&source["fields"], source["provider"].as_str()));
    for (fields, provider) in
        std::iter::once((fields, record["field_sources"]["videos"].as_str())).chain(sources)
    {
        let mut add = |kind, url: &str, title: Option<&str>, provider: Option<&str>| {
            if seen.insert((kind, url.to_owned())) {
                links.push(Link {
                    kind,
                    url: url.to_owned(),
                    title: title.map(str::to_owned),
                    provider: provider.map(str::to_owned),
                });
            }
        };
        for video in list(&fields["videos"]) {
            let Some(url) = web_url(&video["url"]) else {
                continue;
            };
            let kind = match video["type"].as_str() {
                Some("Trailer" | "Teaser") => Kind::Trailer,
                Some("Theme Video") => Kind::ThemeVideo,
                Some("Theme Music") => Kind::ThemeMusic,
                _ => Kind::Extra,
            };
            add(kind, url, video["name"].as_str(), provider);
        }
        for (kind, key) in [
            (Kind::Trailer, "trailer_url"),
            (Kind::ThemeVideo, "theme_video_url"),
            (Kind::ThemeMusic, "theme_music_url"),
            (Kind::Extra, "extra_url"),
        ] {
            if let Some(url) = web_url(&fields[key]) {
                add(
                    kind,
                    url,
                    None,
                    provider.or_else(|| record["field_sources"][key].as_str()),
                );
            }
        }
    }
    links
}

struct Download {
    url: Url,
    extension: &'static str,
    from_emby: bool,
}

pub(super) async fn export(
    client: &Client,
    emby: Option<&Emby>,
    directory: &Path,
    record: &Value,
    options: &OutputOptions,
) -> (Vec<OutputFile>, usize, Vec<String>) {
    let kinds = [
        Kind::Trailer,
        Kind::ThemeVideo,
        Kind::ThemeMusic,
        Kind::Extra,
    ];
    if !kinds.iter().any(|kind| kind.enabled(options)) {
        return (Vec::new(), 0, Vec::new());
    }
    let links = links(record)
        .into_iter()
        .filter(|link| link.kind.enabled(options))
        .collect::<Vec<_>>();
    let emby_work = emby.and_then(|emby| {
        list(&record["sources"])
            .find(|source| source["provider"] == "Emby" && emby.owns_source(source))
            .map(|source| (emby, &source["data"]["item"]))
    });
    let mut files = Vec::new();
    let mut failures = 0;
    let mut messages = Vec::new();
    let (trailers, extras) = emby_work.map_or((false, false), |(_, item)| {
        (
            options.trailers && item["LocalTrailerCount"].as_u64().unwrap_or_default() > 0,
            options.extras && item["SpecialFeatureCount"].as_u64().unwrap_or_default() > 0,
        )
    });
    let user = match emby_work {
        Some((emby, _)) if trailers || extras => Some(emby.extras_user(client).await),
        _ => None,
    };
    for kind in kinds.into_iter().filter(|kind| kind.enabled(options)) {
        let num_links = links.iter().filter(|link| link.kind == kind).count();
        let mut downloads = Vec::new();
        if let Some((emby, item)) = emby_work {
            let id = &item["Id"];
            let entries = match kind {
                Kind::ThemeVideo | Kind::ThemeMusic => {
                    emby.theme_media(client, id, kind == Kind::ThemeMusic).await
                }
                Kind::Trailer | Kind::Extra
                    if (kind == Kind::Trailer && trailers) || (kind == Kind::Extra && extras) =>
                {
                    match user.as_ref() {
                        Some(Ok(user)) => {
                            emby.local_extras(client, id, user, kind == Kind::Trailer)
                                .await
                        }
                        Some(Err(error)) => Err(error.clone()),
                        None => Ok(Vec::new()),
                    }
                }
                _ => Ok(Vec::new()),
            };
            match entries {
                Ok(items) => downloads.extend(
                    items
                        .iter()
                        .filter(|entry| entry["Id"] != *id && entry["LocationType"] != "Remote")
                        .filter_map(|entry| {
                            let location = [
                                &entry["Path"],
                                &entry["FileName"],
                                &entry["MediaSources"][0]["Path"],
                            ];
                            let extension = location
                                .into_iter()
                                .filter_map(Value::as_str)
                                .find_map(|path| kind.extension(path))
                                .or_else(|| {
                                    [&entry["Container"], &entry["MediaSources"][0]["Container"]]
                                        .into_iter()
                                        .filter_map(Value::as_str)
                                        .find_map(|container| kind.container(container))
                                })?;
                            Some(Download {
                                url: emby.media_file_url(&entry["Id"])?,
                                extension,
                                from_emby: true,
                            })
                        }),
                ),
                Err(error) => messages.push(format!("读取 Emby {}失败。{error}", kind.label())),
            }
        }
        downloads.extend(
            links
                .iter()
                .filter(|link| link.kind == kind)
                .filter_map(|link| {
                    let url = Url::parse(&link.url).ok().filter(assets::allowed)?;
                    let extension = kind.extension(url.path())?;
                    Some(Download {
                        url,
                        extension,
                        from_emby: false,
                    })
                }),
        );
        let mut seen = HashSet::new();
        let selected = downloads
            .into_iter()
            .filter(|media| seen.insert(media.url.clone()))
            .take(10);
        let first = files.len();
        let mut pending = stream::iter(selected.enumerate())
            .map(|(index, media)| async move {
                let response = if media.from_emby {
                    emby?
                        .media_response(client, &media.url, DOWNLOAD_TIMEOUT)
                        .await?
                } else {
                    assets::public_response(
                        client,
                        &media.url,
                        "video/*, audio/*, application/octet-stream",
                        DOWNLOAD_TIMEOUT,
                    )
                    .await?
                };
                let path = format!(
                    "{}/{}-{:02}.{}",
                    kind.directory(),
                    kind.name(),
                    index + 1,
                    media.extension
                );
                save_media(response, directory, &path, media.extension).await?;
                Some(OutputFile {
                    path,
                    kind: kind.name(),
                    source_url: Some(media.url.into()),
                    person: None,
                })
            })
            .buffer_unordered(2);
        while let Some(item) = pending.next().await {
            match item {
                Some(file) => files.push(file),
                None => failures += 1,
            }
        }
        if files.len() == first {
            messages.push(if num_links > 0 {
                format!("{}仅取得链接。", kind.label())
            } else {
                format!("未取得{}文件。", kind.label())
            });
        }
    }
    if !links.is_empty() {
        let path = "media-links.json";
        if write_json(&directory.join(path), &json!(links)).is_ok() {
            files.push(OutputFile {
                path: path.into(),
                kind: "media_links",
                source_url: None,
                person: None,
            });
        } else {
            failures += 1;
        }
    }
    (files, failures, messages)
}

async fn save_media(
    mut response: Response,
    directory: &Path,
    path: &str,
    extension: &str,
) -> Option<()> {
    let length = response.content_length();
    if response.status() != reqwest::StatusCode::OK
        || length.is_some_and(|length| length > FILE_LIMIT)
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("text/") || value.starts_with("application/json")
            })
    {
        return None;
    }
    let destination = directory.join(path);
    let temporary = tempfile::NamedTempFile::new_in(directory).ok()?;
    let mut file = tokio::fs::File::from_std(temporary.reopen().ok()?);
    let mut size = 0;
    let mut signature = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        size += chunk.len() as u64;
        if size > FILE_LIMIT {
            return None;
        }
        if signature.len() < 12 {
            signature.extend_from_slice(&chunk[..chunk.len().min(12 - signature.len())]);
            if signature.len() == 12 && !has_signature(&signature, extension) {
                return None;
            }
        }
        file.write_all(&chunk).await.ok()?;
    }
    if signature.len() < 12 || length.is_some_and(|length| length != size) {
        return None;
    }
    file.flush().await.ok()?;
    drop(file);
    super::settings::make_child_dir(destination.parent()?).ok()?;
    temporary.persist_noclobber(destination).ok()?;
    Some(())
}

fn has_signature(bytes: &[u8], extension: &str) -> bool {
    match extension {
        "mp4" | "m4v" | "m4a" => &bytes[4..8] == b"ftyp",
        "mov" => matches!(&bytes[4..8], b"ftyp" | b"moov" | b"wide" | b"mdat"),
        "mkv" | "webm" => bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "avi" => bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI ",
        "wav" => bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE",
        "mp3" | "aac" => bytes.starts_with(b"ID3") || bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0,
        "flac" => bytes.starts_with(b"fLaC"),
        "ogg" | "opus" => bytes.starts_with(b"OggS"),
        _ => false,
    }
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use std::fs;

    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
        routing::get,
    };

    use super::*;
    use crate::{
        EmbySettings, LookupResponse, MediaKind,
        output::{ExportPlan, prepare},
    };

    const MP4: &[u8] = b"\x00\x00\x00\x18ftypisom\x00\x00\x00\x01isomiso2";
    const MP3: &[u8] = b"ID3\x03\x00\x00\x00\x00\x00\x00\xff\xfb\xa0";

    #[tokio::test]
    async fn exports_emby_files_and_links_without_leaving_failed_downloads() {
        let mock = Router::new().fallback(get(|headers: HeaderMap, request: Uri| async move {
            assert_eq!(headers["x-emby-token"], "mock-key");
            assert!(!request.to_string().contains("mock-key"));
            let query = request.query().unwrap_or_default();
            match request.path() {
                "/emby/Items" => Json(json!({"Items":[{
                    "Id":"1", "Type":"Movie", "Name":"Film", "Path":"/library/Film.mp4",
                    "LocalTrailerCount":1,"SpecialFeatureCount":1,
                    "MediaStreams":[{"Type":"Video","Width":1920,"Height":1080},{"Type":"Subtitle","Language":"zh-CN"}],
                    "RemoteTrailers":[{"Url":"https://www.youtube.com/watch?v=trailer","Name":"Official"}]
                }]})).into_response(),
                "/emby/Users/Query" => Json(json!({"Items":[{"Id":"admin","Policy":{"IsAdministrator":true}}]})).into_response(),
                "/emby/Users/admin/Items/1/LocalTrailers" => Json(json!([{"Id":"2","Path":"/trailers/preview.MP4"}])).into_response(),
                "/emby/Users/admin/Items/1/SpecialFeatures" => Json(json!([{"Id":"5","Path":"/extras/behind.mp4"}])).into_response(),
                "/emby/Items/1/ThemeVideos" => {
                    assert!(query.contains("InheritFromParent=false"));
                    Json(json!({"Items":[{"Id":"3","Path":"/backdrops/intro.mp4"}]})).into_response()
                }
                "/emby/Items/1/ThemeSongs" => Json(json!({"Items":[{"Id":"4","Container":"mp3"}]})).into_response(),
                "/emby/Items/2/File" | "/emby/Items/3/File" => ([(header::CONTENT_TYPE, "video/mp4")], MP4).into_response(),
                "/emby/Items/4/File" => ([(header::CONTENT_TYPE, "audio/mpeg")], MP3).into_response(),
                "/emby/Items/5/File" => ([(header::CONTENT_TYPE, "application/octet-stream")], b"<html>oops</html>".as_slice()).into_response(),
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let serving = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        let settings_file = directory.path().join("emby.json");
        write_json(
            &settings_file,
            &json!({"server":server,"api_key":"mock-key","enabled":true}),
        )
        .unwrap();
        let config = EmbySettings::load(settings_file).unwrap();
        let emby = config.active().unwrap();
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let (_, source) = emby
            .find(&client, MediaKind::Movie, "emby:1", None)
            .await
            .unwrap();
        let lookup = LookupResponse {
            record: json!({"query":"Film","media_type":"movie","fields":source.fields,"sources":[source]}),
            saved: None,
            storage_error: None,
        };
        let plan = ExportPlan {
            directory: Some(directory.path().canonicalize().unwrap()),
            owner: "test".into(),
            options: serde_json::from_value(json!({
                "metadata":false,"poster":false,"emby_nfo":true,
                "trailers":true,"theme_videos":true,"theme_music":true,"extras":true
            }))
            .unwrap(),
        };
        let prepared = prepare(&client, Some(&emby), &plan, 1, 2, &lookup, &[])
            .await
            .unwrap_or_else(|_| panic!("prepare"));
        let work = plan.directory.as_ref().unwrap().join("batch-1/work-2");
        assert!(!work.exists());
        let report = prepared.publish();
        assert_eq!(report.written, 5);
        assert_eq!(report.failed, 1);
        assert_eq!(fs::read(work.join("trailers/trailer-01.mp4")).unwrap(), MP4);
        assert_eq!(
            fs::read(work.join("backdrops/theme_video-01.mp4")).unwrap(),
            MP4
        );
        assert_eq!(
            fs::read(work.join("theme-music/theme_music-01.mp3")).unwrap(),
            MP3
        );
        assert!(!work.join("extras").exists());
        let manifest = fs::read_to_string(work.join("files.json")).unwrap();
        assert!(manifest.contains("未取得花絮文件"));
        assert!(!manifest.contains("mock-key"));
        let nfo = fs::read_to_string(work.join("Film.nfo")).unwrap();
        assert!(nfo.contains("<trailer>https://www.youtube.com/watch?v=trailer</trailer>"));
        assert!(nfo.contains("<tag>1920×1080</tag>"));
        assert!(nfo.contains("<tag>中文字幕</tag>"));
        assert_eq!(fs::read_dir(&work).unwrap().count(), 6);
        serving.abort();
    }

    #[tokio::test]
    async fn keeps_link_only_and_missing_media_visible_in_export_feedback() {
        let record = json!({"fields":{"trailer_url":"https://www.youtube.com/watch?v=abc",
            "videos":[{"type":"Trailer","url":"https://www.youtube.com/watch?v=abc"},
                      {"type":"Behind the Scenes","url":"https://vimeo.com/123"}]}});
        let directory = tempfile::tempdir().unwrap();
        let options = serde_json::from_value(json!({"trailers":true,"theme_videos":true})).unwrap();
        let (files, failed, messages) =
            export(&Client::new(), None, directory.path(), &record, &options).await;
        assert_eq!(files.len(), 1);
        assert_eq!(failed, 0);
        assert_eq!(messages, ["预告片仅取得链接。", "未取得主题视频文件。"]);
        let links: Value = serde_json::from_reader(
            fs::File::open(directory.path().join("media-links.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(links.as_array().unwrap().len(), 1);
        assert_eq!(links[0]["kind"], "trailer");
    }
}
