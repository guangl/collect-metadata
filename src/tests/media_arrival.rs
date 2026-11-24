use std::{fs, sync::Mutex, time::Duration};

use collect_metadata::EmbySettings;

use super::*;

const TOKEN: &str = "fixture-media-arrival-token-00000000";

async fn request(
    address: &str,
    path: &str,
    repair: Option<Value>,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{address}{path}");
    let mut request = match repair {
        Some(repair) => client.post(url).json(&repair),
        None => client.get(url),
    };
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    if path != "/repair" {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    (status, response.json().await.unwrap())
}

#[tokio::test]
async fn health_distinguishes_missing_fresh_and_stale_status() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let collector = Arc::new(Collector::from_environment(None).unwrap());
    let failure = router(root.to_path_buf(), Arc::clone(&collector))
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), io::ErrorKind::NotFound);
    fs::write(root.join("token"), "too-short").unwrap();
    assert_eq!(
        router(root.to_path_buf(), Arc::clone(&collector))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    fs::write(root.join("token"), format!("{TOKEN}\n")).unwrap();
    let app = router(root.to_path_buf(), collector).await.unwrap();
    let (address, listener) = super::super::tests::listen(app).await;
    let (code, state) = request(&address, "/healthz", None, None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        state,
        json!({"ok":false,"state":"unknown","age_text":"从未运行"})
    );
    let status_path = root.join("status.json");
    fs::write(&status_path, "[]").unwrap();
    assert_eq!(
        request(&address, "/status.json", None, None).await.1["state"],
        "unknown"
    );
    fs::write(
        &status_path,
        r#"{"state":"warn","state_label":"部分失败","count":7}"#,
    )
    .unwrap();
    let (code, state) = request(&address, "/healthz", None, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(state["state"], "warn");
    assert!(state["age_text"].as_str().unwrap().ends_with("秒前"));

    let file = fs::File::options().write(true).open(status_path).unwrap();
    for (age, unit) in [(720, "分钟前"), (7200, "小时前")] {
        file.set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
        let (code, state) = request(&address, "/status.json", None, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(state["state"], "stale");
        assert_eq!(state["state_label"], "已停滞");
        assert_eq!(state["count"], 7);
        assert!(state["age_seconds"].as_u64().unwrap() >= age);
        assert!(state["age_text"].as_str().unwrap().ends_with(unit));
        assert_eq!(
            request(&address, "/healthz", None, None).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    listener.abort();
}

#[tokio::test]
async fn repair_checks_authorization_identity_and_locks_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("token"), TOKEN).unwrap();
    let original = json!({
        "Id":"41","Type":"Episode","Path":"/media/番剧/show/episode.mkv",
        "SeriesId":"7","ParentIndexNumber":1,"IndexNumber":2,
        "Name":"第 2 集","Overview":"手工简介","ProviderIds":{"Tmdb":"11"},"LockedFields":[]
    });
    let item = Arc::new(Mutex::new(original.clone()));
    let emby = Router::new()
        .route(
            "/emby/Users/Query",
            get(|| async { Json(json!({"Items":[{"Id":"5","Policy":{"IsAdministrator":true}}]})) }),
        )
        .route(
            "/emby/Users/5/Items/41",
            get(|State(item): State<Arc<Mutex<Value>>>| async move {
                Json(item.lock().unwrap().clone())
            }),
        )
        .route(
            "/emby/Items/41",
            post(
                |State(item): State<Arc<Mutex<Value>>>, Json(input): Json<Value>| async move {
                    *item.lock().unwrap() = input;
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(Arc::clone(&item));
    let (address, server) = super::super::tests::listen(emby).await;
    let config = root.join("emby.json");
    fs::write(
        &config,
        json!({"enabled":true,"server":address,"api_key":"fixture"}).to_string(),
    )
    .unwrap();
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_emby_settings(Arc::new(EmbySettings::load(config).unwrap()));
    let app = router(root.to_path_buf(), Arc::new(collector))
        .await
        .unwrap();
    let (address, listener) = super::super::tests::listen(app).await;
    let input = json!({"original":original,"field":"Name","text":"正式标题"});

    for bad in [
        None,
        Some("wrong"),
        Some("fixture-media-arrival-token-00000001"),
    ] {
        assert_eq!(
            request(&address, "/repair", Some(input.clone()), bad).await,
            (StatusCode::UNAUTHORIZED, json!({"error":"UNAUTHORIZED"}))
        );
    }
    assert_eq!(*item.lock().unwrap(), original);

    let mut stale = input.clone();
    stale["original"]["ProviderIds"]["Tmdb"] = json!("13");
    assert_eq!(
        request(&address, "/repair", Some(stale), Some(TOKEN)).await,
        (StatusCode::CONFLICT, json!({"error":"EMBY_REPAIR_RETRY"}))
    );
    assert_eq!(
        request(&address, "/repair", Some(input), Some(TOKEN)).await,
        (StatusCode::OK, json!({"written":true}))
    );
    let saved = item.lock().unwrap().clone();
    assert_eq!(saved["Name"], "正式标题");
    assert_eq!(saved["ForcedSortName"], "正式标题");
    assert_eq!(saved["Overview"], original["Overview"]);
    assert_eq!(saved["LockedFields"], json!(["Name", "SortName"]));
    assert_eq!(
        request(
            &address,
            "/repair",
            Some(json!({
                "original":saved,"field":"Name","text":"再次尝试覆盖"
            })),
            Some(TOKEN)
        )
        .await,
        (StatusCode::OK, json!({"written":false}))
    );
    assert_eq!(*item.lock().unwrap(), saved);
    listener.abort();
    server.abort();
}
