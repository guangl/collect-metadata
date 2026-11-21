mod folder;
#[cfg(feature = "web")]
mod mounted;
mod worker;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Collector, LookupResponse, MediaKind, MediaProvider, OutputReport,
    output::{ExportPlan, PreparedExport},
};

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
}

pub(crate) struct Outcome {
    pub stage: &'static str,
    pub message: Option<String>,
    pub candidates: Option<Value>,
    pub result: Option<LookupResponse>,
    pub output: Option<Result<PreparedExport, OutputReport>>,
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

impl Collector {
    fn emby_connection(
        &self,
    ) -> Result<std::sync::Arc<crate::providers::media::emby::Emby>, String> {
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
        let library = emby
            .libraries(&self.client)
            .await?
            .into_iter()
            .find(|library| library.id == id)
            .ok_or("没有找到此 Emby 媒体库。")?;
        let works = emby.library_entries(&self.client, id).await?;
        Ok((library, works))
    }

    pub async fn preview_emby_library(
        &self,
        id: &str,
    ) -> Result<Vec<crate::EmbyWorkPreview>, String> {
        self.read_emby_library(id).await.map(|(_, works)| works)
    }

    pub async fn import_emby_library(&self, id: &str) -> Result<i64, String> {
        let store = self.require_store().await?;
        let (library, works) = self.read_emby_library(id).await?;
        let items = works
            .into_iter()
            .map(|work| BatchItemInput {
                kind: work.kind,
                query: format!("emby:{}", work.id),
                source_files: Vec::new(),
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Err("此媒体库没有电影或剧集。".into());
        }
        let settings = self.output_settings().ok_or("后台输出配置不可用。")?;
        let owner = settings.durable_owner()?;
        let export = settings.plan()?;
        let name = format!("Emby · {}", library.name);
        store
            .create_batch(items, false, Some(&name), export.as_ref(), Some(&owner))
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
            let export = self
                .output_settings()
                .map(|settings| settings.plan())
                .transpose()?
                .flatten();
            let folder_name = items[0].source_files[0]
                .split_once('/')
                .map(|(name, _)| name.to_owned());
            let owner = if self
                .emby_settings()
                .and_then(|settings| settings.active())
                .is_some()
            {
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
