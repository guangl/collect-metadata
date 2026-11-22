use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use collect_metadata::{
    Collector, ErrorCode, HistoryItem, LookupResponse, MediaKind, MediaProvider,
};
use serde::Deserialize;

use super::{
    AppState,
    errors::{ApiError, api_error},
};

#[derive(Deserialize)]
struct LookupInput {
    query: String,
}

#[derive(Deserialize)]
struct SearchInput {
    kind: MediaKind,
    query: String,
}

#[derive(Deserialize)]
struct MediaInput {
    kind: MediaKind,
    provider: MediaProvider,
    id: String,
}

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<i64>,
    kind: Option<MediaKind>,
    language: Option<String>,
}

pub(super) fn routes() -> Router<Arc<Collector>> {
    Router::new()
        .route("/api/lookups", post(lookup))
        .route("/api/search", post(search_media))
        .route("/api/works", post(collect_media))
        .route("/api/history", get(history))
        .route("/api/history/languages", get(history_languages))
        .route("/api/history/clear", post(clear_history))
        .route("/api/history/{id}", get(load_snapshot))
}

async fn lookup(
    State(collector): AppState,
    Json(input): Json<LookupInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .lookup(&input.query)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::MediaLookupFailed, error))
}

async fn search_media(
    State(collector): AppState,
    Json(input): Json<SearchInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .search_media(input.kind, &input.query)
        .await
        .map(|results| ([(header::CACHE_CONTROL, "no-store")], Json(results)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::MediaSearchFailed, error))
}

async fn collect_media(
    State(collector): AppState,
    Json(input): Json<MediaInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .collect_media(input.kind, input.provider, &input.id)
        .await
        .map(lookup_response)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::MediaCollectionFailed,
                error,
            )
        })
}

async fn history(
    State(collector): AppState,
    Query(query): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .history(query.before, query.kind, query.language.as_deref())
        .await
        .map(|items: Vec<HistoryItem>| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::HistoryReadFailed,
                error,
            )
        })
}

async fn history_languages(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .history_languages()
        .await
        .map(|languages| ([(header::CACHE_CONTROL, "no-store")], Json(languages)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::HistoryReadFailed,
                error,
            )
        })
}

async fn clear_history(
    State(collector): AppState,
    Json(()): Json<()>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .clear_history()
        .await
        .map(|count| ([(header::CACHE_CONTROL, "no-store")], Json(count)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::HistoryClearFailed,
                error,
            )
        })
}

async fn load_snapshot(
    State(collector): AppState,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .load(id)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::NOT_FOUND, ErrorCode::HistoryReadFailed, error))
}

pub(super) fn lookup_response(value: LookupResponse) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}
