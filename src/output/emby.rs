use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    OutputFile, OutputReport, PreparedExport, assets::image_bytes, nfo, settings::OutputOptions,
};
use crate::{
    LookupResponse,
    providers::media::emby::{Emby, episode_fields},
};

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PathMapping {
    server: String,
    local: String,
}

impl PathMapping {
    pub(super) fn validate(
        paths: &mut [Self],
        web: bool,
        root: Option<&Path>,
    ) -> Result<(), String> {
        if paths.len() > 32 {
            return Err("最多可配置 32 条 Emby 媒体路径映射。".into());
        }
        if web && root.is_none() && !paths.is_empty() {
            return Err("请先为 Web 服务配置可写的 EMBY_WRITEBACK_ROOT。".into());
        }
        let mut servers = HashSet::new();
        for path in paths {
            path.server = path.server.trim().replace('\\', "/");
            path.local = path.local.trim().to_owned();
            let Some(server) = ServerPath::parse(&path.server) else {
                return Err("Emby 端路径必须是绝对路径，例如 /media/Movies。".into());
            };
            let local = Path::new(&path.local);
            if path.local.len() > 4096
                || path.local.contains('\0')
                || local.components().any(|part| part == Component::ParentDir)
                || (web
                    && local
                        .components()
                        .any(|part| !matches!(part, Component::Normal(_))))
                || (!web && !local.is_absolute())
            {
                return Err(if web {
                    "请输入可写挂载根目录下的相对路径。"
                } else {
                    "请输入本机可访问的媒体目录绝对路径。"
                }
                .into());
            }
            let mut key = format!("{}:{}", server.root, server.parts.join("/"));
            if server.windows() {
                key.make_ascii_lowercase();
            }
            if !servers.insert(key) {
                return Err("每个 Emby 端目录只能配置一条路径映射。".into());
            }
        }
        Ok(())
    }
}

struct ServerPath<'a> {
    root: &'a str,
    parts: Vec<&'a str>,
}

impl<'a> ServerPath<'a> {
    fn parse(path: &'a str) -> Option<Self> {
        if path.len() > 4096 || path.contains('\0') {
            return None;
        }
        let (root, remainder) = if let Some(remainder) = path.strip_prefix("//") {
            ("//", remainder)
        } else if let Some(remainder) = path.strip_prefix('/') {
            ("/", remainder)
        } else if matches!(path.as_bytes(), [b'A'..=b'Z' | b'a'..=b'z', b':', b'/', ..]) {
            (&path[..2], &path[3..])
        } else {
            return None;
        };
        let parts: Vec<_> = remainder
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.iter().any(|part| matches!(*part, "." | "..")) || (root == "//" && parts.len() < 2)
        {
            return None;
        }
        Some(Self { root, parts })
    }

    fn windows(&self) -> bool {
        self.root != "/"
    }

