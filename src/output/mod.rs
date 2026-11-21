mod assets;
mod emby;
mod media;
mod naming;
mod nfo;
mod settings;

use std::{
    fs,
    path::{Path, PathBuf},
};

use futures_util::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{LookupResponse, providers::media::emby::Emby};

pub(crate) use assets::image_bytes;
pub(crate) use settings::ExportPlan;
pub use settings::{OutputOptions, OutputSettings, OutputSettingsView};

#[derive(Clone, Deserialize, Serialize)]
pub struct OutputReport {
    directory: Option<String>,
    written: usize,
    failed: usize,
    message: Option<String>,
}

impl OutputReport {
    fn error(message: &str) -> Self {
        Self {
            directory: None,
            written: 0,
            failed: 0,
            message: Some(message.to_owned()),
        }
    }

    pub(crate) fn has_issue(&self) -> bool {
        self.message.is_some()
    }
}

#[derive(Serialize)]
struct OutputFile {
    path: String,
    kind: &'static str,
    source_url: Option<String>,
    person: Option<String>,
}

pub(crate) enum PreparedExport {
    Batch(BatchExport),
    Emby(emby::MediaExport),
}

impl PreparedExport {
    pub(crate) fn publish(self) -> OutputReport {
        match self {
            Self::Batch(batch) => batch.publish(),
            Self::Emby(media) => media.publish(),
        }
    }
}

pub(crate) fn emby_source(record: &Value) -> Option<&Value> {
    record["sources"]
        .as_array()?
        .iter()
        .find(|source| source["provider"] == "Emby")
        .map(|source| &source["data"])
}

pub(crate) use emby::prepare as prepare_emby;

pub(crate) struct BatchExport {
    temporary: tempfile::TempDir,
    destination: PathBuf,
    owner: String,
    report: OutputReport,
}

impl BatchExport {
    fn publish(self) -> OutputReport {
        let Some(parent) = self.destination.parent() else {
            return OutputReport::error("输出目录无效。");
        };
        if parent.canonicalize().ok().as_deref() != Some(parent) {
            return OutputReport::error("输出目录已移动或不可用。");
        }
        match fs::symlink_metadata(&self.destination) {
            Ok(entry) if entry.file_type().is_dir() => {
                let prior = fs::read(self.destination.join("files.json"))
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Value>(&data).ok());
                if let Some(prior) = prior
                    && prior["owner"].as_str() == Some(&self.owner)
                    && let Ok(report) = serde_json::from_value(prior["report"].clone())
                {
                    return report;
                }
                OutputReport::error("输出目录已经存在，无法写出文件。")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::rename(self.temporary.path(), &self.destination).is_ok() {
                    self.report
                } else {
                    OutputReport::error("无法写出文件，请检查输出目录。")
                }
            }
            _ => OutputReport::error("无法使用目标输出目录。"),
        }
    }
}

