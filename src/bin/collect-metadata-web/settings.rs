use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use collect_metadata::{Collector, EmbyInput, ErrorCode, OutputOptions};
use serde::Deserialize;

use super::{
    AppState,
    errors::{ApiError, api_error},
};

#[derive(Deserialize)]
struct CredentialInput {
    credential: Option<String>,
}

pub(super) fn routes() -> Router<Arc<Collector>> {
    Router::new()
        .route("/api/settings/ai", get(ai_status))
        .route("/api/settings/postgres", get(check_storage))
        .route("/api/settings/tmdb", get(tmdb_status).post(save_tmdb))
        .route("/api/settings/tmdb/test", post(test_tmdb))
        .route(
            "/api/settings/bangumi",
            get(bangumi_status).post(save_bangumi),
        )
        .route("/api/settings/bangumi/test", post(test_bangumi))
        .route(
            "/api/settings/output",
            get(output_settings).post(save_output_settings),
        )
        .route("/api/settings/emby", get(emby_settings).post(save_emby))
        .route("/api/settings/emby/test", post(test_emby))
        .route(
            "/api/settings/languages",
            get(language_settings).post(save_languages),
        )
}

async fn ai_status(State(collector): AppState) -> Json<bool> {
    Json(collector.ai_configured())
}

async fn check_storage(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    let cache = [(header::CACHE_CONTROL, "no-store")];
    if !collector.storage_configured() {
        return Ok((cache, Json("none")).into_response());
    }
    collector
        .check_storage()
        .await
        .map(|()| (cache, StatusCode::NO_CONTENT).into_response())
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::StorageCheckFailed,
                error,
            )
        })
}

async fn tmdb_status(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.tmdb_status()),
    )
}

async fn test_tmdb(
    State(collector): AppState,
    Json(input): Json<CredentialInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .test_tmdb(input.credential.as_deref())
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::TmdbConnectionFailed,
                error,
            )
        })
}

async fn save_tmdb(
    State(collector): AppState,
    Json(input): Json<CredentialInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .save_tmdb(input.credential.unwrap_or_default())
        .await
        .map(|status| ([(header::CACHE_CONTROL, "no-store")], Json(status)))
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::TmdbSettingsFailed,
                error,
            )
        })
}

async fn bangumi_status(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.bangumi_status()),
    )
}

async fn test_bangumi(
    State(collector): AppState,
    Json(input): Json<CredentialInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .test_bangumi(input.credential.as_deref())
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::BangumiConnectionFailed,
                error,
            )
        })
}

async fn save_bangumi(
    State(collector): AppState,
    Json(input): Json<CredentialInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .save_bangumi(input.credential.unwrap_or_default())
        .await
        .map(|status| ([(header::CACHE_CONTROL, "no-store")], Json(status)))
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::BangumiSettingsFailed,
                error,
            )
        })
}

async fn output_settings(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            collector
                .output_settings()
                .expect("web output settings")
                .view(),
        ),
    )
}

async fn save_output_settings(
    State(collector): AppState,
    Json(options): Json<OutputOptions>,
) -> Result<impl IntoResponse, ApiError> {
    let settings = Arc::clone(collector.output_settings().expect("web output settings"));
    let view = tokio::task::spawn_blocking(move || settings.save(options))
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::OutputSettingsFailed,
                "无法保存输出设置。",
            )
        })?
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::OutputSettingsFailed,
                error,
            )
        })?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view)))
}

async fn emby_settings(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.emby_settings().expect("web Emby settings").view()),
    )
}

async fn test_emby(
    State(collector): AppState,
    Json(input): Json<EmbyInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .test_emby(input)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::EmbyConnectionFailed,
                error,
            )
        })
}

async fn save_emby(
    State(collector): AppState,
    Json(input): Json<EmbyInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .save_emby(input)
        .await
        .map(|view| ([(header::CACHE_CONTROL, "no-store")], Json(view)))
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::EmbySettingsFailed,
                error,
            )
        })
}

async fn language_settings(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.language_settings().view()),
    )
}

async fn save_languages(
    State(collector): AppState,
    Json(languages): Json<Vec<String>>,
) -> Result<impl IntoResponse, ApiError> {
    let settings = Arc::clone(collector.language_settings());
    tokio::task::spawn_blocking(move || settings.save(languages))
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::LanguageSettingsFailed,
                "无法保存元数据语言。",
            )
        })?
        .map(Json)
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::LanguageSettingsFailed,
                error,
            )
        })
}