    fn relative_to(&self, base: &Self) -> Option<&[&str]> {
        let equal = |left: &str, right: &str| {
            if self.windows() {
                left.eq_ignore_ascii_case(right)
            } else {
                left == right
            }
        };
        (equal(self.root, base.root)
            && self.parts.len() >= base.parts.len()
            && self
                .parts
                .iter()
                .zip(&base.parts)
                .all(|(left, right)| equal(left, right)))
        .then(|| &self.parts[base.parts.len()..])
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct MediaPaths {
    paths: Vec<PathMapping>,
    web_root: Option<PathBuf>,
}

impl MediaPaths {
    pub(super) fn new(paths: Vec<PathMapping>, web: bool, web_root: Option<PathBuf>) -> Self {
        Self {
            paths: if web && web_root.is_none() {
                Vec::new()
            } else {
                paths
            },
            web_root,
        }
    }

    fn target(&self, path: &str, series: bool) -> Option<Target> {
        let normalized = path.replace('\\', "/");
        let server = ServerPath::parse(&normalized)?;
        let (mapping, tail) = self
            .paths
            .iter()
            .filter_map(|mapping| {
                let base = ServerPath::parse(&mapping.server)?;
                Some((mapping, server.relative_to(&base)?))
            })
            .min_by_key(|(_, tail)| tail.len())?;
        let mut relative = PathBuf::new();
        let root = match &self.web_root {
            Some(root) => {
                let local = Path::new(&mapping.local);
                if local
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return None;
                }
                relative.push(local);
                root.clone()
            }
            None => {
                let local = PathBuf::from(&mapping.local);
                if !local.is_absolute() {
                    return None;
                }
                local
            }
        };
        for part in tail {
            let mut components = Path::new(part).components();
            if !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
            {
                return None;
            }
            relative.push(part);
        }
        let video = if series {
            None
        } else {
            if tail.is_empty() {
                return None;
            }
            let name = relative.file_name()?;
            Path::new(name).file_stem()?;
            Some(name.to_owned())
        };
        if video.is_some() {
            relative.pop();
        }
        Some(Target {
            root,
            relative,
            video,
        })
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct Target {
    root: PathBuf,
    relative: PathBuf,
    video: Option<OsString>,
}

impl Target {
    fn directory(&self) -> io::Result<Dir> {
        let root = Dir::open_ambient_dir(&self.root, ambient_authority())?;
        let dir = if self.relative.as_os_str().is_empty() {
            root
        } else {
            root.open_dir(&self.relative)?
        };
        if let Some(video) = &self.video
            && !dir.symlink_metadata(video)?.is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular video file",
            ));
        }
        Ok(dir)
    }

    fn display_path(&self) -> String {
        self.root
            .join(&self.relative)
            .to_string_lossy()
            .into_owned()
    }

    fn stem(&self) -> Option<&str> {
        Path::new(self.video.as_ref()?).file_stem()?.to_str()
    }

    fn video_sidecar(&self, suffix: &str) -> String {
        format!(
            "{}{suffix}",
            self.stem().expect("mapped Emby video has a file stem")
        )
    }

    fn backdrop_stem(&self) -> String {
        let Some(stem) = self.stem() else {
            return "fanart".into();
        };
        // A folder named for the video (including Emby's version suffix) can use shared fanart.
        if self
            .relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|folder| {
                stem == folder
                    || stem
                        .strip_prefix(folder)
                        .is_some_and(|suffix| suffix.starts_with(" - "))
            })
        {
            "fanart".into()
        } else {
            self.video_sidecar("-thumb")
        }
    }
}

struct StagedFile {
    name: String,
    path: PathBuf,
}

impl StagedFile {
    async fn new(stage: &Path, name: String, bytes: impl AsRef<[u8]>) -> io::Result<Self> {
        let path = stage.join(uuid::Uuid::new_v4().simple().to_string());
        tokio::fs::write(&path, bytes).await?;
        Ok(Self { name, path })
    }

