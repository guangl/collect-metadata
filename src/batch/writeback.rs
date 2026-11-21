use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    Collector,
    output::{WritebackJob, WritebackPreviewFile, emby_source, plan_writeback},
    storage::WritebackRow,
};

#[derive(Serialize)]
pub struct WritebackPreview {
    token: String,
    writable: usize,
    files: Vec<WritebackPreviewFile>,
}

impl Collector {
    pub async fn preview_writeback(
        &self,
        batch: i64,
        row: Option<i64>,
    ) -> Result<WritebackPreview, String> {
        self.plan_writeback(batch, row)
            .await
            .map(|(preview, _)| preview)
    }

    pub async fn queue_writeback(
        &self,
        batch: i64,
        row: Option<i64>,
        token: &str,
    ) -> Result<usize, String> {
        let (preview, plans) = self.plan_writeback(batch, row).await?;
        if token != preview.token || preview.writable == 0 {
            return Err("写回目标已变更或不可用，请重新预览。".into());
        }
        self.output_settings()
            .ok_or("输出设置不可用。")?
            .durable_owner()?;
        self.require_store()
            .await?
            .queue_writebacks(batch, &plans)
            .await
            .map_err(|_| "无法保存写回任务。")?
            .ok_or("批次状态已变更，请重新预览。".into())
    }

    async fn plan_writeback(
        &self,
        batch: i64,
        row: Option<i64>,
    ) -> Result<(WritebackPreview, Vec<(WritebackRow, WritebackJob)>), String> {
        let settings = self.output_settings().ok_or("输出设置不可用。")?;
        let owner = settings.owner();
        let paths = settings.media_paths();
        let rows = self
            .require_store()
            .await?
            .writeback_rows(batch, row, &owner)
            .await
            .map_err(|_| "无法读取已保存的采集结果。")?;
        if rows.is_empty() {
            return Err("当前进程没有可预览的写回条目。请检查提交批次的设备或刷新队列。".into());
        }
        tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            let mut plans = Vec::new();
            let mut writable = 0;
            for row in rows {
                let Some(data) = emby_source(&row.record) else {
                    continue;
                };
                let (entries, preview, count) =
                    plan_writeback(&paths, data, &row.record, &row.state);
                files.extend(preview);
                writable += count;
                if !entries.is_empty() {
                    let job = WritebackJob {
                        id: uuid::Uuid::new_v4().simple().to_string(),
                        owner: owner.clone(),
                        stage: row.stage.clone(),
                        files: entries,
                    };
                    plans.push((row, job));
                }
            }
            // Approval covers the selected snapshots, current mapping and observed file state.
            let approved = plans
                .iter()
                .map(|(row, job)| (row.id, row.snapshot, &row.state, &job.files))
                .collect::<Vec<_>>();
            let encoded = serde_json::to_vec(&(batch, row, owner, approved))
                .map_err(|_| "无法生成写回预览。")?;
            let token = Sha256::digest(encoded)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Ok((
                WritebackPreview {
                    token,
                    writable,
                    files,
                },
                plans,
            ))
        })
        .await
        .map_err(|_| "无法读取媒体目录。")?
    }
}
