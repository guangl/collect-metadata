use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use collect_metadata::{
    Collector, EmbyOverwriteInput, EmbyPreviewImage, EmbyScrapeScope, EmbyScrapeTarget,
    EmbySelectionInput, ErrorCode, diagnostics,
};
use serde_json::{Value, json};
use std::env;

use super::{
    AppState,
    errors::{ApiError, api_error},
};

pub(super) fn routes() -> Router<Arc<Collector>> {
    Router::new()
        .route("/api/emby/libraries", get(emby_libraries))
        .route("/api/emby/import", post(import_emby))
        .route("/api/emby/preview", post(preview_emby))
        .route("/api/emby/catalog", get(emby_catalog))
        .route("/api/emby/import-all", post(import_emby_catalog))
        .route("/api/emby/import-selection", post(import_emby_selection))
        .route("/api/emby/items/{id}/scrape", post(import_emby_item))
        .route("/api/emby/overwrite", post(overwrite_emby))
        .route("/api/emby/refresh", post(refresh_emby_metadata))
        .route(
            "/api/emby/episodes/{id}/overview",
            get(emby_episode_overview),
        )
        .route("/api/emby/series/{id}", get(emby_series_preview))
        .route(
            "/api/emby/libraries/{id}/missing-episode-overviews",
            get(emby_missing_episode_overviews),
        )
        .route("/api/emby/credits/{id}", get(emby_credits))
        .route("/api/emby/images/{id}/{image}", get(emby_preview_image))
        .route(
            "/api/emby/sync/{server}",
            post(sync_emby_item).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
}

async fn emby_libraries(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_libraries()
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyLibraryFailed, error))
}

async fn import_emby(
    State(collector): AppState,
    Json(library): Json<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .import_emby_library(&library)
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyImportFailed, error))
}

async fn preview_emby(
    State(collector): AppState,
    Json(library): Json<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .preview_emby_library(&library)
        .await
        .map(|works| ([(header::CACHE_CONTROL, "no-store")], Json(works)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyLibraryFailed, error))
}

async fn emby_catalog(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_catalog()
        .await
        .map(|catalog| ([(header::CACHE_CONTROL, "no-store")], Json(catalog)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyLibraryFailed, error))
}

async fn import_emby_catalog(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .import_emby_catalog()
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyImportFailed, error))
}

async fn import_emby_selection(
    State(collector): AppState,
    Json(input): Json<EmbySelectionInput>,
) -> Result<Json<i64>, ApiError> {
    collector
        .import_emby_selection(input)
        .await
        .map(Json)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyImportFailed, error))
}

async fn import_emby_item(
    State(collector): AppState,
    Path(id): Path<String>,
    Json(scope): Json<EmbyScrapeScope>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .import_emby_item(&id, scope)
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyImportFailed, error))
}

async fn overwrite_emby(
    State(collector): AppState,
    Json(input): Json<EmbyOverwriteInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .overwrite_emby(input)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::EmbyOverwriteFailed,
                error,
            )
        })
}

async fn refresh_emby_metadata(
    State(collector): AppState,
    Json(target): Json<EmbyScrapeTarget>,
) -> Result<StatusCode, ApiError> {
    collector
        .refresh_emby_metadata(target)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyRefreshFailed, error))
}

async fn emby_episode_overview(
    State(collector): AppState,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_episode_overview(&id)
        .await
        .map(|overview| ([(header::CACHE_CONTROL, "no-store")], Json(overview)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyDetailsFailed, error))
}

async fn emby_series_preview(
    State(collector): AppState,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_series_preview(&id)
        .await
        .map(|series| ([(header::CACHE_CONTROL, "no-store")], Json(series)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyDetailsFailed, error))
}

async fn emby_missing_episode_overviews(
    State(collector): AppState,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_missing_episode_overviews(&id)
        .await
        .map(Json)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyDetailsFailed, error))
}

async fn emby_credits(
    State(collector): AppState,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_credits(&id)
        .await
        .map(|people| ([(header::CACHE_CONTROL, "no-store")], Json(people)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, ErrorCode::EmbyDetailsFailed, error))
}

async fn emby_preview_image(
    State(collector): AppState,
    Path((id, image)): Path<(String, EmbyPreviewImage)>,
) -> impl IntoResponse {
    let Some((bytes, extension)) = collector.emby_preview_image(&id, image).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match extension {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

async fn sync_emby_item(
    State(collector): AppState,
    Path(server): Path<String>,
    headers: HeaderMap,
    Json(doc): Json<Value>,
) -> Result<StatusCode, ApiError> {
    let token = env::var("EMBY_SYNC_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            diagnostics::warn("emby_sync_rejected", json!({"reason": "not_configured"}));
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::EmbySyncFailed,
                "未启用插件同步。",
            )
        })?;
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        != Some(token.as_str())
    {
        diagnostics::warn("emby_sync_rejected", json!({"reason": "unauthorized"}));
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            ErrorCode::EmbySyncFailed,
            "同步令牌无效。请核对插件和 Web 使用的同步令牌。",
        ));
    }
    let id = doc["Id"].as_str().unwrap_or_default();
    if server.is_empty()
        || server.len() > 64
        || !server
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || id.parse::<u64>().ok().filter(|n| *n > 0).is_none()
        || !matches!(
            doc["Type"].as_str(),
            Some("Movie" | "Series" | "Season" | "Episode" | "Video")
        )
    {
        diagnostics::warn("emby_sync_rejected", json!({"reason": "invalid_item"}));
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::EmbySyncFailed,
            "同步请求中的 Emby 服务器或媒体条目无效。请到 Emby 日志中核对条目。",
        ));
    }
    let preference = headers
        .get("x-emby-metadata-language")
        .and_then(|value| value.to_str().ok());
    collector
        .upsert_emby_item(&server, id, &doc, preference)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::EmbySyncFailed,
                error,
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}
