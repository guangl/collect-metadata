use crate::{HistoryItem, MediaKind, diagnostics, storage::Store};

use super::{Collector, LookupResponse, localization};

impl Collector {
    pub async fn upsert_emby_item(
        &self,
        server: &str,
        id: &str,
        document: &serde_json::Value,
        preferred_language: Option<&str>,
    ) -> Result<(), String> {
        let store = self.require_store().await?;
        let previous = store.emby_item(server, id).await?.unwrap_or_default();
        let document = localization::records::sync_document(
            &self.languages,
            &self.client,
            document,
            &previous,
            preferred_language,
        )
        .await;
        store.upsert_emby_item(server, id, &document).await
    }

    async fn store(&self) -> Result<Option<&Store>, String> {
        let Some(database) = &self.database else {
            return Ok(None);
        };
        self.store
            .get_or_try_init(|| Store::connect(database))
            .await
            .map(Some)
    }

    pub(crate) async fn require_store(&self) -> Result<&Store, String> {
        self.store()
            .await?
            .ok_or("批量后台采集需要连接 PostgreSQL。".into())
    }

    pub async fn init_storage(&self) -> Result<(), String> {
        self.store().await.map(|_| ())
    }

    pub async fn check_storage(&self) -> Result<(), String> {
        self.require_store().await?.check_connection().await
    }

    pub fn storage_configured(&self) -> bool {
        self.database.is_some()
    }

    pub(crate) async fn save_lookup(&self, mut response: LookupResponse) -> LookupResponse {
        if response.has_fields() {
            match self.store().await {
                Ok(Some(store)) => match store.save(response.code(), &response.record).await {
                    Ok(item) => response.saved = Some(item),
                    Err(error) => response.storage_error = Some(diagnostics::with_reference(error)),
                },
                Ok(None) => {}
                Err(error) => response.storage_error = Some(diagnostics::with_reference(error)),
            }
        }
        response
    }

    pub async fn history(
        &self,
        before: Option<i64>,
        kind: Option<MediaKind>,
        language: Option<&str>,
    ) -> Result<Vec<HistoryItem>, String> {
        if let Some(code) = language
            && code != "und"
            && (!matches!(code.len(), 2 | 3) || !code.bytes().all(|byte| byte.is_ascii_lowercase()))
        {
            return Err("语言代码无效。".into());
        }
        match self.store().await? {
            Some(store) => store.history(before, kind, language).await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn history_languages(&self) -> Result<Vec<String>, String> {
        match self.store().await? {
            Some(store) => store.history_languages().await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn load(&self, id: i64) -> Result<LookupResponse, String> {
        let store = self.store().await?.ok_or("未连接 PostgreSQL。")?;
        let (saved, record) = store.load(id).await?;
        Ok(LookupResponse {
            record,
            saved: Some(saved),
            storage_error: None,
        })
    }
}
