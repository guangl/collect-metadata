use std::{env, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use collect_metadata::{
    BatchAction, BatchItemInput, CandidateChoice, Collector, EmbyInput, EmbySettings, HistoryItem,
    LanguageSettings, LookupResponse, MediaKind, MediaProvider, OutputOptions, OutputSettings,
    TmdbSettings, browse_mounted_folders, preview_folder, scan_mounted_folder,
};
use serde::Deserialize;
use serde_json::{Value, json};

type AppState = State<Arc<Collector>>;
type ApiError = (StatusCode, Json<Value>);

#[derive(Deserialize)]
struct LookupInput {
    query: String,
}

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<i64>,
    kind: Option<MediaKind>,
    language: Option<String>,
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
struct BatchBefore {
    before: Option<i64>,
}

#[derive(Deserialize)]
struct MediaFolderQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct TmdbInput {
    credential: Option<String>,
}

fn tmdb_config_file() -> PathBuf {
    if let Some(path) = env::var_os("TMDB_CONFIG_FILE").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    env::var_os("XDG_CONFIG_HOME")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("collect-metadata")
        .join("tmdb-token")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = tmdb_config_file();
    let output_root = env::var_os("SCRAPE_OUTPUT_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config.with_file_name("output"));
    let output = OutputSettings::load(
        config.with_file_name("output-settings.json"),
        Some(output_root),
    )?;
    let emby = Arc::new(EmbySettings::load(
        config.with_file_name("emby-settings.json"),
    )?);
    let languages = Arc::new(LanguageSettings::load(Some(
        config.with_file_name("metadata-languages.json"),
    ))?);
    let tmdb = TmdbSettings::from_file(config)?;
    let collector = Collector::from_environment(None)?
        .with_tmdb_settings(Arc::new(tmdb))
        .with_output_settings(Arc::new(output))
        .with_emby_settings(emby)
        .with_language_settings(languages);
    if !collector.storage_configured() {
        return Err("请通过 DATABASE_URL 或 PGHOST 配置 PostgreSQL。".into());
    }
    collector.init_storage().await?;
    let collector = Arc::new(collector);
    let app = router(Arc::clone(&collector));
    let bind = env::var("APP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("Collect Metadata listening on {}", listener.local_addr()?);
    tokio::spawn(collector.run_batch_worker());
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(collector: Arc<Collector>) -> Router {
    Router::new()
        .route("/", get(index))
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../dist/style.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../dist/app.js"),
                )
            }),
        )
        .route(
            "/icon.png",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    include_bytes!("../../dist/icon.png").as_slice(),
                )
            }),
        )
        .route(
            "/tmdb-logo.svg",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/svg+xml")],
                    include_str!("../../dist/tmdb-logo.svg"),
                )
            }),
        )
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .route("/api/settings/postgres", get(check_storage))
        .route("/api/settings/tmdb", get(tmdb_status).post(save_tmdb))
        .route("/api/settings/tmdb/test", post(test_tmdb))
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
        .route("/api/emby/libraries", get(emby_libraries))
        .route("/api/emby/import", post(import_emby))
        .route("/api/emby/preview", post(preview_emby))
        .route(
            "/api/emby/sync/{server}",
            post(sync_emby_item).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/lookups", post(lookup))
        .route("/api/search", post(search_media))
        .route("/api/works", post(collect_media))
        .route("/api/history", get(history))
        .route("/api/history/languages", get(history_languages))
        .route("/api/history/{id}", get(load_snapshot))
        .route("/api/batches", get(list_batches).post(create_batch))
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
        .with_state(collector)
}

#[cfg(test)]
#[path = "../tests/web.rs"]
mod tests;

async fn index() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self' https:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(include_str!("../../dist/index.html")),
    )
}

async fn tmdb_status(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.tmdb_status()),
    )
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
                "无法保存元数据语言。".into(),
            )
        })?
        .map(Json)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn emby_settings(State(collector): AppState) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(collector.emby_settings().expect("web Emby settings").view()),
    )
}

