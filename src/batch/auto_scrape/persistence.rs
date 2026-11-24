use std::future::Future;

use jiff::civil::Date;
use serde_json::Value;

use super::{AutoScrape, Event, Outcome};
use crate::{Collector, LookupResponse};

pub(super) trait ScrapePersistence: Sync {
    fn ensure_available(&self) -> impl Future<Output = Result<(), String>> + Send;

    fn verified_bangumi_season(
        &self,
        server: &str,
        series: &str,
        season: i32,
    ) -> impl Future<Output = Result<Option<Vec<u32>>, String>> + Send;

    fn archived_bangumi_episodes(
        &self,
        subjects: &[u32],
    ) -> impl Future<Output = Result<Vec<Value>, String>> + Send;

    fn verified_episode_airdate(
        &self,
        server: &str,
        item: &Value,
        bangumi_id: u32,
        local: Date,
        official: Date,
    ) -> impl Future<Output = Result<bool, String>> + Send;

    fn save_emby_item(
        &self,
        server: &str,
        item: &str,
        document: &Value,
    ) -> impl Future<Output = Result<(), String>> + Send;

    fn save_snapshot(&self, result: LookupResponse) -> impl Future<Output = Option<String>> + Send;
}

impl AutoScrape {
    pub(super) async fn process(
        &self,
        collector: &Collector,
        event: &Event,
    ) -> Result<Outcome, String> {
        self.process_with(collector, event, collector).await
    }
}

impl ScrapePersistence for Collector {
    async fn ensure_available(&self) -> Result<(), String> {
        self.require_store().await.map(|_| ())
    }

    async fn verified_bangumi_season(
        &self,
        server: &str,
        series: &str,
        season: i32,
    ) -> Result<Option<Vec<u32>>, String> {
        self.require_store()
            .await?
            .verified_bangumi_season(server, series, season)
            .await
    }

    async fn archived_bangumi_episodes(&self, subjects: &[u32]) -> Result<Vec<Value>, String> {
        self.require_store()
            .await?
            .archived_bangumi_episodes(subjects)
            .await
    }

    async fn verified_episode_airdate(
        &self,
        server: &str,
        item: &Value,
        bangumi_id: u32,
        local: Date,
        official: Date,
    ) -> Result<bool, String> {
        self.require_store()
            .await?
            .verified_episode_airdate(server, item, bangumi_id, local, official)
            .await
    }

    async fn save_emby_item(
        &self,
        server: &str,
        item: &str,
        document: &Value,
    ) -> Result<(), String> {
        self.upsert_emby_item(server, item, document, None).await
    }

    async fn save_snapshot(&self, result: LookupResponse) -> Option<String> {
        self.save_lookup(result).await.storage_error
    }
}
