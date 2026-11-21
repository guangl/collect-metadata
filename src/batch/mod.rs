mod folder;
#[cfg(feature = "web")]
mod mounted;
mod worker;
mod writeback;

use std::collections::HashSet;

use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Collector, EmbyLibraryPreview, EmbyPreviewImage, EmbySeriesPreview, EmbyWorkPreview,
    LookupResponse, MediaKind, MediaProvider, OutputReport,
    output::{BatchExport, ExportPlan, WritebackJob, WritebackState},
    providers::media::emby::{EPISODE_QUERY_PREFIX, Emby},
};

pub use writeback::WritebackPreview;
pub use folder::{
    FolderBatchResult, FolderPreview, FolderSelection, FolderWorkPreview, MAX_FOLDER_FILES,
    preview_folder,
};
#[cfg(feature = "desktop")]
pub use folder::{scan_folder_paths, scan_native_folder};
#[cfg(feature = "web")]
pub use mounted::{browse_mounted_folders, scan_mounted_folder};

#[derive(Deserialize)]
pub struct BatchItemInput {
    pub kind: MediaKind,
    pub query: String,
    #[serde(skip)]
    pub source_files: Vec<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbyScrapeScope {
    Movie,
    Series,
    Episode,
}

impl EmbyScrapeScope {
    fn item_type(self) -> &'static str {
        match self {
            Self::Movie => "Movie,Video",
            Self::Series => "Series",
            Self::Episode => "Episode",
        }
    }
}

