use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use collect_metadata::{
    AiBatchInput, BatchAction, BatchItemInput, CandidateChoice, Collector, ErrorCode,
    browse_mounted_folders, preview_folder, scan_mounted_folder,
};
use serde::Deserialize;

use super::metadata::lookup_response;
use super::{
    AppState,
    errors::{ApiError, api_error},
};

#[derive(Deserialize)]
struct BatchBefore {
    before: Option<i64>,
}

#[derive(Deserialize)]
struct MediaFolderQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct WritebackSelection {
    row: Option<i64>,
}

#[derive(Deserialize)]
struct WritebackConfirmation {
    row: Option<i64>,
    token: String,
}

pub(super) fn routes() -> Router<Arc<Collector>> {
    Router::new()
        .route("/api/batches", get(list_batches).post(create_batch))
        .route("/api/batches/ai", post(create_ai_batch))
        .route(
            "/api/batches/{batch}/rows/{row}/match",
            post(correct_batch_match),
        )
        .route(
            "/api/media-folders",
            get(browse_media_folders).post(preview_mounted_folder),
        )
        .route(
            "/api/folder-preview",
            post(scan_media_folder).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/api/folder-batches",
            post(create_folder_batch).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/batches/{batch}", get(batch_rows))
        .route("/api/batches/{batch}/state", get(batch_view))
        .route("/api/batches/{batch}/control", post(control_batch))
        .route(
            "/api/batches/{batch}/writeback",
            get(preview_writeback).post(queue_writeback),
        )
        .route("/api/batches/{batch}/rows/{row}/result", get(batch_result))
        .route("/api/batches/{batch}/rows/{row}/files", get(batch_files))
        .route(
            "/api/batches/{batch}/rows/{row}/retry",
            post(retry_batch_row),
        )
        .route(
            "/api/batches/{batch}/rows/{row}/candidate",
            post(choose_batch_candidate),
        )
}

async fn list_batches(
    State(collector): AppState,
    Query(input): Query<BatchBefore>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .list_batches(input.before)
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::BatchReadFailed,
                error,
            )
        })
}

async fn create_batch(
    State(collector): AppState,
    Json(items): Json<Vec<BatchItemInput>>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .create_batch(items)
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::BatchCreateFailed, error))
}

async fn create_ai_batch(
    State(collector): AppState,
    Json(input): Json<AiBatchInput>,
) -> Result<Json<i64>, ApiError> {
    collector
        .create_ai_batch(input)
        .await
        .map(Json)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::AiBatchCreateFailed,
                error,
            )
        })
}

async fn correct_batch_match(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
    Json(candidate): Json<CandidateChoice>,
) -> Result<StatusCode, ApiError> {
    collector
        .correct_batch_match(batch, row, candidate)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::BatchMatchFailed, error))
}

async fn browse_media_folders(
    Query(input): Query<MediaFolderQuery>,
) -> Result<impl IntoResponse, ApiError> {
    browse_mounted_folders(input.path)
        .await
        .map(|folders| ([(header::CACHE_CONTROL, "no-store")], Json(folders)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::FolderScanFailed, error))
}

async fn preview_mounted_folder(Json(path): Json<String>) -> Result<impl IntoResponse, ApiError> {
    scan_mounted_folder(path)
        .await
        .map(|selection| ([(header::CACHE_CONTROL, "no-store")], Json(selection)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::FolderScanFailed, error))
}

async fn scan_media_folder(Json(paths): Json<Vec<String>>) -> Result<impl IntoResponse, ApiError> {
    tokio::task::spawn_blocking(move || preview_folder(paths))
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::FolderScanFailed,
                "无法扫描所选文件夹。",
            )
        })?
        .map(|preview| ([(header::CACHE_CONTROL, "no-store")], Json(preview)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::FolderScanFailed, error))
}

async fn create_folder_batch(
    State(collector): AppState,
    Json(paths): Json<Vec<String>>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .create_folder_batch(paths)
        .await
        .map(|result| {
            (
                if result.id.is_some() {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(result),
            )
        })
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::BatchCreateFailed, error))
}

async fn batch_rows(
    State(collector): AppState,
    Path(batch): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_rows(batch)
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::BatchReadFailed,
                error,
            )
        })
}

async fn batch_view(
    State(collector): AppState,
    Path(batch): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_view(batch)
        .await
        .map(|view| ([(header::CACHE_CONTROL, "no-store")], Json(view)))
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::BatchReadFailed,
                error,
            )
        })
}

async fn control_batch(
    State(collector): AppState,
    Path(batch): Path<i64>,
    Json(action): Json<BatchAction>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .control_batch(batch, action)
        .await
        .map(Json)
        .map_err(|error| api_error(StatusCode::CONFLICT, ErrorCode::BatchUpdateFailed, error))
}

async fn preview_writeback(
    State(collector): AppState,
    Path(batch): Path<i64>,
    Query(selection): Query<WritebackSelection>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .preview_writeback(batch, selection.row)
        .await
        .map(|preview| ([(header::CACHE_CONTROL, "no-store")], Json(preview)))
        .map_err(|error| {
            api_error(
                StatusCode::CONFLICT,
                ErrorCode::WritebackPreviewFailed,
                error,
            )
        })
}

async fn queue_writeback(
    State(collector): AppState,
    Path(batch): Path<i64>,
    Json(confirmation): Json<WritebackConfirmation>,
) -> Result<Json<usize>, ApiError> {
    collector
        .queue_writeback(batch, confirmation.row, &confirmation.token)
        .await
        .map(Json)
        .map_err(|error| api_error(StatusCode::CONFLICT, ErrorCode::WritebackQueueFailed, error))
}

async fn batch_result(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_row_result(batch, row)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::NOT_FOUND, ErrorCode::BatchReadFailed, error))
}

async fn batch_files(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_files(batch, row)
        .await
        .map(|files| ([(header::CACHE_CONTROL, "no-store")], Json(files)))
        .map_err(|error| api_error(StatusCode::NOT_FOUND, ErrorCode::BatchReadFailed, error))
}

async fn retry_batch_row(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    collector
        .retry_batch_row(batch, row)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::CONFLICT, ErrorCode::BatchUpdateFailed, error))
}

async fn choose_batch_candidate(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
    Json(candidate): Json<CandidateChoice>,
) -> Result<StatusCode, ApiError> {
    collector
        .select_batch_candidate(batch, row, candidate)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::CONFLICT, ErrorCode::BatchMatchFailed, error))
}
