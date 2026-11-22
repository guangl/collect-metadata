pub(crate) mod ai;
#[cfg(feature = "web")]
mod auto_scrape;
mod emby;
mod folder;
#[cfg(feature = "web")]
mod mounted;
mod worker;
mod writeback;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Collector, LookupResponse, MediaKind, MediaProvider, OutputReport,
    output::{BatchExport, ExportPlan, WritebackJob, WritebackState},
    providers::media::emby::EPISODE_QUERY_PREFIX,
};

#[cfg(feature = "web")]
pub use auto_scrape::AutoScrape;
pub use emby::{EmbyOverwriteInput, EmbyScrapeScope, EmbyScrapeTarget, EmbySelectionInput};
pub use folder::{
    FolderBatchResult, FolderPreview, FolderSelection, FolderWorkPreview, MAX_FOLDER_FILES,
    preview_folder,
};
#[cfg(feature = "desktop")]
pub use folder::{scan_folder_paths, scan_native_folder};
#[cfg(feature = "web")]
pub use mounted::{browse_mounted_folders, scan_mounted_folder};
pub use writeback::WritebackPreview;

#[derive(Deserialize)]
pub struct BatchItemInput {
    pub kind: MediaKind,
    pub query: String,
    #[serde(skip)]
    pub source_files: Vec<String>,
}

#[derive(Deserialize)]
pub struct AiBatchInput {
    pub items: Vec<BatchItemInput>,
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

#[cfg(feature = "desktop")]
#[derive(Serialize)]
pub struct FolderWorkResult {
    stage: &'static str,
    message: Option<String>,
    candidates: Option<Value>,
    result: Option<LookupResponse>,
    output: Option<OutputReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_id: Option<String>,
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

#[derive(Clone, Copy)]
pub(crate) enum MatchMode {
    Existing,
    FillMissing,
    Ai,
}

impl MatchMode {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::FillMissing => "fill_missing",
            Self::Ai => "ai",
        }
    }
}

pub(crate) struct Claim {
    pub id: i64,
    pub batch: i64,
    pub attempt: i64,
    pub kind: MediaKind,
    pub query: String,
    pub candidate: Option<CandidateChoice>,
    pub remember_match: bool,
    pub use_ai: bool,
    pub fill_missing: bool,
    pub export_revision: i64,
    pub folder_root: Option<String>,
    pub export_plan: Option<ExportPlan>,
    pub source_files: Vec<String>,
    pub writeback: Option<(WritebackJob, WritebackState)>,
}

pub(crate) struct Outcome {
    pub stage: &'static str,
    pub message: Option<String>,
    pub candidates: Option<Value>,
    pub result: Option<LookupResponse>,
    pub remembered_match: Option<(String, CandidateChoice)>,
    pub output: Option<Result<BatchExport, OutputReport>>,
}

impl Outcome {
    fn new(stage: &'static str, message: Option<String>) -> Self {
        Self {
            stage,
            message,
            candidates: None,
            result: None,
            remembered_match: None,
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
            remembered_match: None,
            output: None,
        }
    }
}

impl Collector {
    #[cfg(feature = "desktop")]
    pub async fn scrape_folder_work(
        &self,
        batch: i64,
        row: i64,
        paths: Vec<String>,
        candidate: Option<CandidateChoice>,
    ) -> Result<FolderWorkResult, String> {
        if batch >= 0 || row <= 0 {
            return Err("本地刮削任务编号无效。".into());
        }
        let (items, _) = folder::classify_paths(paths)?;
        let [item]: [BatchItemInput; 1] = items
            .try_into()
            .map_err(|_| "请选择属于同一作品的视频文件。")?;
        let export_plan = self
            .output_settings()
            .map(|settings| settings.folder_plan())
            .transpose()?
            .flatten();
        let claim = Claim {
            id: row,
            batch,
            attempt: 0,
            kind: item.kind,
            query: item.query,
            candidate,
            remember_match: false,
            use_ai: false,
            fill_missing: false,
            export_revision: 0,
            folder_root: item.source_files[0]
                .split_once('/')
                .map(|(root, _)| root.to_owned()),
            export_plan,
            source_files: item.source_files,
            writeback: None,
        };
        let outcome = self
            .process_claim(None, &claim)
            .await
            .ok_or("无法完成本地刮削。")?;
        let output = outcome
            .output
            .map(|prepared| prepared.map_or_else(|report| report, BatchExport::publish));
        let output_issue = output.as_ref().is_some_and(OutputReport::has_issue);
        let diagnostic_id = if matches!(outcome.stage, "failed" | "not_found") || output_issue {
            crate::diagnostics::warn(
                "local_folder_scrape_issue",
                serde_json::json!({"stage": outcome.stage, "output_issue": output_issue}),
            );
            crate::diagnostics::current_request_id()
        } else {
            None
        };
        Ok(FolderWorkResult {
            stage: outcome.stage,
            message: outcome.message,
            candidates: outcome.candidates,
            result: outcome.result.filter(LookupResponse::has_fields),
            output,
            diagnostic_id,
        })
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
        self.create_manual_batch(items, false).await
    }

    pub async fn create_ai_batch(&self, input: AiBatchInput) -> Result<i64, String> {
        self.create_manual_batch(input.items, true).await
    }

    async fn create_manual_batch(
        &self,
        items: Vec<BatchItemInput>,
        use_ai: bool,
    ) -> Result<i64, String> {
        self.check_ai(use_ai)?;
        if items.iter().any(|item| {
            let query = item.query.trim();
            query.starts_with("emby:") || query.starts_with(EPISODE_QUERY_PREFIX)
        }) {
            return Err("Emby 条目请通过媒体库入口提交。".into());
        }
        if use_ai && items.iter().any(|item| item.kind == MediaKind::Av) {
            return Err("AI 批次只接受电影和剧集片名或来源 ID。".into());
        }
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
        let mode = if use_ai {
            MatchMode::Ai
        } else {
            MatchMode::Existing
        };
        self.require_store()
            .await?
            .create_batch_mode(
                items,
                self.tmdb_configured() && !use_ai,
                use_ai.then_some("AI 刮削"),
                None,
                None,
                mode,
            )
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

    pub async fn correct_batch_match(
        &self,
        batch: i64,
        item: i64,
        candidate: CandidateChoice,
    ) -> Result<(), String> {
        let id = &candidate.id;
        let valid = match candidate.provider {
            MediaProvider::Tmdb => {
                if !self.tmdb_configured() {
                    return Err("请先在「设置」中配置 TMDB API 凭据。".into());
                }
                id.parse::<u32>().is_ok_and(|id| id > 0)
            }
            MediaProvider::Wikidata => id
                .strip_prefix('Q')
                .is_some_and(|digits| digits.parse::<u64>().is_ok_and(|id| id > 0)),
            MediaProvider::Tvmaze => id.parse::<u64>().is_ok_and(|id| id > 0),
            MediaProvider::Bangumi => crate::providers::media::bangumi::id(id).is_some(),
        };
        if !valid {
            return Err("来源 ID 无效。".into());
        }
        self.require_store()
            .await?
            .correct_batch_match(batch, item, candidate)
            .await
            .map_err(|_| "无法更新条目匹配。".into())
            .and_then(|changed| {
                changed
                    .then_some(())
                    .ok_or("此条目当前无法纠正匹配。".into())
            })
    }

    pub async fn batch_row_result(&self, batch: i64, item: i64) -> Result<LookupResponse, String> {
        self.require_store()
            .await?
            .batch_row_result(batch, item)
            .await?
            .ok_or("此条目暂无可查看的结果。".into())
    }
}