async fn emby_libraries(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .emby_libraries()
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
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
        .ok_or_else(|| api_error(StatusCode::SERVICE_UNAVAILABLE, "未启用插件同步。".into()))?;
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        != Some(token.as_str())
    {
        return Err(api_error(StatusCode::UNAUTHORIZED, "同步令牌无效。".into()));
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
            Some("Movie" | "Series" | "Season" | "Episode")
        )
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "无效的 Emby 服务器或媒体条目。".into(),
        ));
    }
    let preference = headers
        .get("x-emby-metadata-language")
        .and_then(|value| value.to_str().ok());
    collector
        .upsert_emby_item(&server, id, &doc, preference)
        .await
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn preview_emby(
    State(collector): AppState,
    Json(library): Json<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .preview_emby_library(&library)
        .await
        .map(|works| ([(header::CACHE_CONTROL, "no-store")], Json(works)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn import_emby(
    State(collector): AppState,
    Json(library): Json<String>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .import_emby_library(&library)
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn test_emby(
    State(collector): AppState,
    Json(input): Json<EmbyInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .test_emby(input)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn save_emby(
    State(collector): AppState,
    Json(input): Json<EmbyInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .save_emby(input)
        .await
        .map(|view| ([(header::CACHE_CONTROL, "no-store")], Json(view)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
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
                "无法保存输出设置。".into(),
            )
        })?
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view)))
}

async fn check_storage(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .check_storage()
        .await
        .map(|()| {
            (
                [(header::CACHE_CONTROL, "no-store")],
                StatusCode::NO_CONTENT,
            )
        })
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn test_tmdb(
    State(collector): AppState,
    Json(input): Json<TmdbInput>,
) -> Result<StatusCode, ApiError> {
    collector
        .test_tmdb(input.credential.as_deref())
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn save_tmdb(
    State(collector): AppState,
    Json(input): Json<TmdbInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .save_tmdb(input.credential.unwrap_or_default())
        .await
        .map(|status| ([(header::CACHE_CONTROL, "no-store")], Json(status)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn lookup(
    State(collector): AppState,
    Json(input): Json<LookupInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .lookup(&input.query)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn search_media(
    State(collector): AppState,
    Json(input): Json<SearchInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .search_media(input.kind, &input.query)
        .await
        .map(|results| ([(header::CACHE_CONTROL, "no-store")], Json(results)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn collect_media(
    State(collector): AppState,
    Json(input): Json<MediaInput>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .collect_media(input.kind, input.provider, &input.id)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn history_languages(State(collector): AppState) -> Result<impl IntoResponse, ApiError> {
    collector
        .history_languages()
        .await
        .map(|languages| ([(header::CACHE_CONTROL, "no-store")], Json(languages)))
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn history(
    State(collector): AppState,
    Query(query): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .history(query.before, query.kind, query.language.as_deref())
        .await
        .map(|items: Vec<HistoryItem>| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn load_snapshot(
    State(collector): AppState,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .load(id)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::NOT_FOUND, error))
}

fn lookup_response(value: LookupResponse) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}

fn api_error(status: StatusCode, error: String) -> ApiError {
    (status, Json(json!({ "error": error })))
}

async fn create_batch(
    State(collector): AppState,
    Json(items): Json<Vec<BatchItemInput>>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .create_batch(items)
        .await
        .map(|id| (StatusCode::CREATED, Json(id)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn browse_media_folders(
    Query(input): Query<MediaFolderQuery>,
) -> Result<impl IntoResponse, ApiError> {
    browse_mounted_folders(input.path)
        .await
        .map(|folders| ([(header::CACHE_CONTROL, "no-store")], Json(folders)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn preview_mounted_folder(Json(path): Json<String>) -> Result<impl IntoResponse, ApiError> {
    scan_mounted_folder(path)
        .await
        .map(|selection| ([(header::CACHE_CONTROL, "no-store")], Json(selection)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn scan_media_folder(Json(paths): Json<Vec<String>>) -> Result<impl IntoResponse, ApiError> {
    tokio::task::spawn_blocking(move || preview_folder(paths))
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "无法扫描所选文件夹。".into(),
            )
        })?
        .map(|preview| ([(header::CACHE_CONTROL, "no-store")], Json(preview)))
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
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
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))
}

async fn list_batches(
    State(collector): AppState,
    Query(input): Query<BatchBefore>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .list_batches(input.before)
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn batch_rows(
    State(collector): AppState,
    Path(batch): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_rows(batch)
        .await
        .map(|items| ([(header::CACHE_CONTROL, "no-store")], Json(items)))
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn batch_view(
    State(collector): AppState,
    Path(batch): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_view(batch)
        .await
        .map(|view| ([(header::CACHE_CONTROL, "no-store")], Json(view)))
        .map_err(|error| api_error(StatusCode::SERVICE_UNAVAILABLE, error))
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
        .map_err(|error| api_error(StatusCode::CONFLICT, error))
}

async fn batch_result(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_row_result(batch, row)
        .await
        .map(lookup_response)
        .map_err(|error| api_error(StatusCode::NOT_FOUND, error))
}

async fn batch_files(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    collector
        .batch_files(batch, row)
        .await
        .map(|files| ([(header::CACHE_CONTROL, "no-store")], Json(files)))
        .map_err(|error| api_error(StatusCode::NOT_FOUND, error))
}

async fn retry_batch_row(
    State(collector): AppState,
    Path((batch, row)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    collector
        .retry_batch_row(batch, row)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| api_error(StatusCode::CONFLICT, error))
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
        .map_err(|error| api_error(StatusCode::CONFLICT, error))
}
