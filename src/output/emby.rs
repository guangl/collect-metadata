use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use futures_util::{StreamExt, stream::FuturesOrdered};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    OutputFile, OutputReport,
    assets::{allowed, image_bytes, public_response},
    nfo,
    settings::OutputOptions,
};
use crate::providers::media::emby::{Emby, episode_fields};

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

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
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

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "png", "webp", "gif"];
const BACKUP_DIRECTORY: &str = ".collect-metadata-backups";

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FileKind {
    Nfo,
    Poster,
    Backdrop,
    Still,
}

impl FileKind {
    fn label(self) -> &'static str {
        match self {
            Self::Nfo => "nfo",
            Self::Poster => "poster",
            Self::Backdrop => "backdrop",
            Self::Still => "still",
        }
    }

    fn image_type(self) -> Option<&'static str> {
        match self {
            Self::Nfo => None,
            Self::Poster | Self::Still => Some("Primary"),
            Self::Backdrop => Some("Backdrop"),
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

impl Target {
    fn path(&self, name: &str) -> String {
        self.root
            .join(&self.relative)
            .join(name)
            .to_string_lossy()
            .into_owned()
    }
}

pub(crate) fn plan(
    paths: &MediaPaths,
    data: &Value,
    record: &Value,
    state: &WritebackState,
) -> (Vec<PlannedFile>, Vec<WritebackPreviewFile>, usize) {
    let item = &data["item"];
    let series = item["Type"] == "Series";
    let children = data["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|child| series && child["Type"] == "Episode" && child["LocationType"] != "Virtual");
    let fields = &record["fields"];
    let episodes: HashMap<_, _> = fields["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|episode| Some((episode["id"].as_str()?, episode)))
        .collect();
    let fill_missing = record["fill_missing"] == true;
    let main_item = item;
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut preview = Vec::new();
    let mut writable = 0;
    for item in std::iter::once(item).chain(children) {
        let path = item["Path"].as_str().filter(|path| !path.is_empty());
        let source = path.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "Emby #{} · {}",
                item["Id"].as_str().unwrap_or("?"),
                item["Name"].as_str().unwrap_or("")
            )
        });
        let target = path.and_then(|path| paths.target(path, item["Type"] == "Series"));
        let directory = target.as_ref().and_then(|target| target.directory().ok());
        let blocked = if target.is_none() {
            Some("媒体路径缺失或未匹配写回映射。")
        } else if directory.is_none() {
            Some("无法访问媒体目录或视频。请检查挂载和权限。")
        } else {
            None
        };
        let episode = item["Type"] == "Episode";
        let image_fields = if item["Id"] == main_item["Id"] {
            fields
        } else {
            item["Id"]
                .as_str()
                .and_then(|id| episodes.get(id).copied())
                .unwrap_or(&Value::Null)
        };
        let primary = if episode {
            image_fields["still_url"].as_str()
        } else {
            image_fields["poster_url"]
                .as_str()
                .or_else(|| image_fields["cover_url"].as_str())
        };
        let mut kinds = Vec::new();
        for (kind, emby_available, url) in [
            (
                if episode {
                    FileKind::Still
                } else {
                    FileKind::Poster
                },
                item["ImageTags"]["Primary"].is_string(),
                primary,
            ),
            (
                FileKind::Backdrop,
                !episode
                    && item["BackdropImageTags"]
                        .as_array()
                        .is_some_and(|tags| !tags.is_empty()),
                (!episode)
                    .then(|| image_fields["backdrop_url"].as_str())
                    .flatten(),
            ),
        ] {
            let public_image = url
                .filter(|_| !fill_missing || !emby_available)
                .and_then(|url| Url::parse(url).ok())
                .filter(allowed)
                .map(|url| url.to_string());
            if emby_available || public_image.is_some() {
                kinds.push((kind, public_image));
            }
        }
        kinds.push((FileKind::Nfo, None));
        for (kind, public_image) in kinds {
            let key = format!("{}\n{}", kind.label(), source);
            if !seen.insert(key.clone()) {
                continue;
            }
            let destinations = target
                .as_ref()
                .and_then(|target| {
                    let dir = directory.as_ref()?;
                    let names = match kind {
                        FileKind::Nfo => vec![if target.video.is_some() {
                            target.video_sidecar(".nfo")
                        } else {
                            "tvshow.nfo".into()
                        }],
                        FileKind::Poster => image_names(if target.video.is_some() {
                            target.video_sidecar("-poster")
                        } else {
                            "poster".into()
                        }),
                        FileKind::Backdrop => image_names(target.backdrop_stem()),
                        FileKind::Still => image_names(target.video_sidecar("-thumb")),
                    };
                    Some(
                        names
                            .into_iter()
                            .map(|name| Destination {
                                existing: if target
                                    .video
                                    .as_ref()
                                    .is_some_and(|video| video.eq_ignore_ascii_case(&name))
                                {
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
            let mut file = PlannedFile {
                key,
                source: source.clone(),
                item_id: item["Id"].clone(),
                kind,
                public_image,
                target: target.clone(),
                destinations,
                blocked: blocked.map(str::to_owned),
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

fn image_names(stem: String) -> Vec<String> {
    IMAGE_EXTENSIONS
        .iter()
        .map(|extension| format!("{stem}.{extension}"))
        .collect()
}

struct StagedFile {
    destination: Destination,
    path: PathBuf,
}

pub(crate) struct PreparedWriteback {
    _stage: Option<tempfile::TempDir>,
    files: Vec<(PlannedFile, Result<StagedFile, String>)>,
}

pub(crate) async fn prepare(
    client: &Client,
    emby: Option<&Emby>,
    paths: &MediaPaths,
    job: &WritebackJob,
    record: &Value,
    state: &WritebackState,
) -> PreparedWriteback {
    let stage = tempfile::tempdir().ok();
    let source = record["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|source| source["provider"] == "Emby");
    let emby = emby.filter(|emby| source.is_some_and(|source| emby.owns_source(source)));
    let data = source.map(|source| &source["data"]).unwrap_or(&Value::Null);
    let children: HashMap<_, _> = data["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|child| Some((child["Id"].as_str()?, child)))
        .collect();
    let stage_path = stage.as_ref().map(tempfile::TempDir::path);
    let mut files = Vec::new();
    {
        let mut jobs = FuturesOrdered::new();
        let mut pending = job
            .files
            .iter()
            .filter_map(|file| Some((file, file.kind.image_type()?)));
        loop {
            for (file, image_type) in pending.by_ref().take(4 - jobs.len()) {
                jobs.push_back(prepare_image(
                    client, emby, paths, stage_path, file, image_type,
                ));
            }
            let Some(file) = jobs.next().await else { break };
            files.push(file);
        }
    }
    let mut images: HashMap<&str, Vec<OutputFile>> = HashMap::new();
    for (file, staged) in &files {
        if let Ok(staged) = staged {
            images
                .entry(&file.source)
                .or_default()
                .push(image_reference(file.kind.label(), &staged.destination.name));
        }
    }
    let episodes: HashMap<_, _> = record["fields"]["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|episode| Some((episode["id"].as_str()?, episode)))
        .collect();
    let mut nfos = Vec::new();
    for file in job
        .files
        .iter()
        .filter(|file| file.kind.image_type().is_none())
    {
        let staged = async {
            check_target(file, paths)?;
            let stage = stage_path.ok_or("无法创建写回临时目录。")?;
            let id = file.item_id.as_str().unwrap_or_default();
            let episode = children
                .get(id)
                .map(|item| episode_record(item, episodes.get(id).copied(), record));
            let mut images = images.remove(file.source.as_str()).unwrap_or_default();
            for kind in ["poster", "backdrop", "still"] {
                if let Some(FileResult::Written { path, .. }) =
                    state.files.get(&format!("{kind}\n{}", file.source))
                    && let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str())
                {
                    images.push(image_reference(kind, name));
                }
            }
            let xml = nfo::work_xml(
                episode.as_ref().unwrap_or(record),
                &OutputOptions::default(),
                &images,
            );
            stage_file(stage, &file.destinations[0], xml).await
        }
        .await;
        nfos.push((file.clone(), staged));
    }
    files.extend(nfos);
    PreparedWriteback {
        _stage: stage,
        files,
    }
}

async fn prepare_image(
    client: &Client,
    emby: Option<&Emby>,
    paths: &MediaPaths,
    stage: Option<&Path>,
    file: &PlannedFile,
    image_type: &str,
) -> (PlannedFile, Result<StagedFile, String>) {
    let staged = async {
        check_target(file, paths)?;
        let stage = stage.ok_or("无法创建写回临时目录。")?;
        let response = if let Some(url) = &file.public_image {
            let url = Url::parse(url).map_err(|_| "来源图片地址无效。")?;
            public_response(client, &url, "image/*", Duration::from_secs(10))
                .await
                .ok_or("无法下载公开来源的图片。")?
        } else {
            let emby = emby.ok_or("无法连接此快照对应的 Emby 服务来读取图片。")?;
            let mut url = emby
                .image_url(&file.item_id, image_type)
                .ok_or("Emby 图片地址无效。")?;
            url.query_pairs_mut().append_pair("format", "jpg");
            emby.image_response(client, &url)
                .await
                .ok_or("无法下载 Emby 图片。")?
        };
        let (bytes, extension) = image_bytes(response)
            .await
            .ok_or("图片格式无效或超出大小限制。")?;
        let destination = file
            .destinations
            .iter()
            .find(|entry| {
                Path::new(&entry.name)
                    .extension()
                    .is_some_and(|ending| ending == extension)
            })
            .ok_or("未找到预览中的图片文件名。")?;
        stage_file(stage, destination, bytes).await
    }
    .await;
    (file.clone(), staged)
}

fn check_target(file: &PlannedFile, paths: &MediaPaths) -> Result<(), String> {
    if let Some(error) = &file.blocked {
        return Err(error.clone());
    }
    if paths.target(
        &file.source,
        file.target
            .as_ref()
            .is_some_and(|target| target.video.is_none()),
    ) != file.target
    {
        return Err("写回映射已变更，请重新预览。".into());
    }
    Ok(())
}

fn image_reference(kind: &'static str, name: &str) -> OutputFile {
    OutputFile {
        path: name.into(),
        kind,
        source_url: None,
        person: None,
    }
}

fn episode_record(item: &Value, localized: Option<&Value>, record: &Value) -> Value {
    let mut fields = episode_fields(item);
    if let Some(localized) = localized {
        for key in ["title", "description", "description_original"] {
            if let Some(text) = localized.get(key).filter(|text| text.is_string()) {
                fields.insert(key, text.clone());
            }
        }
    }
    if !fields.contains_key("series") {
        fields.insert("series", record["fields"]["title"].clone());
    }
    json!({"scope": "episode", "media_type": "series", "fields": fields})
}

async fn stage_file(
    stage: &Path,
    destination: &Destination,
    bytes: impl AsRef<[u8]>,
) -> Result<StagedFile, String> {
    if destination.existing == Existing::Blocked {
        return Err("目标不是普通文件，或无法读取现有资料。".into());
    }
    let path = stage.join(uuid::Uuid::new_v4().simple().to_string());
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|_| "无法暂存写回文件。")?;
    Ok(StagedFile {
        destination: destination.clone(),
        path,
    })
}

impl PreparedWriteback {
    pub(crate) fn publish(
        self,
        id: &str,
        mut state: WritebackState,
    ) -> (WritebackState, OutputReport) {
        let directory = self
            .files
            .iter()
            .find_map(|(file, _)| file.target.as_ref().map(Target::display_path));
        for (index, (file, staged)) in self.files.into_iter().enumerate() {
            // The job ID remains stable when the queue reclaims an interrupted attempt.
            let result = staged
                .and_then(|staged| publish_file(&file, &staged, &format!("{id}-{index}.bak")));
            state.files.insert(
                file.key,
                result.unwrap_or_else(|error| FileResult::Failed { error }),
            );
        }
        let failed = state
            .files
            .values()
            .filter(|file| matches!(file, FileResult::Failed { .. }))
            .count();
        state.remaining = Some(failed);
        let report = OutputReport {
            directory,
            written: state.files.len() - failed,
            failed,
            message: (failed > 0)
                .then(|| format!("{failed} 项资料未写回。重新预览可查看原因并重试。")),
        };
        (state, report)
    }
}

fn publish_file(
    file: &PlannedFile,
    staged: &StagedFile,
    backup_name: &str,
) -> Result<FileResult, String> {
    let target = file.target.as_ref().ok_or("未配置媒体路径。")?;
    let dir = target.directory().map_err(|_| "媒体目录或视频已不可用。")?;
    let name = &staged.destination.name;
    let previous = &staged.destination.existing;
    let current = Existing::read(&dir, name);
    let backup = format!("{BACKUP_DIRECTORY}/{backup_name}");
    let backup_exists = dir
        .symlink_metadata(&backup)
        .is_ok_and(|metadata| metadata.is_file());
    if matches!(current, Existing::File { .. })
        && (backup_exists || matches!(previous, Existing::Missing))
        && same_contents(&dir, name, &staged.path)
    {
        return Ok(FileResult::Written {
            path: target.path(name),
            backup: backup_exists.then(|| target.path(&backup)),
        });
    }
    if &current != previous {
        return Err("目标在预览后已变更，请重新预览。".into());
    }
    let replaced = matches!(current, Existing::File { .. });
    if replaced {
        preserve_existing(&dir, name, previous, backup_name)
            .map_err(|_| "无法保留旧文件，写回已跳过。")?;
    }
    write_atomic(
        &dir,
        name,
        fs::File::open(&staged.path).map_err(|_| "无法读取暂存资料。")?,
        || Existing::read(&dir, name) == *previous,
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            "目标在预览后已变更，请重新预览。"
        } else {
            "无法写回文件，请检查挂载和写入权限。"
        }
    })?;
    Ok(FileResult::Written {
        path: target.path(name),
        backup: replaced.then(|| target.path(&backup)),
    })
}

fn same_contents(dir: &Dir, name: &str, staged: &Path) -> bool {
    let read = || -> io::Result<bool> {
        let existing = dir.open(name)?;
        if existing.metadata()?.len() != fs::metadata(staged)?.len() {
            return Ok(false);
        }
        let mut existing = io::BufReader::new(existing);
        let mut staged = io::BufReader::new(fs::File::open(staged)?);
        use io::BufRead;
        loop {
            let left = existing.fill_buf()?;
            let right = staged.fill_buf()?;
            if left != right || left.is_empty() {
                return Ok(left == right);
            }
            let count = left.len();
            existing.consume(count);
            staged.consume(count);
        }
    };
    read().unwrap_or(false)
}

fn preserve_existing(
    dir: &Dir,
    name: &str,
    previous: &Existing,
    backup_name: &str,
) -> io::Result<()> {
    match dir.create_dir(BACKUP_DIRECTORY) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !dir.symlink_metadata(BACKUP_DIRECTORY)?.is_dir() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let backups = dir.open_dir(BACKUP_DIRECTORY)?;
    match Existing::read(&backups, backup_name) {
        Existing::File { .. } => return Ok(()),
        Existing::Blocked => return Err(io::ErrorKind::AlreadyExists.into()),
        Existing::Missing => {}
    }
    let input = dir.open(name)?;
    write_atomic(&backups, backup_name, input, || {
        Existing::read(dir, name) == *previous
            && Existing::read(&backups, backup_name) == Existing::Missing
    })
}

fn write_atomic(
    dir: &Dir,
    name: &str,
    mut input: impl io::Read,
    current: impl FnOnce() -> bool,
) -> io::Result<()> {
    let temporary = format!(".collect-metadata-{}.tmp", uuid::Uuid::new_v4().simple());
    let mut output = dir.open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
    let result = (|| {
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        if !current() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        dir.rename(&temporary, dir, name)
    })();
    if result.is_err() {
        let _ = dir.remove_file(&temporary);
    }
    result
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
