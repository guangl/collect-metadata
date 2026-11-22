use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use cap_std::fs::{Dir, OpenOptions};
use futures_util::{StreamExt, stream::FuturesOrdered};
use reqwest::{Client, Url};
use serde_json::{Value, json};

use crate::providers::media::emby::{Emby, episode_fields};

use super::super::{
    OutputFile, OutputOptions, OutputReport,
    assets::{image_bytes, public_response},
    nfo,
};
use super::{
    Destination, Existing, FileResult, MediaPaths, PlannedFile, Target, WritebackJob,
    WritebackState,
};
#[cfg(feature = "web")]
use super::{FileKind, plan_with_mode};

pub(super) const BACKUP_DIRECTORY: &str = ".collect-metadata-backups";

pub(super) struct StagedFile {
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
            for kind in ["poster", "backdrop", "logo", "landscape", "still"] {
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
            url.query_pairs_mut()
                .append_pair("EnableImageEnhancers", "false");
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
            .is_some_and(|target| !target.is_video()),
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

pub(super) async fn stage_file(
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

pub(super) fn publish_file(
    file: &PlannedFile,
    staged: &StagedFile,
    backup_name: &str,
) -> Result<FileResult, String> {
    let target = file.target.as_ref().ok_or("未配置媒体路径。")?;
    let dir = target.directory().map_err(|_| "媒体目录或视频已不可用。")?;
    let name = &staged.destination.name;
    let previous = &staged.destination.existing;
    let current = Existing::read(&dir, name);
    if file.create_only {
        if current == Existing::Missing {
            write_atomic(
                &dir,
                name,
                fs::File::open(&staged.path).map_err(|_| "无法读取暂存资料。")?,
                true,
                || true,
            )
            .map_err(|_| "无法写入新资料，请检查挂载和权限。")?;
        } else if current == Existing::Blocked {
            return Err("目标在写回时已变更。".into());
        }
        return Ok(FileResult::Written {
            path: target.path(name),
            backup: None,
        });
    }
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
        false,
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
    write_atomic(&backups, backup_name, input, true, || {
        Existing::read(dir, name) == *previous
            && Existing::read(&backups, backup_name) == Existing::Missing
    })
}

fn write_atomic(
    dir: &Dir,
    name: &str,
    mut input: impl io::Read,
    create_only: bool,
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
        if create_only {
            dir.hard_link(&temporary, dir, name)
        } else {
            dir.rename(&temporary, dir, name)
        }
    })();
    if create_only || result.is_err() {
        let _ = dir.remove_file(&temporary);
    }
    result
}

#[cfg(feature = "web")]
pub(crate) async fn write_automatic(
    client: &Client,
    emby: &Emby,
    paths: &MediaPaths,
    record: &Value,
    defer_episode_nfo: bool,
) -> Result<usize, String> {
    use crate::providers::media::episode_title::{field_locked, usable};

    let data = super::super::emby_source(record).ok_or("缺少 Emby 文件关联。")?;
    let state = WritebackState::default();
    let item = &data["item"];
    let mut repaired = 0;
    let (mut files, _, _) = plan_with_mode(paths, data, record, &state, true);
    if item["Type"] == "Episode" {
        let title = &record["fields"]["title"];
        if defer_episode_nfo || title.as_str().is_none_or(|title| title.trim().is_empty()) {
            files.retain(|file| !matches!(file.kind, FileKind::Nfo));
        } else if !field_locked(item, "Name")
            && let Some(title) = usable(title, item)
            && let Some(target) = item["Path"]
                .as_str()
                .and_then(|path| paths.target(path, false))
        {
            let item = item.clone();
            let title = title.to_owned();
            repaired = tokio::task::spawn_blocking(move || {
                repair_nfo(&target, &target.video_sidecar(".nfo"), |xml| {
                    nfo::fill_episode_title(xml, &item, &title)
                })
            })
            .await
            .map_err(|_| "无法补充分集 NFO 标题。")?
            .map_err(|error| format!("无法补充分集 NFO 标题：{error}"))? as usize;
        }
    } else if item["Type"] == "Season"
        && !field_locked(item, "Overview")
        && let Some(number) = item["IndexNumber"].as_u64()
        && let Some(plot) = record["fields"]["description"]
            .as_str()
            .filter(|plot| !plot.trim().is_empty())
        && let Some(target) = item["Path"]
            .as_str()
            .and_then(|path| paths.target(path, true))
    {
        let plot = plot.to_owned();
        repaired = tokio::task::spawn_blocking(move || {
            repair_nfo(&target, "season.nfo", |xml| {
                nfo::fill_season_plot(xml, number, &plot)
            })
        })
        .await
        .map_err(|_| "无法补充季 NFO 简介。")?
        .map_err(|error| format!("无法补充季 NFO 简介：{error}"))? as usize;
    }
    if files.is_empty() {
        return Ok(repaired);
    }
    let job = WritebackJob {
        id: uuid::Uuid::new_v4().simple().to_string(),
        owner: String::new(),
        stage: String::new(),
        files,
    };
    let output = prepare(client, Some(emby), paths, &job, record, &state).await;
    if let Some(error) = output
        .files
        .iter()
        .find_map(|(_, staged)| staged.as_ref().err())
    {
        return Err(error.clone());
    }
    let (state, report) = tokio::task::spawn_blocking(move || output.publish(&job.id, state))
        .await
        .map_err(|_| "无法保存自动写回资料。")?;
    if let Some(error) = state.files.into_values().find_map(|file| match file {
        FileResult::Failed { error } => Some(error),
        _ => None,
    }) {
        return Err(error);
    }
    Ok(report.written + repaired)
}

#[cfg(feature = "web")]
fn repair_nfo(
    target: &Target,
    name: &str,
    edit: impl FnOnce(&str) -> Option<String>,
) -> io::Result<bool> {
    use io::Read;

    const MAX_NFO: u64 = 1024 * 1024;
    let dir = target.directory()?;
    let previous = Existing::read(&dir, name);
    if !matches!(&previous, Existing::File { length, .. } if *length <= MAX_NFO) {
        return Ok(false);
    }
    let mut original = Vec::new();
    dir.open(name)?
        .take(MAX_NFO + 1)
        .read_to_end(&mut original)?;
    if original.len() as u64 > MAX_NFO {
        return Ok(false);
    }
    let Some(updated) = std::str::from_utf8(&original).ok().and_then(edit) else {
        return Ok(false);
    };
    let backup = format!("automatic-{}.bak", uuid::Uuid::new_v4().simple());
    preserve_existing(&dir, name, &previous, &backup)?;
    write_atomic(&dir, name, updated.as_bytes(), false, || {
        Existing::read(&dir, name) == previous
    })?;
    Ok(true)
}
