use std::{collections::HashMap, fs, panic::AssertUnwindSafe, time::Duration};

use axum::{extract::OriginalUri, http::HeaderMap};
use futures_util::FutureExt;
use reqwest::{Client, Url};
use sqlx::PgPool;

use super::*;

async fn listen(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, server)
}

#[tokio::test]
async fn web_language_preferences_validate_and_persist() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("languages.json");
    let settings = Arc::new(LanguageSettings::load(Some(path.clone())).unwrap());
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_language_settings(settings);
    let (address, server) = listen(router(Arc::new(collector))).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };
    let route = "/api/settings/languages";
    assert_eq!(
        api.success(route, None).await["languages"],
        json!(["zh-CN"])
    );
    assert_eq!(
        api.success(route, Some(json!(["ja", "zh-hant"]))).await["languages"],
        json!(["ja", "zh-TW"])
    );
    assert_eq!(
        api.send(route, Some(json!(["invalid!"]))).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        LanguageSettings::load(Some(path)).unwrap().view().languages,
        ["ja", "zh-TW"]
    );
    server.abort();
}

#[tokio::test]
async fn folder_preview_groups_paths_without_creating_a_batch() {
    let collector = Arc::new(Collector::from_environment(None).unwrap());
    let (address, server) = listen(router(collector)).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };
    let route = "/api/folder-preview";
    let preview = api
        .success(
            route,
            Some(json!([
                "Library/Dark (2017)/Season 01/S01E02.mkv",
                "Library/Dark (2017)/Season 01/S01E01.mkv",
                "Library/Parasite (2019).mp4",
                "Library/readme.txt"
            ])),
        )
        .await;
    assert_eq!(
        preview["works"],
        json!([
            {"kind":"series", "query":"Dark (2017)", "file_count":2},
            {"kind":"movie", "query":"Parasite (2019)", "file_count":1}
        ])
    );
    assert_eq!(preview["files"], 3);
    assert_eq!(preview["skipped"], 1);
    assert!(preview.get("id").is_none());
    assert_eq!(
        api.success(route, Some(json!(["Library/readme.txt"])))
            .await["works"],
        json!([])
    );
    assert_eq!(
        api.send(route, Some(json!(["Library/../unsafe.mkv"])))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    server.abort();
}

async fn emby_fixture(headers: HeaderMap, OriginalUri(uri): OriginalUri) -> impl IntoResponse {
    if headers
        .get("x-emby-token")
        .and_then(|value| value.to_str().ok())
        != Some("test-secret")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = uri.path();
    if path.contains("/Images/") {
        return (
            [("content-type", "image/jpeg")],
            vec![255_u8, 216, 255, 224],
        )
            .into_response();
    }
    if path.ends_with("/Library/MediaFolders") {
        return Json(json!({"Items":[{"Id":"7","Name":"测试资料库"}]})).into_response();
    }
    let args: HashMap<_, _> = Url::parse(&format!("http://local{uri}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let film = json!({
        "Id":"30","Type":"Movie","Name":"海报电影","OriginalTitle":"Poster Movie","ProductionYear":2019,
        "Path":"/media/海报电影.mkv","PremiereDate":"2019-05-30T00:00:00Z","RunTimeTicks":7_200_000_000_i64,
        "Overview":"这是简介","Genres":["悬疑"],"CommunityRating":8.5,"ProviderIds":{"Tmdb":"496243"},
        "ImageTags":{"Primary":"3"},"BackdropImageTags":["4"],"People":[{"Id":"99","Name":"导演","Type":"Director","PrimaryImageTag":"3"}]
    });
    let show = json!({
        "Id":"31","Type":"Series","Name":"资料剧集","OriginalTitle":"Poster Series","ProductionYear":2017,
        "Overview":"这是简介","ProviderIds":{"Tmdb":"70523"},"ImageTags":{"Primary":"3"}
    });
    let data = match (
        args.get("Ids").map(String::as_str),
        args.get("ParentId").map(String::as_str),
    ) {
        (Some("30"), _) => vec![film],
        (Some("31"), _) => vec![show],
        (_, Some("31")) => vec![
            json!({"Id":"32","Type":"Season","Name":"一季"}),
            json!({"Id":"34","Type":"Episode","Name":"第一集","Overview":"已有中文译文","IndexNumber":1,"ParentIndexNumber":1,
                "Path":"/media/资料剧集/S01E01.mkv","ImageTags":{"Primary":"3"}}),
        ],
        (_, Some("7")) => vec![film, show],
        _ if args
            .get("AnyProviderIdEquals")
            .is_some_and(|value| value == "Tmdb.70523") =>
        {
            vec![show]
        }
        _ => vec![film],
    };
    Json(json!({"TotalRecordCount":data.len(),"Items":data})).into_response()
}

