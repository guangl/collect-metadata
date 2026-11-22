mod paths;
mod publish;

pub(crate) use paths::MediaPaths;
pub(super) use paths::PathMapping;
#[cfg(feature = "web")]
pub(crate) use publish::write_automatic;
pub(crate) use publish::{PreparedWriteback, prepare};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    time::UNIX_EPOCH,
};

use cap_std::fs::Dir;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::assets::allowed;

use paths::Target;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "png", "webp", "gif"];
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FileKind {
    Nfo,
    Poster,
    Backdrop,
    Logo,
    Landscape,
    Still,
}

impl FileKind {
    fn label(self) -> &'static str {
        match self {
            Self::Nfo => "nfo",
            Self::Poster => "poster",
            Self::Backdrop => "backdrop",
            Self::Logo => "logo",
            Self::Landscape => "landscape",
            Self::Still => "still",
        }
    }

    fn image_type(self) -> Option<&'static str> {
        match self {
            Self::Nfo => None,
            Self::Poster | Self::Still => Some("Primary"),
            Self::Backdrop => Some("Backdrop"),
            Self::Logo => Some("Logo"),
            Self::Landscape => Some("Thumb"),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
enum Existing {
    Missing,
    File { length: u64, modified: i128 },
    Blocked,
}

impl Existing {
    fn read(dir: &Dir, name: &str) -> Self {
        match dir.symlink_metadata(name) {
            Ok(metadata) if metadata.is_file() => {
                metadata
                    .modified()
                    .map_or(Self::Blocked, |modified| Self::File {
                        length: metadata.len(),
                        modified: match modified.into_std().duration_since(UNIX_EPOCH) {
                            Ok(elapsed) => elapsed.as_nanos() as i128,
                            Err(error) => -(error.duration().as_nanos() as i128),
                        },
                    })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::Missing,
            _ => Self::Blocked,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Destination {
    name: String,
    existing: Existing,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct PlannedFile {
    key: String,
    source: String,
    item_id: Value,
    kind: FileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_image: Option<String>,
    target: Option<Target>,
    destinations: Vec<Destination>,
    blocked: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    create_only: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WritebackJob {
    pub(crate) id: String,
    pub(crate) owner: String,
    pub(crate) stage: String,
    pub(crate) files: Vec<PlannedFile>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct WritebackState {
    pub(crate) remaining: Option<usize>,
    files: BTreeMap<String, FileResult>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum FileResult {
    Written {
        path: String,
        backup: Option<String>,
    },
    Failed {
        error: String,
    },
}

#[derive(Serialize)]
pub struct WritebackPreviewFile {
    source: String,
    kind: &'static str,
    destinations: Vec<PreviewDestination>,
    path: Option<String>,
    backup: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct PreviewDestination {
    path: String,
    action: &'static str,
}

impl PlannedFile {
    fn preview(&self, state: &WritebackState) -> WritebackPreviewFile {
        let mut file = WritebackPreviewFile {
            source: self.source.clone(),
            kind: self.kind.label(),
            destinations: Vec::new(),
            path: None,
            backup: None,
            error: self.blocked.clone(),
        };
        match state.files.get(&self.key) {
            Some(FileResult::Written { path, backup }) => {
                file.path = Some(path.clone());
                file.backup = backup.clone();
                return file;
            }
            Some(FileResult::Failed { error }) if file.error.is_none() => {
                file.error = Some(error.clone());
            }
            _ => {}
        }
        if let Some(target) = &self.target {
            file.destinations = self
                .destinations
                .iter()
                .map(|destination| PreviewDestination {
                    path: target.path(&destination.name),
                    action: match &destination.existing {
                        Existing::Missing => "create",
                        Existing::File { .. } => "replace",
                        Existing::Blocked => "blocked",
                    },
                })
                .collect();
        }
        file
    }
}

pub(crate) fn plan(
    paths: &MediaPaths,
    data: &Value,
    record: &Value,
    state: &WritebackState,
) -> (Vec<PlannedFile>, Vec<WritebackPreviewFile>, usize) {
    plan_with_mode(paths, data, record, state, false)
}

fn plan_with_mode(
    paths: &MediaPaths,
    data: &Value,
    record: &Value,
    state: &WritebackState,
    create_only: bool,
) -> (Vec<PlannedFile>, Vec<WritebackPreviewFile>, usize) {
    let main_item = &data["item"];
    let series = main_item["Type"] == "Series";
    let children = data["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|child| {
            !create_only
                && series
                && child["Type"] == "Episode"
                && child["LocationType"] != "Virtual"
        });
    let fields = &record["fields"];
    let episodes: HashMap<_, _> = fields["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|episode| Some((episode["id"].as_str()?, episode)))
        .collect();
    let fill_missing = record["fill_missing"] == true;
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut preview = Vec::new();
    let mut writable = 0;
    for item in std::iter::once(main_item).chain(children) {
        let path = item["Path"].as_str().filter(|path| !path.is_empty());
        let source = path.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "Emby #{} · {}",
                item["Id"].as_str().unwrap_or("?"),
                item["Name"].as_str().unwrap_or("")
            )
        });
        let target = path.and_then(|path| {
            paths.target(
                path,
                matches!(item["Type"].as_str(), Some("Series" | "Season")),
            )
        });
        let directory = target.as_ref().and_then(|target| target.directory().ok());
        let blocked = if target.is_none() {
            Some("媒体路径缺失或未匹配写回映射。")
        } else if directory.is_none() {
            Some("无法访问媒体目录或视频。请检查挂载和权限。")
        } else {
            None
        };
        let image_fields = if item["Id"] == main_item["Id"] {
            fields
        } else {
            item["Id"]
                .as_str()
                .and_then(|id| episodes.get(id).copied())
                .unwrap_or(&Value::Null)
        };
        let artwork = artwork_sources(item, image_fields, fill_missing);
        let sidecars = artwork.into_iter().chain([(FileKind::Nfo, None)]);
        for (kind, public_image) in sidecars {
            let key = format!("{}\n{}", kind.label(), source);
            if !seen.insert(key.clone()) {
                continue;
            }
            let destinations = target
                .as_ref()
                .and_then(|target| {
                    let dir = directory.as_ref()?;
                    Some(
                        destination_names(kind, item, target, create_only)
                            .into_iter()
                            .map(|name| Destination {
                                existing: if target.is_video_name(&name) {
                                    Existing::Blocked
                                } else {
                                    Existing::read(dir, &name)
                                },
                                name,
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            if create_only && let (Some(dir), Some(target)) = (directory.as_ref(), target.as_ref())
            {
                let alternates = automatic_alternates(kind, item, target);
                if destinations
                    .iter()
                    .any(|entry| entry.existing != Existing::Missing)
                    || alternates
                        .iter()
                        .any(|name| Existing::read(dir, name) != Existing::Missing)
                {
                    continue;
                }
            }
            let mut file = PlannedFile {
                key,
                source: source.clone(),
                item_id: item["Id"].clone(),
                kind,
                public_image,
                target: target.clone(),
                destinations,
                blocked: blocked.map(str::to_owned),
                create_only,
            };
            if file.blocked.is_none()
                && file
                    .destinations
                    .iter()
                    .all(|destination| destination.existing == Existing::Blocked)
            {
                file.blocked = Some("目标不是普通文件，或无法读取现有资料。".into());
            }
            preview.push(file.preview(state));
            if !matches!(state.files.get(&file.key), Some(FileResult::Written { .. })) {
                writable += usize::from(file.blocked.is_none());
                files.push(file);
            }
        }
    }
    (files, preview, writable)
}

fn artwork_sources(
    item: &Value,
    fields: &Value,
    fill_missing: bool,
) -> Vec<(FileKind, Option<String>)> {
    let tags = &item["ImageTags"];
    let artwork: &[(FileKind, bool, Option<&str>)] = if item["Type"] == "Episode" {
        &[(
            FileKind::Still,
            tags["Primary"].is_string(),
            fields["still_url"].as_str(),
        )]
    } else {
        let backdrop = fields["backdrop_url"].as_str();
        &[
            (
                FileKind::Poster,
                tags["Primary"].is_string(),
                fields["poster_url"]
                    .as_str()
                    .or_else(|| fields["cover_url"].as_str()),
            ),
            (
                FileKind::Backdrop,
                item["BackdropImageTags"]
                    .as_array()
                    .is_some_and(|tags| !tags.is_empty()),
                backdrop,
            ),
            (
                FileKind::Logo,
                tags["Logo"].is_string(),
                fields["logo_url"].as_str(),
            ),
            (
                FileKind::Landscape,
                tags["Thumb"].is_string(),
                fields["landscape_url"]
                    .as_str()
                    .filter(|url| Some(*url) != backdrop),
            ),
        ]
    };
    artwork
        .iter()
        .filter_map(|&(kind, emby_available, url)| {
            let public_image = url
                .filter(|_| !fill_missing || !emby_available)
                .and_then(|url| Url::parse(url).ok())
                .filter(allowed)
                .map(|url| url.to_string());
            (emby_available || public_image.is_some()).then_some((kind, public_image))
        })
        .collect()
}

fn destination_names(
    kind: FileKind,
    item: &Value,
    target: &Target,
    create_only: bool,
) -> Vec<String> {
    let image_stem = match kind {
        FileKind::Nfo => {
            return vec![if target.is_video() {
                target.video_sidecar(".nfo")
            } else if item["Type"] == "Season" {
                "season.nfo".into()
            } else {
                "tvshow.nfo".into()
            }];
        }
        FileKind::Poster if target.is_video() => target.video_sidecar("-poster"),
        FileKind::Poster => "poster".into(),
        FileKind::Backdrop => {
            let stem = target.backdrop_stem();
            if create_only
                && stem != "fanart"
                && let Some(video) = target.stem()
            {
                format!("{video}-fanart")
            } else {
                stem
            }
        }
        FileKind::Logo if target.is_video() => target.video_sidecar("-clearlogo"),
        FileKind::Logo => "clearlogo".into(),
        FileKind::Landscape if target.is_video() => target.video_sidecar("-landscape"),
        FileKind::Landscape => "landscape".into(),
        FileKind::Still => target.video_sidecar("-thumb"),
    };
    image_names(image_stem)
}

fn automatic_alternates(kind: FileKind, item: &Value, target: &Target) -> Vec<String> {
    let mut names = Vec::new();
    let stem = target.stem();
    let image_stems = match kind {
        FileKind::Nfo => {
            if item["Type"] == "Movie" && target.backdrop_stem() == "fanart" {
                names.push("movie.nfo".into());
            }
            return names;
        }
        FileKind::Poster if item["Type"] != "Movie" || target.backdrop_stem() == "fanart" => {
            vec![
                "folder".to_owned(),
                "poster".into(),
                "cover".into(),
                "default".into(),
                "movie".into(),
                "show".into(),
            ]
        }
        FileKind::Poster => Vec::new(),
        FileKind::Backdrop if item["Type"] != "Movie" || target.backdrop_stem() == "fanart" => {
            vec![
                "backdrop".to_owned(),
                "fanart".into(),
                "background".into(),
                "art".into(),
            ]
        }
        FileKind::Backdrop => Vec::new(),
        FileKind::Logo => match stem {
            Some(stem) => vec![format!("{stem}-logo")],
            None => vec!["clearlogo".into(), "logo".into()],
        },
        FileKind::Landscape => match stem {
            Some(stem) => vec![format!("{stem}-thumb")],
            None => vec!["landscape".into(), "thumb".into()],
        },
        FileKind::Still => stem
            .map(|stem| vec![format!("metadata/{stem}")])
            .unwrap_or_default(),
    };
    let mut image_stems = image_stems;
    if stem.is_some() && target.backdrop_stem() == "fanart" {
        match kind {
            FileKind::Logo => image_stems.extend(["clearlogo".into(), "logo".into()]),
            FileKind::Landscape => image_stems.extend(["landscape".into(), "thumb".into()]),
            _ => {}
        }
    }
    if matches!(kind, FileKind::Poster)
        && let Some(stem) = stem
    {
        image_stems.extend(
            ["", "-poster", "-cover", "-default", "-movie"].map(|suffix| format!("{stem}{suffix}")),
        );
    }
    image_stems
        .into_iter()
        .flat_map(|name| {
            IMAGE_EXTENSIONS
                .iter()
                .chain(["jpeg", "tbn"].iter())
                .map(move |ext| format!("{name}.{ext}"))
        })
        .collect()
}

fn image_names(stem: String) -> Vec<String> {
    IMAGE_EXTENSIONS
        .iter()
        .map(|extension| format!("{stem}.{extension}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::publish::{BACKUP_DIRECTORY, publish_file, stage_file};
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn writeback_preview_includes_public_artwork_and_keeps_existing_emby_artwork() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("Film.mkv"), "video").unwrap();
        let paths = MediaPaths::new(
            vec![PathMapping {
                server: "/media".into(),
                local: String::new(),
            }],
            true,
            Some(root.path().into()),
        );
        let mut data = json!({"item":{"Id":"1","Type":"Movie","Path":"/media/Film.mkv"}});
        let poster_url = "https://image.tmdb.org/t/p/w500/poster.jpg";
        let mut record = json!({"fill_missing":true,"fields":{"poster_url":poster_url}});
        let state = WritebackState::default();

        let (files, _, writable) = plan(&paths, &data, &record, &state);
        assert_eq!(writable, 2);
        assert_eq!(files[0].public_image.as_deref(), Some(poster_url));
        assert_eq!(files[0].destinations[0].name, "Film-poster.jpg");

        data["item"]["ImageTags"] = json!({"Primary":"existing"});
        let (files, _, _) = plan(&paths, &data, &record, &state);
        assert!(files[0].public_image.is_none());

        record["fill_missing"] = json!(false);
        let (files, _, _) = plan(&paths, &data, &record, &state);
        assert_eq!(files[0].public_image.as_deref(), Some(poster_url));

        data["item"]["ImageTags"] = Value::Null;
        record["fields"]["poster_url"] = json!("http://127.0.0.1/other.jpg");
        let (files, _, _) = plan(&paths, &data, &record, &state);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].kind.label(), "nfo");
    }

    #[tokio::test]
    async fn automatic_writeback_keeps_existing_aliases_and_never_replaces_a_new_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let film = root.path().join("Film");
        fs::create_dir(&film).unwrap();
        fs::write(film.join("Film.mkv"), "video").unwrap();
        fs::write(film.join("movie.nfo"), "local metadata").unwrap();
        fs::write(film.join("poster.jpeg"), "local poster").unwrap();
        let paths = MediaPaths::new(
            vec![PathMapping {
                server: "/media".into(),
                local: String::new(),
            }],
            true,
            Some(root.path().into()),
        );
        let data = json!({"item":{"Id":"1","Type":"Movie","Path":"/media/Film/Film.mkv"}});
        let record = json!({"fields":{"poster_url":"https://image.tmdb.org/t/p/w500/p.jpg"}});
        let state = WritebackState::default();
        assert!(
            plan_with_mode(&paths, &data, &record, &state, true)
                .0
                .is_empty()
        );

        fs::remove_file(film.join("movie.nfo")).unwrap();
        let (files, _, _) = plan_with_mode(&paths, &data, &record, &state, true);
        assert_eq!(files.len(), 1);
        let staged = stage_file(root.path(), &files[0].destinations[0], "scraped")
            .await
            .unwrap();
        fs::write(film.join("Film.nfo"), "arrived during staging").unwrap();
        publish_file(&files[0], &staged, "no-backup").unwrap();
        assert_eq!(
            fs::read_to_string(film.join("Film.nfo")).unwrap(),
            "arrived during staging"
        );
        assert!(!film.join(BACKUP_DIRECTORY).exists());
    }

    #[test]
    fn automatic_sidecars_take_logo_and_landscape_from_emby_using_separate_names() {
        let root = tempfile::tempdir().unwrap();
        let series = root.path().join("Series");
        fs::create_dir(&series).unwrap();
        let paths = MediaPaths::new(
            vec![PathMapping {
                server: "/media".into(),
                local: String::new(),
            }],
            true,
            Some(root.path().into()),
        );
        let data = json!({"item":{"Id":"1","Type":"Series","Path":"/media/Series",
            "ImageTags":{"Logo":"png","Thumb":"jpg"}}});
        let record =
            json!({"fill_missing":true,"fields":{"logo_url":"https://image.tmdb.org/new.png"}});
        let files = plan_with_mode(&paths, &data, &record, &WritebackState::default(), true).0;
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].kind.image_type(), Some("Logo"));
        assert!(
            files[0]
                .destinations
                .iter()
                .any(|file| file.name == "clearlogo.png")
        );
        assert!(files[0].public_image.is_none());
        assert_eq!(files[1].kind.image_type(), Some("Thumb"));
        assert_eq!(files[1].destinations[0].name, "landscape.jpg");
        assert_eq!(files[2].destinations[0].name, "tvshow.nfo");
    }

    #[tokio::test]
    async fn writeback_checks_the_confirmed_target_and_reuses_its_backup_on_replay() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let nfo = root.join("Film.nfo");
        fs::write(root.join("Film.mkv"), "video").unwrap();
        fs::write(&nfo, "original").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&nfo)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH - std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let paths = MediaPaths::new(
            vec![PathMapping {
                server: "/media".into(),
                local: String::new(),
            }],
            true,
            Some(root.into()),
        );
        let data = json!({"item":{"Id":"1","Type":"Movie","Path":"/media/Film.mkv"}});
        let state = WritebackState::default();
        let (files, _, _) = plan(&paths, &data, &Value::Null, &state);
        serde_json::to_vec(&files).unwrap();
        let staged = stage_file(root, &files[0].destinations[0], "collected metadata")
            .await
            .unwrap();

        fs::write(&nfo, "changed after confirmation").unwrap();
        assert!(publish_file(&files[0], &staged, "first.bak").is_err());
        assert_eq!(
            fs::read_to_string(&nfo).unwrap(),
            "changed after confirmation"
        );
        assert!(!root.join(BACKUP_DIRECTORY).exists());

        let (files, _, _) = plan(&paths, &data, &Value::Null, &state);
        let staged = stage_file(root, &files[0].destinations[0], "collected metadata")
            .await
            .unwrap();
        for _ in 0..2 {
            let FileResult::Written {
                backup: Some(backup),
                ..
            } = publish_file(&files[0], &staged, "confirmed.bak").unwrap()
            else {
                panic!("expected a backup")
            };
            assert_eq!(
                fs::read_to_string(backup).unwrap(),
                "changed after confirmation"
            );
            assert_eq!(fs::read_to_string(&nfo).unwrap(), "collected metadata");
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
        let (files, _, writable) = plan(
            &paths,
            &json!({"item":{"Id":"1","Type":"Movie","Path":"/media/Film.mkv"}}),
            &Value::Null,
            &WritebackState::default(),
        );
        assert_eq!(writable, 0);
        assert!(files[0].blocked.is_some());
        assert_eq!(
            fs::read_to_string(outside.join("Film.nfo")).unwrap(),
            "untouched"
        );
    }
}
