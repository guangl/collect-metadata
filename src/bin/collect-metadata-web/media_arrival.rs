use std::{env, io, path::PathBuf, sync::Arc, time::SystemTime};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use collect_metadata::{Collector, diagnostics};
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(test)]
#[path = "../../tests/media_arrival.rs"]
mod tests;

#[derive(Clone)]
struct MediaArrival {
    directory: PathBuf,
    authorization: String,
    collector: Arc<Collector>,
}

pub(super) async fn start(collector: Arc<Collector>) -> io::Result<()> {
    let Some(directory) = env::var_os("MEDIA_ARRIVAL_DIRECTORY").map(PathBuf::from) else {
        return Ok(());
    };
    let router = router(directory, collector).await?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:7099").await?;
    tokio::spawn(async move {
        if axum::serve(listener, router).await.is_err() {
            diagnostics::error("media_arrival_listener_failed", json!({}));
        }
    });
    Ok(())
}

async fn router(directory: PathBuf, collector: Arc<Collector>) -> io::Result<Router> {
    let token = tokio::fs::read_to_string(directory.join("token")).await?;
    let token = token.trim();
    if token.len() < 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid media repair token",
        ));
    }
    Ok(Router::new()
        .route("/status.json", get(status))
        .route("/healthz", get(health))
        .route("/repair", post(repair))
        .with_state(MediaArrival {
            directory,
            authorization: format!("Bearer {token}"),
            collector,
        }))
}

impl MediaArrival {
    async fn snapshot(&self) -> Value {
        let path = self.directory.join("status.json");
        let data = tokio::fs::read(&path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object);
        let Some(mut snapshot) = data else {
            return json!({"state": "unknown", "state_label": "没有运行记录", "age_text": "从未运行"});
        };
        let age = tokio::fs::metadata(&path)
            .await
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if let Some(age) = age {
            let seconds = age.as_secs();
            snapshot["age_seconds"] = json!(seconds);
            snapshot["age_text"] = json!(if seconds < 90 {
                format!("{seconds} 秒前")
            } else if seconds < 5400 {
                format!("{} 分钟前", seconds / 60)
            } else {
                format!("{:.1} 小时前", age.as_secs_f64() / 3600.0)
            });
            if seconds > 600 {
                snapshot["state"] = json!("stale");
                snapshot["state_label"] = json!("已停滞");
            }
        } else {
            snapshot["state"] = json!("unknown");
        }
        snapshot
    }
}

async fn status(State(state): State<MediaArrival>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.snapshot().await),
    )
}

async fn health(State(state): State<MediaArrival>) -> impl IntoResponse {
    let snapshot = state.snapshot().await;
    let ok = matches!(snapshot["state"].as_str(), Some("ok" | "warn"));
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok": ok, "state": snapshot["state"], "age_text": snapshot["age_text"]})),
    )
}

#[derive(Deserialize)]
struct Repair {
    original: Value,
    field: String,
    text: String,
}

async fn repair(
    State(state): State<MediaArrival>,
    headers: HeaderMap,
    Json(input): Json<Repair>,
) -> (StatusCode, Json<Value>) {
    if !headers.get(header::AUTHORIZATION).is_some_and(|value| {
        let supplied = value.as_bytes();
        let expected = state.authorization.as_bytes();
        supplied.len() == expected.len()
            && supplied
                .iter()
                .zip(expected)
                .fold(0, |diff, (a, b)| diff | (a ^ b))
                == 0
    }) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "UNAUTHORIZED"})),
        );
    }
    match state
        .collector
        .repair_anime_metadata(&input.original, &input.field, &input.text)
        .await
    {
        Ok(written) => (StatusCode::OK, Json(json!({"written": written}))),
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "EMBY_REPAIR_RETRY"})),
        ),
    }
}