    fn publish(&self, dir: &Dir, video: Option<&OsString>) -> io::Result<()> {
        if video.is_some_and(|video| video.eq_ignore_ascii_case(&self.name)) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        match dir.symlink_metadata(&self.name) {
            Ok(entry) if !entry.is_file() => return Err(io::ErrorKind::AlreadyExists.into()),
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let temporary = format!(".collect-metadata-{}.tmp", uuid::Uuid::new_v4().simple());
        let mut output =
            dir.open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        let result = (|| {
            let mut input = fs::File::open(&self.path)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            drop(output);
            dir.rename(&temporary, dir, &self.name)
        })();
        if result.is_err() {
            let _ = dir.remove_file(&temporary);
        }
        result
    }
}

pub(crate) struct MediaExport {
    _stage: tempfile::TempDir,
    entries: Vec<(Target, Vec<StagedFile>)>,
    report: OutputReport,
}

impl MediaExport {
    pub(super) fn publish(mut self) -> OutputReport {
        for (target, files) in self.entries {
            let Ok(dir) = target.directory() else {
                self.report.failed += files.len();
                continue;
            };
            for file in files {
                if file.publish(&dir, target.video.as_ref()).is_ok() {
                    self.report.written += 1;
                } else {
                    self.report.failed += 1;
                }
            }
        }
        let failed = self.report.failed;
        if failed > 0 {
            let details = self.report.message.take().unwrap_or_default();
            self.report.message = Some(format!(
                "{failed} 项 Emby 资料未能写回。请检查媒体路径映射、挂载状态和写入权限。{details}"
            ));
        }
        self.report
    }
}

pub(crate) async fn prepare(
    client: &Client,
    emby: &Emby,
    paths: &MediaPaths,
    data: &Value,
    result: &LookupResponse,
) -> Result<PreparedExport, OutputReport> {
    if paths.paths.is_empty() {
        return Err(OutputReport::error(
            "请在「设置 → 输出与命名」配置 Emby 媒体路径映射。",
        ));
    }
    let item = &data["item"];
    let series = item["Type"] == "Series";
    let target = item["Path"]
        .as_str()
        .and_then(|path| paths.target(path, series))
        .ok_or_else(|| OutputReport::error("Emby 条目的媒体路径未能匹配已配置的写回目录。"))?;
    target.directory().map_err(|_| {
        OutputReport::error("无法访问对应的 Emby 媒体目录或视频，请检查路径映射和挂载状态。")
    })?;
    let stage =
        tempfile::tempdir().map_err(|_| OutputReport::error("无法创建 Emby 写回的临时目录。"))?;
    let directory = target.display_path();
    let episodes: HashMap<_, _> = result.record["fields"]["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|episode| Some((episode["id"].as_str()?, episode)))
        .collect();
    let mut targets = HashSet::new();
    let mut child_entries = Vec::new();
    let mut missing = 0;
    if series {
        for child in data["children"].as_array().into_iter().flatten() {
            if child["Type"] != "Episode" || child["LocationType"] == "Virtual" {
                continue;
            }
            let Some(path) = child["Path"].as_str().filter(|path| !path.is_empty()) else {
                continue;
            };
            let Some(target) = paths
                .target(path, false)
                .filter(|target| target.directory().is_ok())
            else {
                missing += 1;
                continue;
            };
            let Some(episode) = child["Id"]
                .as_str()
                .and_then(|id| episodes.get(id).copied())
            else {
                missing += 1;
                continue;
            };
            if targets.insert(target.clone()) {
                child_entries.push((target, child, Some(episode)));
            }
        }
    }
    let mut pending = std::iter::once((target, item, None)).chain(child_entries);
    let mut jobs = FuturesUnordered::new();
    let mut entries = Vec::new();
    let mut failed = missing;
    loop {
        for (target, item, episode) in pending.by_ref().take(4 - jobs.len()) {
            jobs.push(prepare_entry(
                client,
                emby,
                stage.path(),
                target,
                item,
                episode,
                &result.record,
            ));
        }
        let Some((target, files, failures)) = jobs.next().await else {
            break;
        };
        entries.push((target, files));
        failed += failures;
    }
    drop(jobs);
    Ok(PreparedExport::Emby(MediaExport {
        _stage: stage,
        entries,
        report: OutputReport {
            directory: Some(directory),
            written: 0,
            failed,
            message: (missing > 0).then(|| format!("其中 {missing} 个分集的媒体路径不可用。")),
        },
    }))
}

async fn prepare_entry(
    client: &Client,
    emby: &Emby,
    stage: &Path,
    target: Target,
    item: &Value,
    episode: Option<&Value>,
    record: &Value,
) -> (Target, Vec<StagedFile>, usize) {
    let mut files = Vec::new();
    let mut images = Vec::new();
    let mut failed = 0;
    let is_episode = item["Type"] == "Episode";
    let primary = if is_episode {
        target.video_sidecar("-thumb")
    } else if target.video.is_some() {
        target.video_sidecar("-poster")
    } else {
        "poster".into()
    };
    for (available, image_type, kind, stem) in [
        (
            item["ImageTags"]["Primary"].is_string(),
            "Primary",
            if is_episode { "still" } else { "poster" },
            primary,
        ),
        (
            !is_episode
                && item["BackdropImageTags"]
                    .as_array()
                    .is_some_and(|tags| !tags.is_empty()),
            "Backdrop",
            "backdrop",
            target.backdrop_stem(),
        ),
    ] {
        if !available {
            continue;
        }
        let download = async {
            let mut url = emby.image_url(&item["Id"], image_type)?;
            url.query_pairs_mut().append_pair("format", "jpg");
            let response = emby.image_response(client, &url).await?;
            let (bytes, extension) = image_bytes(response).await?;
            StagedFile::new(stage, format!("{stem}.{extension}"), bytes)
                .await
                .ok()
        };
        if let Some(file) = download.await {
            images.push(OutputFile {
                path: file.name.clone(),
                kind,
                source_url: None,
                person: None,
            });
            files.push(file);
        } else {
            failed += 1;
        }
    }
    let name = if target.video.is_some() {
        target.video_sidecar(".nfo")
    } else {
        "tvshow.nfo".into()
    };
    let episode_record = episode.map(|localized| {
        let mut fields = episode_fields(item);
        for key in ["title", "description", "description_original"] {
            if let Some(text) = localized.get(key).filter(|text| text.is_string()) {
                fields.insert(key, text.clone());
            }
        }
        if !fields.contains_key("series") {
            fields.insert("series", record["fields"]["title"].clone());
        }
        json!({"scope": "episode", "media_type": "series", "fields": fields})
    });
    let xml = nfo::work_xml(
        episode_record.as_ref().unwrap_or(record),
        &OutputOptions::default(),
        &images,
    );
    match StagedFile::new(stage, name, xml).await {
        Ok(file) => files.push(file),
        Err(_) => failed += 1,
    }
    (target, files, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_the_most_specific_server_path_by_component() {
        let root = tempfile::tempdir().unwrap();
        let mut mappings = vec![
            PathMapping {
                server: r"Z:\Media".into(),
                local: "general".into(),
            },
            PathMapping {
                server: r"Z:\Media\TV".into(),
                local: "series".into(),
            },
        ];
        PathMapping::validate(&mut mappings, true, Some(root.path())).unwrap();
        let paths = MediaPaths::new(mappings, true, Some(root.path().into()));

        let target = paths
            .target(r"z:\media\tv\剧集\Season 1\第 1 集.mp4", false)
            .unwrap();
        assert_eq!(target.relative, Path::new("series/剧集/Season 1"));
        assert_eq!(target.video_sidecar(".nfo"), "第 1 集.nfo");
        assert_eq!(
            paths
                .target(r"Z:\Media\TV Archive\Film.mkv", false)
                .unwrap()
                .relative,
            Path::new("general/TV Archive")
        );
        for path in [
            r"Z:\MediaArchive\Film.mkv",
            r"Z:\Media\..\Film.mp4",
            "https://media/show",
        ] {
            assert!(paths.target(path, false).is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn confines_sidecars_to_the_mapped_directory() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("media");
        let outside = workspace.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("Film.mkv"), "video").unwrap();
        fs::write(outside.join("Film.nfo"), "untouched").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(outside.join("Film.nfo"), root.join("Film.nfo")).unwrap();
        fs::write(root.join("Film.mkv"), "another video").unwrap();

        let mappings = vec![PathMapping {
            server: "/media".into(),
            local: String::new(),
        }];
        let paths = MediaPaths::new(mappings, true, Some(root));
        assert!(
            paths
                .target("/media/escape/Film.mkv", false)
                .unwrap()
                .directory()
                .is_err()
        );
        let target = paths.target("/media/Film.mkv", false).unwrap();
        let staged = StagedFile::new(workspace.path(), "Film.nfo".into(), "new")
            .await
            .unwrap();
        assert!(
            staged
                .publish(&target.directory().unwrap(), target.video.as_ref())
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.join("Film.nfo")).unwrap(),
            "untouched"
        );
    }
}
