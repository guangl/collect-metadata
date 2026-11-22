use std::path::Path;
use std::{collections::HashMap, fs, panic::AssertUnwindSafe, time::Duration};

use axum::{Json, extract::{OriginalUri, Query}, http::HeaderMap, routing::post};
use serde_json::Value;
use futures_util::FutureExt;
use reqwest::{Client, Url};
use sqlx::{AssertSqlSafe, PgPool};

use super::*;

async fn listen(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, server)
}

#[tokio::test]
async fn process_health_is_independent_of_postgres_readiness() {
    let stalled_db = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let database = format!("postgres://fixture@{}/fixture", stalled_db.local_addr().unwrap());
    let collector = Arc::new(Collector::from_database_url(&database).unwrap());
    let (address, server) = listen(router(collector)).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };
    assert_eq!(
        api.send("/healthz", None).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api.send("/readyz", None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.abort();
}

#[tokio::test]
async fn failed_api_requests_have_safe_errors_and_server_generated_ids() {
    let collector = Arc::new(Collector::from_environment(None).unwrap());
    let (address, server) = listen(router(collector)).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let supplied = "do-not-echo";
    let invalid = client
        .post(format!("{address}/api/search?private={supplied}"))
        .header("x-request-id", supplied)
        .json(&json!({"kind": supplied, "query": supplied}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let id = invalid.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(id.len(), 32);
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(invalid.headers()[header::CACHE_CONTROL], "no-store");
    let body = invalid.text().await.unwrap();
    assert!(!body.contains(supplied));
    let body = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(body["code"], "INVALID_INPUT");
    assert_eq!(body["diagnostic_id"], id);
    assert!(body["error"].is_string());

    let rejected = client
        .post(format!("{address}/api/search"))
        .json(&json!({"kind":"movie", "query":" "}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let request_id = rejected.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = rejected.json::<Value>().await.unwrap();
    assert_eq!(body["code"], "MEDIA_SEARCH_FAILED");
    assert_eq!(body["error"], "请输入电影或剧集名称（最多 160 个字符）。");
    assert_eq!(body["diagnostic_id"], request_id);

    let unknown = client
        .get(format!("{address}/api/old/{supplied}"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_ne!(unknown.headers()["x-request-id"].to_str().unwrap(), id);
    assert_eq!(
        unknown.json::<Value>().await.unwrap()["code"],
        "RESOURCE_NOT_FOUND"
    );
    server.abort();
}

#[tokio::test]
async fn bangumi_routes_report_status_without_returning_the_token() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("bangumi-token");
    fs::write(&path, "secret-fixture-token\n").unwrap();
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_bangumi_settings(Arc::new(BangumiSettings::from_file(path.clone()).unwrap()));
    let (address, server) = listen(router(Arc::new(collector))).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };
    assert_eq!(api.send("/readyz", None).await.status(), 204);
    assert_eq!(api.success("/api/settings/postgres", None).await, "none");
    let route = "/api/settings/bangumi";
    assert_eq!(api.success(route, None).await, "saved");
    for path in [route, "/api/settings/bangumi/test"] {
        let rejected = api
            .send(path, Some(json!({"credential":"invalid token"})))
            .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert!(!rejected.text().await.unwrap().contains("invalid token"));
    }
    assert_eq!(fs::read_to_string(path).unwrap(), "secret-fixture-token\n");
    server.abort();
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
    if let Some((_, kind)) = path.split_once("/Images/") {
        let (mime, bytes): (&str, &[u8]) = match kind {
            "Logo" => ("image/png", b"\x89PNG\r\n\x1a\n"),
            "Primary" | "Backdrop" | "Thumb" => ("image/jpeg", &[255, 216, 255, 224]),
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        return ([("content-type", mime)], bytes).into_response();
    }
    if path.ends_with("/Library/MediaFolders") {
        return Json(json!({"Items":[
            {"Id":"7","Name":"测试资料库"},
            {"Id":"8","Name":"电视馆"},
            {"Id":"9","Name":"空媒体库"}
        ]}))
        .into_response();
    }
    let args: HashMap<_, _> = Url::parse(&format!("http://local{uri}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    if args.get("HasOverview").is_some_and(|value| value == "false") {
        assert_eq!(args["ParentId"], "7");
        assert_eq!(args["IncludeItemTypes"], "Episode");
        assert_eq!(args["Recursive"], "true");
        assert_eq!(args["IsMissing"], "false");
        assert_eq!(args["EnableImages"], "false");
        let items = match args["StartIndex"].as_str() {
            "0" => json!([
                {"Id":"37","Type":"Episode","SeriesId":"31"},
                {"Id":"80","Type":"Episode","SeriesId":"31","LocationType":"Virtual"}
            ]),
            "2" => json!([
                {"Id":"37","Type":"Episode","SeriesId":"31"},
                {"Id":"81","Type":"Episode","SeriesId":"31","Overview":"已有简介"},
                {"Id":"82","Type":"Episode","SeriesId":"35","Overview":"  "}
            ]),
            _ => panic!("unexpected page size"),
        };
        return Json(json!({"Items":items,"TotalRecordCount":5})).into_response();
    }
    let film = json!({
        "Id":"30","Type":"Movie","Name":"海报电影","OriginalTitle":"Poster Movie","ProductionYear":2019,
        "Path":"/media/海报电影.mkv","PremiereDate":"2019-05-30T00:00:00Z","RunTimeTicks":7_200_000_000_i64,
        "Overview":"这是简介","Genres":["悬疑"],"CommunityRating":8.5,"CriticRating":96,"OfficialRating":"R",
        "ProviderIds":{"Tmdb":"496243","Imdb":"tt6751668","Bangumi":"303162"},
        "TagItems":[{"Id":19,"Name":"剧情"}],
        "ImageTags":{"Primary":"3","Logo":"5","Thumb":"6"},"BackdropImageTags":["4"],"People":[{"Id":"99","Name":"导演","Type":"Director","PrimaryImageTag":"3"},
            {"Id":"98","Name":"声优","Type":"Actor","Role":"角色 (voice)","PrimaryImageTag":"2"},
            {"Id":"bad/path","Name":"无照片编剧","Type":"Writer","PrimaryImageTag":"hidden"}]
    });
    let show = json!({
        "Id":"31","Type":"Series","Name":"资料剧集","OriginalTitle":"Poster Series","ProductionYear":2017,
        "Overview":"这是简介","Path":"/media/资料剧集","ProviderIds":{"Tmdb":"70523"},"ImageTags":{"Primary":"3"}
    });
    let extra = json!({"Id":"35","Type":"Series","Name":"本地剧集"});
    let mut episode = json!({
        "Id":"34","Type":"Episode","Name":"第一集","SeriesName":"资料剧集","Overview":"已有中文译文",
        "IndexNumber":1,"ParentIndexNumber":1,"Path":"/media/资料剧集/Season 1/S01E01.mkv",
        "ImageTags":{"Primary":"3"}
    });
    let fields = args.get("Fields").map(String::as_str).unwrap_or_default();
    for (requested, key, value) in [
        ("PremiereDate", "PremiereDate", json!("2022-02-02T00:00:00Z")),
        ("CommunityRating", "CommunityRating", json!(8.1)),
        ("DateCreated", "DateCreated", json!("2022-03-01T00:00:00Z")),
        ("Settings", "LockData", json!(false)),
        ("VideoCodec", "VideoCodec", json!("hevc")),
    ] {
        if fields.split(',').any(|field| field == requested) {
            episode[key] = value;
        }
    }
    let video = json!({
        "Id":"40","Type":"Video","Name":"家庭旅行","Path":"/media/2024旅行.mov",
        "Overview":"自己的旅行记录","ImageTags":{"Primary":"5"},
        "MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}]
    });
    let mut data = match (
        args.get("Ids").map(String::as_str),
        args.get("ParentId").map(String::as_str),
    ) {
        (Some("30"), _) => vec![film],
        (Some("31"), _) => vec![show],
        (Some("34"), _) => vec![episode],
        (Some("35"), _) => vec![extra],
        (Some("40"), _) => vec![video],
        (_, Some("31")) => vec![
            json!({"Id":"33","Type":"Season","Name":"第二季","IndexNumber":2}),
            json!({"Id":"32","Type":"Season","Name":"第一季","IndexNumber":1}),
            json!({"Id":"38","Type":"Season","Name":"特别篇","IndexNumber":0}),
            episode,
            json!({"Id":"37","Type":"Episode","Name":"回归","IndexNumber":1,"SeasonId":"33",
                "Path":"/media/资料剧集/Season 2/S01E01.mkv"}),
        ],
        (_, Some("7")) => {
            assert_eq!(args["EnableImageTypes"], "Primary,Backdrop,Logo,Thumb");
            let first = args.get("StartIndex").is_some_and(|index| index == "0");
            return Json(json!({"TotalRecordCount":2,"Items":[if first { film } else { show }]}))
                .into_response();
        }
        (_, Some("8")) => vec![
            extra,
            film,
            video,
            json!({"Id":"41","Type":"Video","Name":"花絮","ExtraType":"BehindTheScenes"}),
            json!({"Id":"42","Type":"Video","Name":"虚拟条目","LocationType":"Virtual"}),
            json!({"Id":"43","Type":"Photo","Name":"旅行照片"}),
        ],
        (_, Some("9" | "35")) => vec![],
        _ if args
            .get("AnyProviderIdEquals")
            .is_some_and(|value| value == "Tmdb.70523") =>
        {
            vec![show]
        }
        _ if args
            .get("SearchTerm")
            .is_some_and(|value| value == "家庭旅行") =>
        {
            vec![video]
        }
        _ => vec![film],
    };
    if let Some(types) = args.get("IncludeItemTypes") {
        data.retain(|item| types.split(',').any(|kind| item["Type"] == kind));
    }
    for item in &mut data {
        let requested = args.get("Fields").map_or("", String::as_str);
        for field in ["ProductionYear", "CommunityRating", "CriticRating", "OfficialRating", "TagItems"] {
            if !requested.split(',').any(|name| name == field) {
                item.as_object_mut().unwrap().remove(field);
            }
        }
    }
    Json(json!({"TotalRecordCount":data.len(),"Items":data})).into_response()
}

#[tokio::test]
async fn emby_catalog_previews_multiple_libraries_and_proxies_existing_art() {
    let (mock, emby) = listen(Router::new().fallback(get(emby_fixture))).await;
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("emby.json");
    fs::write(&path, br#"{"enabled":false,"server":"","api_key":""}"#).unwrap();
    fs::write(path.with_file_name("imdb-ratings.json"), br#"{"tt6751668":{"value":8.6,"count":12345}}"#).unwrap();
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_emby_settings(Arc::new(EmbySettings::load(path).unwrap()));
    let (address, server) = listen(router(Arc::new(collector))).await;
    let api = Api {
        address,
        client: Client::builder().no_proxy().build().unwrap(),
    };
    assert!(api.success("/api/emby/catalog", None).await.is_null());
    api.success(
        "/api/settings/emby",
        Some(json!({"enabled":false,"server":mock,"api_key":"test-secret"})),
    )
    .await;

    let response = api.send("/api/emby/catalog", None).await;
    let json = response.text().await.unwrap();
    assert!(!json.contains("test-secret"));
    let catalog: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 3);
    assert_eq!(catalog[0]["works"][1]["title"], "资料剧集");
    assert_eq!(catalog[1]["works"][0]["title"], "本地剧集");
    assert!(catalog[1]["works"][0]["poster"].is_null());
    assert_eq!(catalog[2]["works"], json!([]));
    assert_eq!(
        api.success("/api/emby/libraries/7/missing-episode-overviews", None).await,
        json!({"31":["37"], "35":["82"]}),
    );
    assert_eq!(
        api.send("/api/emby/libraries/404/missing-episode-overviews", None).await.status(),
        StatusCode::BAD_REQUEST,
    );
    let film = &catalog[0]["works"][0];
    assert_eq!(film["poster"], "3");
    assert_eq!(film["backdrop"], "4");
    assert_eq!(film["logo"], "5");
    assert_eq!(film["landscape"], "6");
    assert_eq!(film["overview"], "这是简介");
    assert_eq!(film["year"], 2019);
    assert_eq!(film["rating"], 8.5);
    assert_eq!(film["critic_rating"], 96.0);
    assert_eq!(film["official_rating"], "R");
    assert_eq!(film["tags"], json!(["剧情"]));
    assert_eq!(film["tmdb_id"], "496243");
    assert_eq!(film["bangumi_id"], "303162");
    assert_eq!(film["imdb_rating"], json!({"value":8.6,"count":12345}));
    assert_eq!(film["runtime_minutes"], 12);
    for image in ["poster", "backdrop", "landscape"] {
        let response = api
            .send(&format!("/api/emby/images/30/{image}"), None)
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            &[255, 216, 255, 224]
        );
    }
    let logo = api.send("/api/emby/images/30/logo", None).await;
    assert_eq!(logo.status(), 200);
    assert_eq!(logo.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(logo.bytes().await.unwrap().as_ref(), b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        api.send("/api/emby/images/invalid%2Fid/poster", None)
            .await
            .status(),
        404
    );
    assert_eq!(
        api.success("/api/emby/episodes/34/overview", None).await,
        json!({"overview":"已有中文译文","source":"Emby"}),
    );
    assert_eq!(
        api.send("/api/emby/episodes/30/overview", None).await.status(),
        StatusCode::BAD_REQUEST,
    );
    let people = api.success("/api/emby/credits/30", None).await;
    assert_eq!(people[0], json!({"id":"99","name":"导演","role":"director","detail":null,"portrait":"3"}));
    assert_eq!(people[1]["role"], "voice");
    assert_eq!(people[1]["detail"], "角色 (voice)");
    assert_eq!(people[2]["id"], Value::Null);
    assert_eq!(people[2]["portrait"], Value::Null);
    let portrait = api.send("/api/emby/images/99/portrait", None).await;
    assert_eq!(portrait.status(), 200);
    assert_eq!(portrait.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(api.send("/api/emby/credits/bad%2Fid", None).await.status(), 400);
    let series = api.success("/api/emby/series/31", None).await;
    assert_eq!(series["partial"], false);
    let seasons = series["seasons"].as_array().unwrap();
    assert_eq!(seasons.len(), 3);
    assert_eq!(seasons[0]["number"], 0);
    assert_eq!(seasons[0]["episodes"], json!([]));
    assert_eq!(seasons[1]["episodes"][0]["title"], "第一集");
    assert_eq!(seasons[1]["episodes"][0]["date"], "2022-02-02");
    assert_eq!(seasons[1]["episodes"][0]["premiere_at"], "2022-02-02T00:00:00Z");
    assert_eq!(seasons[1]["episodes"][0]["rating"], 8.1);
    assert_eq!(seasons[1]["episodes"][0]["overview"], "已有中文译文");
    assert_eq!(seasons[1]["episodes"][0]["image"], "3");
    assert_eq!(seasons[2]["episodes"][0]["id"], "37");
    assert_eq!(
        api.send("/api/emby/images/34/episode", None).await.status(),
        200
    );
    emby.abort();
    server.abort();
}

struct Api {
    address: String,
    client: Client,
}

#[tokio::test]
async fn emby_overwrite_validates_target_and_requests_native_refresh() {
    let (requests, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (mock, emby) = listen(
        Router::new()
            .route("/emby/Items/{id}/Refresh", post(move |axum::extract::Path(id): axum::extract::Path<String>,
                headers: HeaderMap, Query(params): Query<HashMap<String, String>>| {
                let requests = requests.clone();
                async move {
                    assert_eq!(headers["x-emby-token"], "test-secret");
                    requests.send((id.clone(), params)).unwrap();
                    if id == "34" { StatusCode::FORBIDDEN } else { StatusCode::NO_CONTENT }
                }
            }))
            .fallback(get(emby_fixture)),
    ).await;
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("emby.json");
    fs::write(&path, json!({"enabled":false,"server":mock,"api_key":"test-secret"}).to_string()).unwrap();
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_emby_settings(Arc::new(EmbySettings::load(path).unwrap()));
    let (address, server) = listen(router(Arc::new(collector))).await;
    let api = Api { address, client: Client::builder().no_proxy().build().unwrap() };
    let route = "/api/emby/overwrite";

    for target in [
        json!({"type":"library","id":"40"}),
        json!({"type":"item","id":"7","scope":"movie"}),
        json!({"type":"item","id":"30","scope":"episode"}),
        json!({"type":"item","id":"bad/id","scope":"movie"}),
    ] {
        assert_eq!(api.send(route, Some(json!({"target":target}))).await.status(), 400);
    }
    assert!(received.try_recv().is_err());

    for (target, id, recursive, replace_images) in [
        (json!({"type":"library","id":"8"}), "8", true, false),
        (json!({"type":"item","id":"30","scope":"movie"}), "30", false, false),
        (json!({"type":"item","id":"40","scope":"movie"}), "40", false, true),
        (json!({"type":"item","id":"31","scope":"series"}), "31", true, false),
        (json!({"type":"item","id":"34","scope":"episode"}), "34", false, true),
    ] {
        let input = if replace_images {
            json!({"target":target,"replace_images":true})
        } else {
            json!({"target":target})
        };
        let reply = api.send(route, Some(input)).await;
        if id == "34" {
            assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
            assert!(reply.text().await.unwrap().contains("HTTP 403"));
        } else {
            assert_eq!(reply.status(), StatusCode::NO_CONTENT);
        }
        let (requested, params) = received.try_recv().unwrap();
        assert_eq!(requested, id);
        assert_eq!(params["Recursive"], recursive.to_string());
        assert_eq!(params["MetadataRefreshMode"], "FullRefresh");
        assert_eq!(params["ReplaceAllMetadata"], "true");
        assert_eq!(params["ReplaceAllImages"], replace_images.to_string());
        assert_eq!(params["ImageRefreshMode"], if replace_images { "FullRefresh" } else { "Default" });
    }

    for (id, scope, recursive) in [("30", "movie", "false"), ("31", "series", "true")] {
        let target = json!({"type":"item","id":id,"scope":scope});
        assert_eq!(
            api.send("/api/emby/refresh", Some(target)).await.status(),
            204
        );
        let (requested, params) = received.try_recv().unwrap();
        assert_eq!(requested, id);
        assert_eq!(params["Recursive"], recursive);
        assert_eq!(params["MetadataRefreshMode"], "FullRefresh");
        assert_eq!(params["ImageRefreshMode"], "FullRefresh");
        assert_eq!(params["ReplaceAllMetadata"], "false");
        assert_eq!(params["ReplaceAllImages"], "false");
    }
    emby.abort();
    server.abort();
}

#[cfg(feature = "desktop")]
#[tokio::test]
async fn desktop_folder_scrapes_and_exports_without_postgres() {
    let (mock, emby) = listen(Router::new().fallback(get(emby_fixture))).await;
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path();
    fs::write(
        path.join("emby.json"),
        json!({
            "enabled":true,"server":mock,"api_key":"test-secret"
        })
        .to_string(),
    )
    .unwrap();
    let output = OutputSettings::load(path.join("output.json"), Some(path.join("output"))).unwrap();
    output
        .save(
            serde_json::from_value(json!({
                "enabled": true, "directory": "local", "poster": false,
            }))
            .unwrap(),
        )
        .unwrap();
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_emby_settings(Arc::new(
            EmbySettings::load(path.join("emby.json")).unwrap(),
        ))
        .with_output_settings(Arc::new(output));

    let video = path.join("Media/家庭旅行.mov");
    fs::create_dir(video.parent().unwrap()).unwrap();
    fs::write(&video, "fixture").unwrap();
    let selection = collect_metadata::scan_native_folder(video.parent().unwrap()).unwrap();
    let file = selection.preview.works[0].source_files.clone();
    let result = collector
        .scrape_folder_work(-1, 1, file, None)
        .await
        .unwrap();
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(result["stage"], "success");
    assert_eq!(result["result"]["record"]["fields"]["title"], "家庭旅行");
    assert!(result["result"]["saved"].is_null());
    let metadata = path.join("output/local/batch--1/work-1/metadata.json");
    assert!(metadata.is_file());
    assert_eq!(fs::read_to_string(video).unwrap(), "fixture");
    emby.abort();
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

    async fn preview_writeback(&self, batch: i64, row: Option<i64>) -> Value {
        let url = format!("/api/batches/{batch}/writeback");
        self.success(&row.map_or_else(|| url.clone(), |row| format!("{url}?row={row}")), None).await
    }

    async fn apply_writeback(&self, batch: i64, row: Option<i64>, preview: &Value) -> Value {
        self.success(
            &format!("/api/batches/{batch}/writeback"),
            Some(json!({"row":row,"token":preview["token"]})),
        ).await;
        self.finish(batch).await
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
    // The SQL identifier contains a fixed prefix and a hexadecimal UUID.
    let schema = format!("metadata_web_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
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
    let cleanup = sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
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
    let media = config.join("media");
    let series = media.join("资料剧集");
    let season_one = series.join("Season 1");
    let season_two = series.join("Season 2");
    fs::create_dir_all(&season_one).unwrap();
    fs::create_dir_all(&season_two).unwrap();
    for video in [
        media.join("海报电影.mkv"),
        media.join("2024旅行.mov"),
        season_one.join("S01E01.mkv"),
        season_two.join("S01E01.mkv"),
    ] {
        fs::write(video, "video fixture").unwrap();
    }
    fs::write(season_one.join("S01E01.nfo"), "previous NFO").unwrap();
    fs::write(config.join("imdb-ratings.json"), br#"{"tt6751668":{"value":8.6,"count":12345}}"#).unwrap();
    let collector = Arc::new(
        Collector::from_database_url(test_url.as_str())
            .unwrap()
            .with_tmdb_settings(Arc::new(
                TmdbSettings::from_file(config.join("tmdb-token")).unwrap(),
            ))
            .with_output_settings(Arc::new(
                OutputSettings::load(config.join("output.json"), Some(config.join("outputs")))
                    .unwrap()
                    .with_emby_root(Some(media.clone())),
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
        "/readyz",
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
        let document = json!({"Id":"1","Type":"Video","Name":"Emby 同步私人影片"});
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
            assert_eq!(synced["Type"], "Video");
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
    let preferences = json!({"enabled":false,"directory":"series","directory_template":"{title} ({year})","filename_prefix":"{id}",
        "emby_paths":[{"server":"/media","local":""}],
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
    assert_eq!(preview.as_array().unwrap().len(), 2);
    assert_eq!(preview[0]["title"], "海报电影");
    assert_eq!(preview[1]["title"], "资料剧集");
    let private_preview = api.success("/api/emby/preview", Some(json!("8"))).await;
    assert_eq!(private_preview.as_array().unwrap().len(), 3);
    assert_eq!(private_preview[2]["title"], "家庭旅行");
    assert_eq!(api.success("/api/batches", None).await, before_preview);
    for (library, expected) in [
        ("7", vec![("emby:30", "success"), ("emby:31", "success")]),
        ("8", vec![("emby:35", "success"), ("emby:30", "success"), ("emby:40", "success")]),
    ] {
        let batch = api
            .success("/api/emby/import", Some(json!(library)))
            .await
            .as_i64()
            .unwrap();
        let finished = api.finish(batch).await;
        let rows = finished["rows"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row["query"].as_str().unwrap(), row["stage"].as_str().unwrap()))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(rows.iter().all(|row| row["has_result"] == true));
    }
    let selected = api.success("/api/emby/import-selection", Some(json!({
        "items": [{"id":"30","scope":"movie"}, {"id":"30","scope":"movie"}, {"id":"40","scope":"movie"}]
    }))).await.as_i64().unwrap();
    let selected = api.finish(selected).await;
    assert_eq!(selected["rows"].as_array().unwrap().len(), 2);
    assert_eq!(selected["rows"][0]["query"], "emby:30");
    assert_eq!(selected["rows"][1]["query"], "emby:40");
    assert!(
        selected["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["stage"] == "success")
    );

    let imported = api
        .success("/api/emby/import-all", Some(Value::Null))
        .await
        .as_i64()
        .unwrap();
    let view = api.finish(imported).await;
    assert_eq!(view["rows"].as_array().unwrap().len(), 4);
    let film = view["rows"].as_array().unwrap().iter().find(|row| row["query"] == "emby:30").unwrap();
    assert_eq!(film["writeback"], "ready");
    let film_id = film["id"].as_i64().unwrap();
    let film_nfo = media.join("海报电影.nfo");
    let film_poster = media.join("海报电影-poster.jpg");
    assert!(!film_nfo.exists(), "collection must leave sidecars untouched");
    assert_eq!(fs::read_to_string(season_one.join("S01E01.nfo")).unwrap(), "previous NFO");

    let stale = api.preview_writeback(imported, Some(film_id)).await;
    fs::write(&film_nfo, "original film metadata").unwrap();
    assert_eq!(api.send(
        &format!("/api/batches/{imported}/writeback"),
        Some(json!({"row":film_id,"token":stale["token"]})),
    ).await.status(), StatusCode::CONFLICT);
    assert_eq!(fs::read_to_string(&film_nfo).unwrap(), "original film metadata");

    // A destination that is a directory cannot be replaced. Other files still publish.
    fs::create_dir(&film_poster).unwrap();
    let preview = api.preview_writeback(imported, None).await;
    assert!(preview["files"].as_array().unwrap().iter().any(|file| {
        file["kind"] == "nfo" && file["destinations"][0]["path"] == film_nfo.to_str().unwrap()
            && file["destinations"][0]["action"] == "replace"
    }));
    let committed = api.apply_writeback(imported, None, &preview).await;
    let rows = committed["rows"].as_array().unwrap();
    let film = rows.iter().find(|row| row["id"] == film_id).unwrap();
    assert_eq!(film["stage"], "success");
    assert_eq!(film["writeback"], "failed");
    assert_eq!(film["output"]["written"], 4);
    assert_eq!(film["output"]["failed"], 1);
    assert_eq!(Path::new(film["output"]["directory"].as_str().unwrap()), media);
    assert!(film_nfo.is_file());
    assert!(film_poster.is_dir());
    for filename in ["海报电影-thumb.jpg", "海报电影-clearlogo.png", "海报电影-landscape.jpg"] {
        assert!(media.join(filename).is_file(), "{filename}");
    }
    assert_eq!(fs::read_to_string(media.join("海报电影.mkv")).unwrap(), "video fixture");
    let status = api.preview_writeback(imported, Some(film_id)).await;
    let nfo = status["files"].as_array().unwrap().iter().find(|file| file["kind"] == "nfo").unwrap();
    let backup = nfo["backup"].as_str().unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), "original film metadata");
    let result_path = format!("/api/batches/{imported}/rows/{film_id}/result");
    let saved = api.success(&result_path, None).await["saved"]["id"].clone();
    fs::remove_dir(&film_poster).unwrap();
    fs::write(&film_nfo, "manual edit after first writeback").unwrap();
    let retry = api.preview_writeback(imported, Some(film_id)).await;
    assert_eq!(retry["writable"], 1);

    let history = api.success("/api/history?kind=movie", None).await;
    let saved_film = history
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == "海报电影")
        .unwrap();
    assert_eq!(saved_film["year"], 2019);
    let id = saved_film["id"].as_i64().unwrap();
    let history_detail = format!("/api/history/{id}");
    assert_eq!(
        api.success(&history_detail, None).await["record"]["fields"]["title"],
        "海报电影"
    );
    api.success("/api/history/languages", None).await;
    assert_eq!(api.send("/api/history/99999", None).await.status(), 404);
    assert_eq!(
        api.send("/api/history?language=%23", None).await.status(),
        503
    );
    let cleared = api.success("/api/history/clear", Some(Value::Null)).await;
    assert!(cleared.as_i64().unwrap() > 0);
    assert_eq!(api.success("/api/history", None).await, json!([]));
    assert_eq!(api.success("/api/history/languages", None).await, json!([]));
    assert_eq!(api.send(&history_detail, None).await.status(), 404);
    assert_eq!(api.success("/api/history/clear", Some(Value::Null)).await, 0);

    let finished = api.apply_writeback(imported, Some(film_id), &retry).await;
    let film = finished["rows"].as_array().unwrap().iter().find(|row| row["id"] == film_id).unwrap();
    assert_eq!(film["writeback"], "done");
    assert_eq!(film["output"]["written"], 5);
    assert_eq!(api.success(&result_path, None).await["saved"]["id"], saved);
    assert_eq!(fs::read_to_string(&film_nfo).unwrap(), "manual edit after first writeback");
    assert_eq!(fs::read_to_string(backup).unwrap(), "original film metadata");
    assert!(film_poster.is_file());

    let unlinked = rows.iter().find(|row| row["query"] == "emby:35").unwrap();
    assert_eq!(unlinked["writeback"], "failed");
    assert!(unlinked["has_result"].as_bool().unwrap());
    let missing = api.preview_writeback(imported, unlinked["id"].as_i64()).await;
    assert_eq!(missing["writable"], 0);
    assert!(missing["files"][0]["error"].as_str().unwrap().contains("路径"));
    let video = rows.iter().find(|row| row["query"] == "emby:40").unwrap();
    assert_eq!(video["stage"], "success");
    let video_exports = std::path::Path::new(video["output"]["directory"].as_str().unwrap());
    let nfo = fs::read_to_string(video_exports.join("2024旅行.nfo")).unwrap();
    assert!(nfo.contains("<title>家庭旅行</title>"));
    assert!(nfo.contains("自己的旅行记录"));
    api.success("/api/batches", None).await;
    api.success(&format!("/api/batches?before={imported}"), None)
        .await;

    let before = api.success("/api/batches", None).await;
    assert_eq!(
        api.send("/api/emby/items/31/scrape", Some(json!("episode")))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(api.success("/api/batches", None).await, before);

    for (item, scope) in [("30", "movie"), ("40", "movie"), ("31", "series"), ("34", "episode")] {
        let batch = api
            .success(&format!("/api/emby/items/{item}/scrape"), Some(json!(scope)))
            .await
            .as_i64()
            .unwrap();
        let finished = api.finish(batch).await;
        let rows = finished["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["stage"], "success");
        let row = rows[0]["id"].as_i64().unwrap();
        let result = api
            .success(&format!("/api/batches/{batch}/rows/{row}/result"), None)
            .await;
        let record = &result["record"];
        assert_eq!(record["work_id"], format!("Emby-{item}"));
        let preview = api.preview_writeback(batch, Some(row)).await;
        let written = api.apply_writeback(batch, Some(row), &preview).await;
        assert_eq!(written["rows"][0]["writeback"], "done");
        let directory = Path::new(written["rows"][0]["output"]["directory"].as_str().unwrap());
        if scope == "episode" {
            assert_eq!(record["scope"], "episode");
            assert_eq!(record["fields"]["episode_number"], 1);
            assert_eq!(record["fields"]["description"], "已有中文译文");
            assert_eq!(record["fields"]["release_date"], "2022-02-02");
            assert_eq!(record["fields"]["rating"], json!({"value":8.1,"scale":10}));
            assert_eq!(record["field_sources"]["rating"], "Emby");
            let raw = &record["sources"][0]["data"]["item"];
            assert_eq!(raw["PremiereDate"], "2022-02-02T00:00:00Z");
            assert_eq!(raw["DateCreated"], "2022-03-01T00:00:00Z");
            assert_eq!(raw["LockData"], false);
            assert_eq!(raw["VideoCodec"], "hevc");
            assert!(record["sources"][0]["data"]["children"].as_array().unwrap().is_empty());
            assert_eq!(directory, season_one);
            assert!(fs::read_to_string(directory.join("S01E01.nfo")).unwrap().contains("<episodedetails>"));
            assert!(directory.join("S01E01-thumb.jpg").is_file());
            assert!(!directory.join("tvshow.nfo").exists());
            assert_eq!(api.success("/api/history?kind=series", None).await[0]["scope"], "episode");
        } else if scope == "series" {
            assert_eq!(record["fields"]["episodes"].as_array().unwrap().len(), 2);
            assert_eq!(directory, series);
            assert!(directory.join("tvshow.nfo").exists());
            assert!(directory.join("poster.jpg").exists());
            assert!(fs::read_to_string(season_one.join("S01E01.nfo")).unwrap().contains("第一集"));
            assert!(fs::read_to_string(season_two.join("S01E01.nfo")).unwrap().contains("回归"));
            assert!(!directory.join("S01E01.nfo").exists());
        } else {
            assert_eq!(rows[0]["kind"], "movie");
            let (title, basename) = if item == "40" {
                ("家庭旅行", "2024旅行.nfo")
            } else {
                ("海报电影", "海报电影.nfo")
            };
            assert_eq!(record["fields"]["title"], title);
            assert!(directory.join(basename).exists());
            if item == "30" {
                assert_eq!(record["fields"]["rating"], json!({"value":8.5,"scale":10}));
                assert_eq!(record["fields"]["imdb_rating"], json!({"value":8.6,"scale":10,"count":12345}));
                assert_eq!(record["fields"]["tags"], json!(["剧情"]));
                let nfo = fs::read_to_string(directory.join(basename)).unwrap();
                assert!(nfo.contains("<uniqueid type=\"bangumi\">303162</uniqueid>"));
                assert!(nfo.contains("<value>8.6</value><votes>12345</votes>"));
            }
        }
    }
    assert!(!config.join("outputs").exists());

    api.success("/api/settings/emby", Some(emby_options)).await;
    let video_batch = api
        .success("/api/folder-batches", Some(json!(["library/家庭旅行.mov"])))
        .await["id"]
        .as_i64()
        .unwrap();
    let video_view = api.finish(video_batch).await;
    assert_eq!(video_view["rows"][0]["stage"], "success");
    assert_eq!(video_view["rows"][0]["title"], "家庭旅行");
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