pub(crate) async fn prepare(
    client: &Client,
    emby: Option<&Emby>,
    plan: &ExportPlan,
    batch: i64,
    row: i64,
    result: &LookupResponse,
    source_files: &[String],
) -> Result<PreparedExport, OutputReport> {
    let root = plan
        .directory
        .as_deref()
        .ok_or_else(|| OutputReport::error("未配置批次输出目录。"))?;
    let parent = root.join(format!("batch-{batch}"));
    if root.canonicalize().ok().as_deref() != Some(root) {
        return Err(OutputReport::error("输出目录已移动或不可用。"));
    }
    settings::make_child_dir(&parent).map_err(|_| OutputReport::error("无法创建批次输出目录。"))?;
    let names = naming::Names::new(result, row);
    let destination = parent.join(names.directory(&plan.options.directory_template));
    let file_names = names.files(&plan.options.filename_prefix);
    let temporary = tempfile::Builder::new()
        .prefix(".collect-metadata-")
        .tempdir_in(&parent)
        .map_err(|_| OutputReport::error("无法创建临时输出目录。"))?;
    let mut files = Vec::new();
    let mut failed = 0;
    let options = &plan.options;
    let document = options
        .metadata
        .then(|| metadata(&result.record, options, source_files));
    for (path, value) in [
        ("metadata.json", document.as_ref()),
        (
            "raw-sources.json",
            options.raw.then_some(&result.record["sources"]),
        ),
    ] {
        if let Some(value) = value {
            let path = file_names.file(path);
            if write_json(&temporary.path().join(&path), value).is_ok() {
                files.push(OutputFile {
                    path,
                    kind: "metadata",
                    source_url: None,
                    person: None,
                });
            } else {
                failed += 1;
            }
        }
    }
    let directory = temporary.path();
    let mut downloads = stream::iter(
        assets::selected_assets(&result.record, options, emby)
            .into_iter()
            .enumerate(),
    )
    .map(|(index, asset)| {
        let file_names = &file_names;
        async move {
            let (bytes, ext) = assets::download(client, &asset, emby).await?;
            let name = match asset.kind {
                "poster" | "backdrop" => format!("{}.{ext}", asset.kind),
                _ => format!("{}-{:03}.{ext}", asset.kind, index + 1),
            };
            let name = file_names.file(&name);
            tokio::fs::write(directory.join(&name), bytes).await.ok()?;
            Some(OutputFile {
                path: name,
                kind: asset.kind,
                source_url: Some(asset.url.into()),
                person: asset.person,
            })
        }
    })
    .buffer_unordered(4);
    while let Some(item) = downloads.next().await {
        match item {
            Some(file) => files.push(file),
            None => failed += 1,
        }
    }
    drop(downloads);
    if options.emby_nfo {
        for (path, contents) in nfo::documents(&result.record, options, source_files, &files) {
            if tokio::fs::write(directory.join(&path), contents)
                .await
                .is_ok()
            {
                files.push(OutputFile {
                    path,
                    kind: "nfo",
                    source_url: None,
                    person: None,
                });
            } else {
                failed += 1;
            }
        }
    }
    let (media_files, media_failed, mut messages) =
        media::export(client, emby, directory, &result.record, options).await;
    files.extend(media_files);
    failed += media_failed;
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    if failed > 0 {
        messages.insert(0, format!("{failed} 个输出文件下载或写入失败。"));
    } else if files.is_empty() && messages.is_empty() {
        messages.push("来源没有提供所选资料的可用文件。".into());
    }
    let message = (!messages.is_empty()).then(|| messages.join(" "));
    let report = OutputReport {
        directory: Some(destination.to_string_lossy().into_owned()),
        written: files.len(),
        failed,
        message,
    };
    write_json(&temporary.path().join("files.json"), &json!({
        "owner": plan.owner,
        "report": report,
        "work": result.code(),
        "source_files": source_files,
        "files": files,
        "sources": result.record["sources"].as_array().into_iter().flatten().map(|source| json!({
            "provider": source["provider"], "url": source["url"],
        })).collect::<Vec<_>>()
    })).map_err(|_| OutputReport::error("无法写入输出清单。"))?;
    Ok(PreparedExport::Batch(BatchExport {
        temporary,
        destination,
        owner: plan.owner.clone(),
        report,
    }))
}

