#[path = "collect-metadata-web/batches.rs"]
mod batches;
#[path = "collect-metadata-web/emby.rs"]
mod emby;
#[path = "collect-metadata-web/errors.rs"]
mod errors;
#[path = "collect-metadata-web/metadata.rs"]
mod metadata;
#[path = "collect-metadata-web/settings.rs"]
mod settings;

#[cfg(test)]
#[path = "../tests/web.rs"]
mod tests;

use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    middleware,
    response::{Html, IntoResponse},
    routing::get,
};
use collect_metadata::{
    AutoScrape, BangumiSettings, Collector, EmbySettings, LanguageSettings, OutputSettings,
    TmdbSettings, diagnostics,
};
use serde_json::json;

type AppState = State<Arc<Collector>>;

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
    diagnostics::info(
        "app_starting",
        json!({"mode": "web", "version": env!("CARGO_PKG_VERSION")}),
    );
    let result = serve().await;
    if result.is_err() {
        diagnostics::error("web_stopped_with_error", json!({}));
    }
    result
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let config = tmdb_config_file();
    let output_root = env::var_os("SCRAPE_OUTPUT_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config.with_file_name("output"));
    let output = diagnostics::startup_step(
        "output_config",
        OutputSettings::load(
            config.with_file_name("output-settings.json"),
            Some(output_root),
        ),
    )?
    .with_emby_root(env::var_os("EMBY_WRITEBACK_ROOT").map(PathBuf::from));
    let emby = Arc::new(diagnostics::startup_step(
        "emby_config",
        EmbySettings::load(config.with_file_name("emby-settings.json")),
    )?);
    let languages = Arc::new(diagnostics::startup_step(
        "language_config",
        LanguageSettings::load(Some(config.with_file_name("metadata-languages.json"))),
    )?);
    let ai_file = config.with_file_name("ai.json");
    let bangumi = diagnostics::startup_step(
        "bangumi_config",
        BangumiSettings::from_file(config.with_file_name("bangumi-token")),
    )?;
    let tmdb = diagnostics::startup_step("tmdb_config", TmdbSettings::from_file(config))?;
    let collector =
        diagnostics::startup_step("collector_config", Collector::from_environment(None))?;
    let collector =
        diagnostics::startup_step("ai_config", collector.with_ai_config_file(&ai_file))?
            .with_tmdb_settings(Arc::new(tmdb))
            .with_bangumi_settings(Arc::new(bangumi))
            .with_output_settings(Arc::new(output))
            .with_emby_settings(emby)
            .with_language_settings(languages);
    diagnostics::startup_step("database_init", collector.init_storage().await)?;
    let collector = Arc::new(collector);
    let app = router(Arc::clone(&collector));
    let bind = env::var("APP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .inspect_err(|error| {
            diagnostics::error(
                "web_bind_failed",
                json!({"reason": format!("{:?}", error.kind())}),
            );
        })?;
    diagnostics::info(
        "web_listening",
        json!({"port": listener.local_addr()?.port()}),
    );
    if let Some(automatic) =
        diagnostics::startup_step("auto_scrape_config", AutoScrape::from_environment())?
    {
        tokio::spawn(automatic.run(Arc::clone(&collector)));
    }
    tokio::spawn(collector.run_batch_worker());
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(collector: Arc<Collector>) -> Router {
    let was_ready = Arc::new(AtomicBool::new(true));
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
        .route("/readyz", get(move |state| readiness(state, was_ready)))
        .merge(settings::routes())
        .merge(emby::routes())
        .merge(metadata::routes())
        .merge(batches::routes())
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn(errors::api_diagnostics))
        .with_state(collector)
}

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

async fn readiness(State(collector): AppState, was_ready: Arc<AtomicBool>) -> impl IntoResponse {
    let reason = if collector.storage_configured() {
        match tokio::time::timeout(Duration::from_secs(2), collector.check_storage()).await {
            Ok(Ok(())) => None,
            Ok(Err(_)) => Some("postgres"),
            Err(_) => Some("timeout"),
        }
    } else {
        None
    };
    let previous = was_ready.swap(reason.is_none(), Ordering::Relaxed);
    let status = if let Some(reason) = reason {
        if previous {
            diagnostics::warn("web_readiness_failed", json!({"reason": reason}));
        }
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        if !previous {
            diagnostics::info("web_readiness_recovered", json!({}));
        }
        StatusCode::NO_CONTENT
    };
    ([(header::CACHE_CONTROL, "no-store")], status)
}
