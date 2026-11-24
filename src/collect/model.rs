use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::{HistoryItem, MediaKind};

#[derive(Serialize)]
pub struct EmbyEpisodeOverview {
    pub(crate) overview: Option<String>,
    pub(crate) source: Option<String>,
}

#[derive(Serialize)]
pub struct LookupResponse {
    pub(crate) record: Value,
    pub(crate) saved: Option<HistoryItem>,
    pub(crate) storage_error: Option<String>,
}

impl LookupResponse {
    pub(crate) fn has_fields(&self) -> bool {
        self.record["fields"]
            .as_object()
            .is_some_and(|fields| !fields.is_empty())
    }

    pub(crate) fn code(&self) -> &str {
        self.record["work_id"]
            .as_str()
            .or_else(|| self.record["fields"]["dvd_id"].as_str())
            .or_else(|| self.record["fields"]["content_id"].as_str())
            .or_else(|| self.record["query"].as_str())
            .expect("lookup records have a query")
    }
}

pub(crate) type Fields = BTreeMap<&'static str, Value>;

#[derive(Serialize)]
pub(super) struct Lookup<'a> {
    pub(super) schema_version: u32,
    pub(super) media_type: MediaKind,
    pub(super) query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) work_id: Option<&'a str>,
    pub(super) fields: Fields,
    pub(super) field_sources: BTreeMap<&'static str, &'static str>,
    pub(super) sources: Vec<Source>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Source {
    pub(crate) provider: &'static str,
    pub(crate) url: String,
    pub(crate) status: &'static str,
    pub(crate) note: Option<String>,
    pub(crate) fields: Fields,
    pub(crate) data: Value,
}