fn metadata(record: &Value, options: &settings::OutputOptions, source_files: &[String]) -> Value {
    let included = |name: &str| match name {
        "description" | "description_original" | "tagline" => options.synopsis,
        "cast" | "directors" | "actresses" | "actors" | "other_cast" | "writers" | "producers" => {
            options.credits
        }
        "rating" | "critic_rating" => options.ratings,
        "episodes" | "episode_count" | "season_count" => options.episodes,
        "poster_url" | "cover_url" => options.poster,
        "backdrop_url" => options.backdrop,
        _ => true,
    };
    let filtered = |key: &str| {
        record[key]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| included(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>()
    };
    let mut fields = filtered("fields");
    if !options.stills
        && let Some(episodes) = fields.get_mut("episodes").and_then(Value::as_array_mut)
    {
        for episode in episodes {
            if let Some(value) = episode.as_object_mut() {
                value.remove("still_url");
            }
        }
    }
    let localized: serde_json::Map<String, Value> = record["localized_metadata"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(language, fields)| {
            let fields = fields
                .as_object()?
                .iter()
                .filter(|(name, _)| included(name.split('/').next().unwrap_or(name)))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>();
            (!fields.is_empty()).then(|| (language.clone(), Value::Object(fields)))
        })
        .collect();
    let mut document = json!({
        "schema_version": record["schema_version"],
        "media_type": record["media_type"],
        "query": record["query"],
        "work_id": record["work_id"],
        "fields": fields,
        "field_sources": filtered("field_sources"),
        "source_files": source_files,
    });
    if record["scope"] == "episode" {
        document["scope"] = record["scope"].clone();
    }
    if record["metadata_languages"].is_array() {
        document["metadata_languages"] = record["metadata_languages"].clone();
        document["localized_metadata"] = Value::Object(localized);
    }
    document
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let file = fs::File::create(path)?;
    serde_json::to_writer_pretty(file, value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_selected_fields_and_preserves_the_completed_output_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let options = serde_json::from_value(json!({
            "enabled": true, "synopsis": false, "ratings": false, "credits": true, "poster": false
        }))
        .unwrap();
        let plan = ExportPlan {
            owner: "test-worker".into(),
            directory: Some(root.clone()),
            options,
            media_paths: None,
        };
        let result = LookupResponse {
            record: json!({
                "query":"Work", "fields":{"title":"Work","rating":{"value":8},"description":"hidden","directors":["Director"]},
                "field_sources":{"title":"TMDB","rating":"TMDB","description":"TMDB","directors":"TMDB"},
                "sources":[{"provider":"TMDB","data":{"extra":"raw"}}]
            }),
            saved: None,
            storage_error: None,
        };
        let client = Client::new();
        let linked = ["Library/Work.mkv".to_owned()];
        let prepared = prepare(&client, None, &plan, 1, 2, &result, &linked)
            .await
            .unwrap_or_else(|_| panic!("prepare output"));
        let directory = root.join("batch-1/work-2");
        assert!(!directory.exists());
        assert_eq!(prepared.publish().written, 1);
        let metadata: Value =
            serde_json::from_slice(&fs::read(directory.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(metadata["source_files"][0], "Library/Work.mkv");
        assert_eq!(metadata["fields"]["directors"][0], "Director");
        assert!(metadata["fields"]["rating"].is_null());
        assert!(metadata["field_sources"]["description"].is_null());
        assert!(!directory.join("raw-sources.json").exists());
        assert_eq!(
            prepare(&client, None, &plan, 1, 2, &result, &linked)
                .await
                .unwrap_or_else(|_| panic!("retry output"))
                .publish()
                .written,
            1
        );
    }

    #[test]
    fn selects_voice_and_director_portraits_from_distinct_roles() {
        let options = serde_json::from_value(json!({
            "poster": false, "portraits": ["voice", "director"]
        }))
        .unwrap();
        let result = json!({"sources":[{"provider":"TMDB", "data":{"credits":{
            "cast":[{"name":"Actor","character":"Person","profile_path":"/actor.jpg"},
                    {"name":"Voice","character":"Person (voice)","profile_path":"/voice.jpg"}],
            "crew":[{"name":"Director","job":"Director","profile_path":"/director.jpg"},
                    {"name":"Writer","job":"Writer","profile_path":"/writer.jpg"}]
        }}}]});
        let selected = assets::selected_assets(&result, &options, None);
        assert_eq!(
            selected.iter().map(|asset| asset.kind).collect::<Vec<_>>(),
            ["voice", "director"]
        );
    }
}