#[derive(Deserialize)]
pub struct EmbyOverwriteInput {
    target: EmbyOverwriteTarget,
    #[serde(default)]
    replace_images: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EmbyOverwriteTarget {
    Library { id: String },
    Item { id: String, scope: EmbyScrapeScope },
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CandidateChoice {
    pub provider: MediaProvider,
    pub id: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Running,
    Paused,
    Cancelled,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchAction {
    Pause,
    Resume,
    Cancel,
    RetryFailed,
}

#[derive(Serialize)]
pub struct BatchView {
    pub status: BatchStatus,
    pub rows: Vec<BatchRow>,
}

#[derive(Serialize)]
pub struct BatchSummary {
    pub id: i64,
    pub created_at: String,
    pub folder_name: Option<String>,
    pub status: BatchStatus,
    pub total: i64,
    pub finished: i64,
    pub waiting: i64,
}

#[derive(Serialize)]
pub struct BatchRow {
    pub id: i64,
    pub kind: MediaKind,
    pub query: String,
    pub stage: String,
    pub message: Option<String>,
    pub candidates: Option<Value>,
    pub has_result: bool,
    pub title: Option<String>,
    pub cover_url: Option<String>,
    pub original_languages: Vec<String>,
    pub file_count: i32,
    pub output: Option<OutputReport>,
    pub writeback: Option<String>,
}

pub(crate) struct Claim {
    pub id: i64,
    pub batch: i64,
    pub attempt: i64,
    pub kind: MediaKind,
    pub query: String,
    pub candidate: Option<CandidateChoice>,
    pub from_folder: bool,
    pub export_plan: Option<ExportPlan>,
    pub source_files: Vec<String>,
    pub writeback: Option<(WritebackJob, WritebackState)>,
}

pub(crate) struct Outcome {
    pub stage: &'static str,
    pub message: Option<String>,
    pub candidates: Option<Value>,
    pub result: Option<LookupResponse>,
    pub output: Option<Result<BatchExport, OutputReport>>,
}

impl Outcome {
    fn new(stage: &'static str, message: Option<String>) -> Self {
        Self {
            stage,
            message,
            candidates: None,
            result: None,
            output: None,
        }
    }

    fn failed(message: String) -> Self {
        Self::new("failed", Some(message))
    }

    fn collected(result: LookupResponse) -> Self {
        let sources = result.record["sources"].as_array().into_iter().flatten();
        let notes = sources
            .clone()
            .filter(|source| matches!(source["status"].as_str(), Some("partial" | "unavailable")))
            .filter_map(|source| {
                Some(format!(
                    "{}：{}",
                    source["provider"].as_str()?,
                    source["note"].as_str()?
                ))
            })
            .collect::<Vec<_>>();
        let has_fields = result.has_fields();
        let has_sources = sources.clone().next().is_some();
        let stage = if has_fields {
            if sources
                .clone()
                .any(|source| matches!(source["status"].as_str(), Some("partial" | "unavailable")))
            {
                "partial"
            } else {
                "success"
            }
        } else if has_sources
            && sources
                .clone()
                .all(|source| source["status"] == "not_found")
        {
            "not_found"
        } else {
            "failed"
        };
        let message = if !notes.is_empty() {
            Some(notes.join(" "))
        } else {
            match stage {
                "not_found" => Some("来源没有找到匹配的作品。".into()),
                "failed" => Some("没有取得可用资料。".into()),
                "partial" => Some("已取得部分资料。".into()),
                _ => None,
            }
        };
        Self {
            stage,
            message,
            candidates: None,
            result: Some(result),
            output: None,
        }
    }
}

fn emby_batch_items(works: impl IntoIterator<Item = EmbyWorkPreview>) -> Vec<BatchItemInput> {
    works.into_iter().map(|work| BatchItemInput {
        kind: work.kind,
        query: format!("emby:{}", work.id),
        source_files: Vec::new(),
    }).collect()
}

impl Collector {
    fn emby_connection(&self) -> Result<std::sync::Arc<Emby>, String> {
        self.emby_settings()
            .and_then(|settings| settings.connection())
            .ok_or_else(|| "请先在「设置 → 家庭 Emby」配置服务地址和 API 密钥。".into())
    }

    pub async fn emby_libraries(&self) -> Result<Vec<crate::EmbyLibrary>, String> {
        self.emby_connection()?.libraries(&self.client).await
    }

    async fn read_emby_library(
        &self,
        id: &str,
    ) -> Result<(crate::EmbyLibrary, Vec<crate::EmbyWorkPreview>), String> {
        let emby = self.emby_connection()?;
        let library = emby.library(&self.client, id).await?;
        let works = emby.library_entries(&self.client, id).await?;
        Ok((library, works))
    }

    pub async fn preview_emby_library(
        &self,
        id: &str,
    ) -> Result<Vec<crate::EmbyWorkPreview>, String> {
        self.read_emby_library(id).await.map(|(_, works)| works)
    }

    async fn read_emby_catalog(&self, emby: &Emby) -> Result<Vec<EmbyLibraryPreview>, String> {
        let libraries = emby.libraries(&self.client).await?;
        stream::iter(libraries.into_iter().map(|library| async move {
            let works = emby
                .library_entries(&self.client, &library.id)
                .await
                .map_err(|error| format!("{}：{error}", library.name))?;
            Ok(EmbyLibraryPreview { library, works })
        }))
        .buffered(4)
        .try_collect()
        .await
    }

    pub async fn emby_catalog(&self) -> Result<Option<Vec<EmbyLibraryPreview>>, String> {
        let Some(emby) = self
            .emby_settings()
            .and_then(|settings| settings.connection())
        else {
            return Ok(None);
        };
        self.read_emby_catalog(&emby).await.map(Some)
    }

    pub async fn emby_preview_image(
        &self,
        id: &str,
        image: EmbyPreviewImage,
    ) -> Option<(Vec<u8>, &'static str)> {
        self.emby_connection()
            .ok()?
            .preview_image(&self.client, id, image)
            .await
    }

    pub async fn emby_credits(&self, id: &str) -> Result<Vec<crate::EmbyCredit>, String> {
        self.emby_connection()?.credits(&self.client, id).await
    }

    pub async fn emby_series_preview(&self, id: &str) -> Result<EmbySeriesPreview, String> {
        self.emby_connection()?
            .series_preview(&self.client, id)
            .await
    }

    pub async fn import_emby_catalog(&self) -> Result<i64, String> {
        let emby = self.emby_connection()?;
        let libraries = self.read_emby_catalog(&emby).await?;
        let mut seen = HashSet::new();
        let works = libraries
            .into_iter()
            .flat_map(|library| library.works)
            .filter(|work| seen.insert(work.id.clone()));
        self.save_emby_batch("Emby · 全部媒体库", emby_batch_items(works)).await
    }

    pub async fn import_emby_library(&self, id: &str) -> Result<i64, String> {
        let (library, works) = self.read_emby_library(id).await?;
        self.save_emby_batch(&format!("Emby · {}", library.name), emby_batch_items(works))
            .await
    }

    pub async fn import_emby_item(&self, id: &str, scope: EmbyScrapeScope) -> Result<i64, String> {
        let (kind, prefix) = match scope {
            EmbyScrapeScope::Movie => (MediaKind::Movie, "emby:"),
            EmbyScrapeScope::Series => (MediaKind::Series, "emby:"),
            EmbyScrapeScope::Episode => (MediaKind::Series, EPISODE_QUERY_PREFIX),
        };
        let title = self
            .emby_connection()?
            .scrape_title(&self.client, id, scope.item_type())
            .await?;
        self.save_emby_batch(
            &format!("Emby · {title}"),
            vec![BatchItemInput {
                kind,
                query: format!("{prefix}{id}"),
                source_files: Vec::new(),
            }],
        )
        .await
    }

    pub async fn overwrite_emby(&self, input: EmbyOverwriteInput) -> Result<(), String> {
        let emby = self.emby_connection()?;
        let (id, recursive) = match input.target {
            EmbyOverwriteTarget::Library { id } => {
                emby.library(&self.client, &id).await?;
                (id, true)
            }
            EmbyOverwriteTarget::Item { id, scope } => {
                emby.item(&self.client, &id, scope.item_type(), "").await?;
                (id, matches!(scope, EmbyScrapeScope::Series))
            }
        };
        emby.overwrite_metadata(&self.client, &id, recursive, input.replace_images)
            .await
    }

    async fn save_emby_batch(&self, name: &str, items: Vec<BatchItemInput>) -> Result<i64, String> {
        let store = self.require_store().await?;
        if items.is_empty() {
            return Err("此媒体库没有可刮削的电影、剧集或私人影片。".into());
        }
        let settings = self.output_settings().ok_or("后台输出配置不可用。")?;
        let owner = settings.durable_owner()?;
        store
            .create_batch(items, false, Some(name), None, Some(&owner))
            .await
            .map_err(|_| "无法保存 Emby 导入批次。".into())
    }

    pub async fn create_folder_batch(
        &self,
        paths: Vec<String>,
    ) -> Result<FolderBatchResult, String> {
        let store = self.require_store().await?;
        let (items, mut result) =
            tokio::task::spawn_blocking(move || folder::classify_paths(paths))
                .await
                .map_err(|_| "无法识别目录中的视频文件。")??;
        if !items.is_empty() {
            let read_emby = self
                .emby_settings()
                .and_then(|settings| settings.active())
                .is_some();
            let export = self
                .output_settings()
                .map(|settings| settings.folder_plan())
                .transpose()?
                .flatten();
            let folder_name = items[0].source_files[0]
                .split_once('/')
                .map(|(name, _)| name.to_owned());
            let owner = if read_emby {
                self.output_settings()
                    .map(|settings| settings.durable_owner())
                    .transpose()?
            } else {
                None
            };
            result.id = Some(
                store
                    .create_batch(
                        items,
                        self.tmdb_configured(),
                        folder_name.as_deref(),
                        export.as_ref(),
                        owner.as_deref(),
                    )
                    .await
                    .map_err(|_| "无法保存文件夹批次。")?,
            );
        }
        Ok(result)
    }

    pub async fn create_batch(&self, items: Vec<BatchItemInput>) -> Result<i64, String> {
        if !(1..=100).contains(&items.len()) {
            return Err("每批支持 1 到 100 条作品。".into());
        }
        if items.iter().any(|item| item.query.len() > 8192) {
            return Err("单条作品名称或番号过长。".into());
        }
        if !self.tmdb_configured()
            && items.iter().any(|item| {
                item.kind != MediaKind::Av
                    && crate::providers::media::tmdb_id_query(&item.query).is_some()
            })
        {
            return Err("批次包含 TMDB ID，请先在「设置」中配置 TMDB API 凭据。".into());
        }
        self.require_store()
            .await?
            .create_batch(items, self.tmdb_configured(), None, None, None)
            .await
            .map_err(|_| "无法保存批次。".into())
    }

    pub async fn list_batches(&self, before: Option<i64>) -> Result<Vec<BatchSummary>, String> {
        self.require_store()
            .await?
            .list_batches(before)
            .await
            .map_err(|_| "无法读取批次列表。".into())
    }

    pub async fn batch_view(&self, batch: i64) -> Result<BatchView, String> {
        self.require_store()
            .await?
            .batch_view(batch)
            .await
            .map_err(|_| "无法读取批次进度。")?
            .ok_or("没有找到此批次。".into())
    }

    pub async fn batch_rows(&self, batch: i64) -> Result<Vec<BatchRow>, String> {
        Ok(self.batch_view(batch).await?.rows)
    }

    pub async fn batch_files(&self, batch: i64, row: i64) -> Result<Vec<String>, String> {
        self.require_store()
            .await?
            .batch_files(batch, row)
            .await
            .map_err(|_| "无法读取条目的文件列表。")?
            .ok_or("没有找到此条目。".into())
    }

    pub async fn control_batch(&self, batch: i64, action: BatchAction) -> Result<u64, String> {
        self.require_store()
            .await?
            .control_batch(batch, action)
            .await
            .map_err(|_| "无法更新批次状态。")?
            .ok_or("批次状态已变动或没有符合条件的条目，请刷新后再试。".into())
    }

    pub async fn retry_batch_row(&self, batch: i64, item: i64) -> Result<(), String> {
        self.require_store()
            .await?
            .retry_batch_row(batch, item)
            .await
            .map_err(|_| "无法重试条目。")?
            .then_some(())
            .ok_or("条目当前无法重试。".into())
    }

    pub async fn select_batch_candidate(
        &self,
        batch: i64,
        item: i64,
        candidate: CandidateChoice,
    ) -> Result<(), String> {
        self.require_store()
            .await?
            .select_batch_candidate(batch, item, candidate)
            .await
            .map_err(|_| "无法保存候选选择。")?
            .then_some(())
            .ok_or("该候选已变动，请刷新进度后再选择。".into())
    }

    pub async fn batch_row_result(&self, batch: i64, item: i64) -> Result<LookupResponse, String> {
        self.require_store()
            .await?
            .batch_row_result(batch, item)
            .await?
            .ok_or("此条目暂无可查看的结果。".into())
    }
}