struct Api {
    address: String,
    client: Client,
}

impl Api {
    async fn send(&self, path: &str, input: Option<Value>) -> reqwest::Response {
        let url = format!("{}{path}", self.address);
        let request = match input {
            Some(value) => self
                .client
                .post(url)
                .header("Content-Type", "application/json")
                .body(value.to_string()),
            None => self.client.get(url),
        };
        request.send().await.unwrap()
    }

    async fn success(&self, path: &str, input: Option<Value>) -> Value {
        let response = self.send(path, input).await;
        let status = response.status();
        let text = response.text().await.unwrap();
        assert!(status.is_success(), "{path} {status}: {text}");
        if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap()
        }
    }

    async fn finish(&self, batch: i64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
        while tokio::time::Instant::now() < deadline {
            let view = self
                .success(&format!("/api/batches/{batch}/state"), None)
                .await;
            if view["rows"].as_array().unwrap().iter().all(|row| {
                matches!(
                    row["stage"].as_str(),
                    Some("success" | "partial" | "failed" | "not_found")
                )
            }) {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("batch {batch} did not finish");
    }
}

#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 创建隔离 schema"]
async fn web_routes_persist_and_export_an_emby_batch() {
    let url = env::var("COLLECT_METADATA_TEST_POSTGRES").unwrap();
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("metadata_web_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut test_url = Url::parse(&url).unwrap();
    test_url
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let outcome = AssertUnwindSafe(verify_web_routes(test_url))
        .catch_unwind()
        .await;
    let cleanup = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    cleanup.unwrap();
}

async fn verify_web_routes(test_url: Url) {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path();
    let collector = Arc::new(
        Collector::from_database_url(test_url.as_str())
            .unwrap()
            .with_tmdb_settings(Arc::new(
                TmdbSettings::from_file(config.join("tmdb-token")).unwrap(),
            ))
            .with_output_settings(Arc::new(
                OutputSettings::load(config.join("output.json"), Some(config.join("outputs")))
                    .unwrap(),
            ))
            .with_emby_settings(Arc::new(
                EmbySettings::load(config.join("emby.json")).unwrap(),
            )),
    );
    collector.init_storage().await.unwrap();
    let worker = tokio::spawn(Arc::clone(&collector).run_batch_worker());
    let (address, server) = listen(router(collector)).await;
    let (mock, emby) = listen(Router::new().fallback(get(emby_fixture))).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };

    for route in [
        "/",
        "/app.js",
        "/style.css",
        "/icon.png",
        "/tmdb-logo.svg",
        "/healthz",
        "/api/settings/postgres",
    ] {
        assert!(api.send(route, None).await.status().is_success(), "{route}");
    }
    assert_eq!(api.send("/api/emby/libraries", None).await.status(), 400);
    assert_eq!(
        api.send("/api/batches/9999/state", None).await.status(),
        503
    );
    let sync = "/api/emby/sync/fixture-server";
    assert!(matches!(
        api.send(sync, Some(json!({}))).await.status().as_u16(),
        401 | 503
    ));
    if let Ok(token) = env::var("EMBY_SYNC_TOKEN") {
        let sync_item = |body: Value| {
            api.client
                .post(format!("{}{sync}", api.address))
                .bearer_auth(&token)
                .header("content-type", "application/json")
                .body(body.to_string())
        };
        assert_eq!(
            sync_item(json!({"Id":"0","Type":"Other"}))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        let document = json!({"Id":"1","Type":"Movie","Name":"Emby 同步电影"});
        api.success("/api/settings/languages", Some(json!(["zh-CN", "ja"])))
            .await;
        let db = PgPool::connect(test_url.as_str()).await.unwrap();
        for (preference, expected) in [
            (Some("en-GB"), json!(["en-GB"])),
            (None, json!(["zh-CN", "ja"])),
        ] {
            let mut request = sync_item(document.clone());
            if let Some(preference) = preference {
                request = request.header("x-emby-metadata-language", preference);
            }
            assert_eq!(request.send().await.unwrap().status(), 204);
            let synced: Value = sqlx::query_scalar("SELECT document FROM collect_metadata_emby_items WHERE server_id='fixture-server' AND item_id='1'")
                .fetch_one(&db).await.unwrap();
            assert_eq!(synced["_collect_metadata"]["metadata_languages"], expected);
            assert_eq!(synced["Name"], document["Name"]);
        }
    }
    let emby_options = json!({"enabled":true, "server":mock,"api_key":"test-secret"});
    let invalid_emby = json!({"enabled":true,"server":"file:///invalid"});
    for route in ["/api/settings/emby", "/api/settings/emby/test"] {
        assert_eq!(
            api.send(route, Some(invalid_emby.clone())).await.status(),
            400
        );
    }
    api.success("/api/settings/emby/test", Some(emby_options.clone()))
        .await;
    assert_eq!(
        api.success("/api/settings/emby", Some(emby_options.clone()))
            .await["configured"],
        true
    );
    assert!(
        api.success("/api/settings/emby", None)
            .await
            .get("api_key")
            .is_none()
    );
    assert_eq!(api.success("/api/emby/libraries", None).await[0]["id"], "7");
    assert_eq!(
        api.send("/api/emby/import", Some(json!("unknown")))
            .await
            .status(),
        400
    );
    let preferences = json!({"enabled":true,"directory":"series","directory_template":"{title} ({year})","filename_prefix":"{id}",
        "poster":true,"backdrop":true,"stills":true,"raw":true,"emby_nfo":true,"portraits":["director"]});
    assert_eq!(
        api.success("/api/settings/output", Some(preferences)).await["options"]["directory"],
        "series"
    );
    api.success("/api/settings/output", None).await;
    assert_eq!(
        api.send(
            "/api/settings/output",
            Some(json!({"enabled":true,"directory":"../escape"}))
        )
        .await
        .status(),
        400
    );
    api.success("/api/settings/tmdb", None).await;
    assert_eq!(
        api.send(
            "/api/settings/tmdb/test",
            Some(json!({"credential":"bad credential"})),
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        api.send(
            "/api/settings/tmdb",
            Some(json!({"credential":"bad credential"}))
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        api.send("/api/lookups", Some(json!({"query":"bad?"})))
            .await
            .status(),
        400
    );
    assert_eq!(
        api.send("/api/search", Some(json!({"kind":"av","query":"word"})))
            .await
            .status(),
        400
    );
    assert_eq!(
        api.send(
            "/api/works",
            Some(json!({"kind":"av","provider":"tmdb","id":"bad"}))
        )
        .await
        .status(),
        400
    );

    api.success(
        "/api/settings/emby",
        Some(json!({"enabled":false, "server":mock})),
    )
    .await;
    let before_preview = api.success("/api/batches", None).await;
    let preview = api.success("/api/emby/preview", Some(json!("7"))).await;
    assert_eq!(
        preview,
        json!([
            {"id":"30", "kind":"movie", "title":"海报电影"},
            {"id":"31", "kind":"series", "title":"资料剧集"}
        ])
    );
    assert_eq!(api.success("/api/batches", None).await, before_preview);
    let imported = api
        .success("/api/emby/import", Some(json!("7")))
        .await
        .as_i64()
        .unwrap();
    let view = api.finish(imported).await;
    assert_eq!(view["rows"].as_array().unwrap().len(), 2);
    let rows = api.success(&format!("/api/batches/{imported}"), None).await;
    let film = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "movie")
        .unwrap();
    let path = film["output"]["directory"].as_str().unwrap();
    assert!(path.contains("海报电影 (2019)"));
    let files = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(files.iter().any(|file| file == "Emby-30-metadata.json"));
    assert!(files.iter().any(|file| file == "海报电影.nfo"));
    assert!(
        files
            .iter()
            .any(|file| file.to_string_lossy().starts_with("Emby-30-director-"))
    );
    let history = api.success("/api/history?kind=movie", None).await;
    assert_eq!(history[0]["year"], 2019);
    let id = history[0]["id"].as_i64().unwrap();
    assert_eq!(
        api.success(&format!("/api/history/{id}"), None).await["record"]["fields"]["title"],
        "海报电影"
    );
    api.success("/api/history/languages", None).await;
    assert_eq!(api.send("/api/history/99999", None).await.status(), 404);
    assert_eq!(
        api.send("/api/history?language=%23", None).await.status(),
        503
    );
    api.success("/api/batches", None).await;
    api.success(&format!("/api/batches?before={imported}"), None)
        .await;

    api.success("/api/settings/emby", Some(emby_options)).await;
    let folder = api
        .success(
            "/api/folder-batches",
            Some(json!(["library/Poster Series.[tmdb-70523].S01E01.mkv"])),
        )
        .await;
    let batch = folder["id"].as_i64().unwrap();
    let rows = api.finish(batch).await;
    let row = rows["rows"][0]["id"].as_i64().unwrap();
    assert_eq!(
        api.success(&format!("/api/batches/{batch}/rows/{row}/files"), None)
            .await[0],
        "library/Poster Series.[tmdb-70523].S01E01.mkv"
    );
    let result = api
        .success(&format!("/api/batches/{batch}/rows/{row}/result"), None)
        .await;
    assert_eq!(
        result["record"]["fields"]["episodes"][0]["description"],
        "已有中文译文"
    );
    assert_eq!(
        api.send(
            &format!("/api/batches/{batch}/rows/{row}/candidate"),
            Some(json!({"provider":"tmdb","id":"1"}))
        )
        .await
        .status(),
        409
    );
    assert_eq!(
        api.send(
            &format!("/api/batches/{batch}/rows/{row}/retry"),
            Some(json!({}))
        )
        .await
        .status(),
        409
    );
    assert_eq!(
        api.send(
            &format!("/api/batches/{batch}/control"),
            Some(json!("cancel"))
        )
        .await
        .status(),
        409
    );

    let invalid = api
        .success("/api/batches", Some(json!([{"kind":"av","query":"bad?"}])))
        .await
        .as_i64()
        .unwrap();
    assert_eq!(api.finish(invalid).await["rows"][0]["stage"], "failed");
    assert_eq!(
        api.success(
            &format!("/api/batches/{invalid}/control"),
            Some(json!("retry_failed"))
        )
        .await,
        1
    );
    api.finish(invalid).await;
    assert_eq!(
        api.send("/api/batches", Some(json!([]))).await.status(),
        400
    );
    assert_eq!(
        api.send("/api/folder-batches", Some(json!(["/absolute/file.mkv"])))
            .await
            .status(),
        400
    );
    assert!(
        api.success("/api/folder-batches", Some(json!(["library/readme.txt"])))
            .await["id"]
            .is_null()
    );
    assert_eq!(
        api.send("/api/batches/9999/rows/9999/result", None)
            .await
            .status(),
        404
    );
    assert_eq!(
        api.send("/api/batches/9999/rows/9999/files", None)
            .await
            .status(),
        404
    );
    let malformed = api
        .success(
            "/api/search",
            Some(json!({"kind":"movie","query":"tmdb:invalid"})),
        )
        .await;
    assert_eq!(malformed["candidates"], json!([]));
    assert!(!malformed["notes"].as_array().unwrap().is_empty());
    if let Ok(credential) = env::var("TMDB_READ_ACCESS_TOKEN") {
        api.success(
            "/api/settings/tmdb/test",
            Some(json!({"credential":credential})),
        )
        .await;
        api.success("/api/settings/tmdb", Some(json!({"credential":credential})))
            .await;
        assert_eq!(api.success("/api/settings/tmdb", None).await, "saved");
        let tmdb = api
            .success(
                "/api/works",
                Some(json!({"kind":"movie","provider":"tmdb","id":"496243"})),
            )
            .await;
        assert_eq!(tmdb["record"]["fields"]["title"], "寄生虫");
        assert!(tmdb["saved"]["cover_url"].as_str().is_some());
        let av = api
            .success("/api/lookups", Some(json!({"query":"SSIS-001"})))
            .await;
        assert_eq!(av["saved"]["media_type"], "av");
        let query = api
            .success(
                "/api/batches",
                Some(json!([{"kind":"series","query":"tmdb:70523"}])),
            )
            .await
            .as_i64()
            .unwrap();
        let view = api.finish(query).await;
        let row = &view["rows"][0];
        assert!(matches!(row["stage"].as_str(), Some("success" | "partial")));
        let result = api
            .success(
                &format!("/api/batches/{query}/rows/{}/result", row["id"]),
                None,
            )
            .await;
        assert_eq!(result["record"]["fields"]["title"], "暗黑");
        assert_eq!(
            api.success("/api/history?kind=series&language=de", None)
                .await[0]["title"],
            "暗黑"
        );
    }
    worker.abort();
    server.abort();
    emby.abort();
    let _ = worker.await;
    let _ = server.await;
}
