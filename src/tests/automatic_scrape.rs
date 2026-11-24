use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool, postgres::PgConnectOptions};
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};

use super::*;
use crate::{BangumiSettings, EmbySettings, OutputSettings, TmdbSettings};

#[derive(Default)]
struct Library {
    items: HashMap<String, Value>,
    sources: HashMap<String, Value>,
    rejected_fields: HashMap<String, &'static str>,
    item_errors: HashMap<String, StatusCode>,
    write_errors: HashMap<String, StatusCode>,
    source_errors: HashMap<String, StatusCode>,
    edit_reads: Vec<String>,
    native_sidecars: bool,
    public_info: Option<Value>,
    readiness_items: Option<Value>,
    artwork: HashMap<String, &'static [u8]>,
    writes: Vec<String>,
    refreshes: Vec<String>,
}

struct Fixture {
    directory: TempDir,
    library: Arc<Mutex<Library>>,
    base: reqwest::Url,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let library = Arc::new(Mutex::new(Library::default()));
        let app = Router::new()
            .fallback(serve)
            .with_state(Arc::clone(&library));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            directory,
            library,
            base,
            server,
        }
    }

    fn item(&self, item: Value) {
        let id = item["Id"].as_str().unwrap().to_owned();
        self.library.lock().unwrap().items.insert(id, item);
    }

    fn saved(&self, id: &str) -> Value {
        self.library.lock().unwrap().items[id].clone()
    }

    fn source(&self, path: &str, result: Value) {
        self.library
            .lock()
            .unwrap()
            .sources
            .insert(path.into(), result);
    }

    fn episode_catalog(&self, subject: u32, episodes: &[Value]) {
        self.source(
            &format!("/bangumi/episodes?subject_id={subject}"),
            json!({"offset":0, "total":episodes.len(), "data":episodes}),
        );
        for episode in episodes {
            self.source(
                &format!("/bangumi/episodes/{}", episode["id"]),
                episode.clone(),
            );
        }
    }

    fn collector(&self, database: Option<&str>) -> Collector {
        let mut collector = match database {
            Some(database) => Collector::from_database_url(database),
            None => Collector::from_environment(None),
        }
        .unwrap();
        let base = self.base.clone();
        collector.client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::custom(move |url| {
                (url.host_str() != Some("127.0.0.1")).then(|| base.clone())
            }))
            .build()
            .unwrap();
        let root = self.directory.path();
        let emby_file = root.join("emby.json");
        fs::write(
            &emby_file,
            json!({"enabled":true, "server":self.base.join("emby/").unwrap(), "api_key":"fixture"})
                .to_string(),
        )
        .unwrap();
        let output = OutputSettings::load(root.join("output.json"), None).unwrap();
        output
            .save(
                serde_json::from_value(json!({
                    "emby_paths":[{"server":"/media","local":root}]
                }))
                .unwrap(),
            )
            .unwrap();
        collector
            .with_emby_settings(Arc::new(EmbySettings::load(emby_file).unwrap()))
            .with_output_settings(Arc::new(output))
            .with_bangumi_settings(Arc::new(BangumiSettings::fixture(
                self.base.join("bangumi/").unwrap(),
            )))
            .with_tmdb_settings(Arc::new(TmdbSettings::fixture(
                self.base.join("tmdb/").unwrap(),
            )))
    }

    fn service(&self, central: bool, mapping: &str) -> AutoScrape {
        AutoScrape {
            spool: self.directory.path().to_owned(),
            url: None,
            roots: vec!["/media".into()],
            private_roots: vec!["/private".into()],
            central_metadata: central,
            season_map: SeasonMap::parse(mapping).unwrap(),
            database_season_map: false,
            emby_timezone: TimeZone::get("Asia/Shanghai").unwrap(),
            skip_specials: true,
        }
    }

    fn event(&self, name: &str) -> Event {
        let path = self.directory.path().join(name);
        fs::write(&path, "").unwrap();
        Event::parse(path).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(State(library): State<Arc<Mutex<Library>>>, request: Request) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let parts: Vec<_> = uri.path().trim_matches('/').split('/').collect();
    let url = reqwest::Url::parse(&format!("http://fixture{uri}")).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    let payload = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let mut library = library.lock().unwrap();

    let result = match parts.as_slice() {
        ["emby", "Items"] => {
            if query.get("Limit").is_some_and(|limit| limit == "1")
                && let Some(value) = &library.readiness_items
            {
                return Json(value.clone()).into_response();
            }
            if let Some(status) = query.get("Ids").and_then(|id| library.item_errors.get(id)) {
                return status.into_response();
            }
            let rows: Vec<_> = match (query.get("Ids"), query.get("ParentId")) {
                (Some(id), _) => library.items.get(id).into_iter().collect(),
                (_, Some(parent)) => library
                    .items
                    .values()
                    .filter(|item| item["ParentId"] == parent.as_str())
                    .collect(),
                _ => Vec::new(),
            };
            Some(json!({"Items":rows,"TotalRecordCount": rows.len()}))
        }
        ["emby", "System", "Info", "Public"] => Some(
            library
                .public_info
                .clone()
                .unwrap_or_else(|| json!({"Id":"100"})),
        ),
        ["emby", "Users", "Query"] => {
            Some(json!({"Items":[{"Id":"5","Policy":{"IsAdministrator":true}}]}))
        }
        ["emby", "Library", "VirtualFolders"] => Some(json!([
            {"Locations":["/media"],"LibraryOptions":{"SaveLocalMetadata":library.native_sidecars,"MetadataSavers":[]}}
        ])),
        ["emby", "Users", "5", "Items", id] => {
            library.edit_reads.push((*id).to_owned());
            library.items.get(*id).cloned()
        }
        ["emby", "Items", id] if method == Method::POST => {
            if let Some(status) = library.write_errors.get(*id) {
                return status.into_response();
            }
            let mut updated: Value = serde_json::from_slice(&payload).unwrap();
            if let Some(&field) = library.rejected_fields.get(*id) {
                updated[field] = library.items[*id][field].clone();
            }
            library.items.insert((*id).into(), updated);
            library.writes.push((*id).into());
            return StatusCode::NO_CONTENT.into_response();
        }
        ["emby", "Items", id, "Refresh"] if method == Method::POST => {
            assert_eq!(query["ReplaceAllMetadata"], "false");
            assert_eq!(query["ReplaceAllImages"], "false");
            library.refreshes.push((*id).into());
            return StatusCode::NO_CONTENT.into_response();
        }
        ["emby", "Items", id, "Images", "Primary"] => {
            assert_eq!(query["EnableImageEnhancers"], "false");
            return match library.artwork.get(*id) {
                Some(bytes) => {
                    ([(axum::http::header::CONTENT_TYPE, "image/png")], *bytes).into_response()
                }
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        ["emby", "Items", _, "Images"] => {
            Some(json!([{"ImageType":"Primary"},{"ImageType":"Backdrop"}]))
        }
        ["ai"] if method == Method::POST => {
            let input: Value = serde_json::from_slice(&payload).unwrap();
            let evidence: Value =
                serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(input["model"], "fixture");
            let title = evidence["title"].as_str().unwrap();
            let count = evidence["candidates"].as_array().unwrap().len();
            library
                .sources
                .get(&format!("/ai?title={title}&candidates={count}"))
                .cloned()
                .map(|reply| json!({"choices":[{"message":{"content":reply.to_string()}}]}))
        }
        _ => {
            if let Some(status) = library.source_errors.get(uri.path()) {
                return status.into_response();
            }
            let detailed = ["subject_id", "query"].into_iter().find_map(|key| {
                query
                    .get(key)
                    .map(|value| format!("{}?{key}={value}", uri.path()))
            });
            detailed
                .as_ref()
                .and_then(|key| library.sources.get(key))
                .or_else(|| library.sources.get(uri.path()))
                .cloned()
        }
    };
    match result {
        Some(value) => Json(value).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn bangumi_episode(id: u32, subject: u32, index: u32, date: &str, title: &str) -> Value {
    json!({
        "id":id, "subject_id":subject, "type":0, "sort":index, "ep":index,
        "airdate":date, "name":title, "name_cn":title, "desc":"来自分集目录的简介", "duration":"24m"
    })
}

fn episode(id: &str, filename: &str, season: u32, index: u32, date: &str) -> Value {
    json!({
        "Id":id,"Type":"Episode","Name":format!("第 {index} 集"),"SeriesId":"10",
        "SeriesName":"Fixture Show","ParentIndexNumber":season,"IndexNumber":index,
        "PremiereDate":date,"Path":format!("/media/show/{filename}"),"ProviderIds":{},"People":[],
    })
}

#[tokio::test]
async fn mapped_intake_repairs_episode_nfo_and_refreshes_without_replacing_metadata() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, r#"{"10":{"2":[12]}}"#);
    fixture.episode_catalog(12, &[bangumi_episode(82, 12, 1, "2026-01-03", "归来")]);
    fixture.item(episode("21", "S02E01.mkv", 2, 1, "2026-01-03"));

    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("S02E01.mkv"), b"media").unwrap();
    let nfo = show.join("S02E01.nfo");
    let original = "<episodedetails><title>第 1 集</title><season>2</season><episode>1</episode><plot>原简介</plot></episodedetails>";
    fs::write(&nfo, original).unwrap();
    let event = fixture.event("Episode-21.event");

    let result = service.process(&collector, &event).await.unwrap();
    assert!(matches!(result, Outcome::Written(1)));
    let repaired = fs::read_to_string(&nfo).unwrap();
    assert!(repaired.contains("<title>归来</title>"));
    assert!(repaired.contains("<plot>原简介</plot>"));
    assert_eq!(fs::read(show.join("S02E01.mkv")).unwrap(), b"media");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(0)
    ));
    let library = fixture.library.lock().unwrap();
    assert_eq!(library.refreshes, ["21", "21"]);
    assert!(library.writes.is_empty());
}

#[tokio::test]
async fn intake_skips_ineligible_items_and_retains_events_on_missing_or_unsourced_items() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    fixture.item(json!({"Id":"1","Type":"Movie","Path":"/media/m.mkv","IsLocked":true}));
    fixture.item(json!({"Id":"2","Type":"Movie","Path":"/external/m.mkv"}));
    fixture.item(json!({"Id":"3","Type":"Video","Path":"/media/home.mkv"}));
    fixture.item(episode("4", "special.mkv", 0, 1, "2026-01-03"));
    fixture.item(json!({"Id":"5","Type":"Season","Path":"/media/show","IndexNumber":1}));
    fixture.item(json!({"Id":"6","Type":"Episode","LocationType":"Virtual"}));
    fixture.item(json!({"Id":"7","Type":"Movie"}));
    fixture.item(json!({"Id":"8","Type":"Video","Path":"/private/untitled.mkv"}));
    fixture.item(json!({"Id":"9","Type":"Series","Path":"/private/show"}));
    fixture.item(json!({"Id":"42","Type":"Video","Path":"/private/ABC-123/video.mkv"}));
    fixture.item(json!({"Id":"43","Type":"Movie","Name":"资料未匹配","Path":"/media/unknown.mkv"}));
    for name in [
        "Movie-1.event",
        "Movie-2.event",
        "Video-3.event",
        "Episode-4.event",
        "Season-5.backfill",
        "Episode-6.event",
        "Series-9.event",
    ] {
        let event = fixture.event(name);
        assert!(
            matches!(
                service.process(&collector, &event).await.unwrap(),
                Outcome::Skipped
            ),
            "{name}"
        );
    }
    let missing = fixture.event("Movie-91.event");
    assert!(matches!(
        service.process(&collector, &missing).await.unwrap(),
        Outcome::MissingItem
    ));
    for (name, message) in [
        ("Movie-7.event", "媒体路径"),
        ("Video-8.event", "识别番号"),
        ("Video-42.event", "未获得私人影片资料"),
        ("Movie-43.event", "TMDB：来源没有找到此条目"),
    ] {
        let event = fixture.event(name);
        let error = service.process(&collector, &event).await.err().unwrap();
        assert!(
            !error.is_empty() && error.contains(message),
            "{name}: {error}"
        );
        assert!(event.path.exists());
    }
    let library = fixture.library.lock().unwrap();
    assert!(library.writes.is_empty());
    assert!(library.refreshes.is_empty());
}

#[tokio::test]
async fn intake_reads_live_events_in_numeric_order_and_checks_emby_readiness() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    for name in [
        "Episode-10.event",
        "Movie-9.event",
        "Episode-1.backfill",
        "Episode-2.backfill-review",
        "Unknown-3.event",
        "Movie-4.backfill",
        "Series-+.event",
        "Season-99.event",
    ] {
        fs::write(service.spool.join(name), "").unwrap();
    }
    fs::create_dir(service.spool.join("Movie-1.event")).unwrap();
    let events = service.events().await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["9", "10", "99", "1"]
    );

    let emby = collector
        .emby_settings()
        .unwrap()
        .automatic_connection(None)
        .unwrap();
    let check = || emby.automatic_ready(&collector.client);
    check().await.unwrap();
    fixture.library.lock().unwrap().public_info = Some(json!({"Id":"100","IsShuttingDown":true}));
    assert!(check().await.unwrap_err().contains("尚未完成启动"));
    fixture.library.lock().unwrap().public_info = Some(json!({"Id":"bad/id"}));
    assert!(check().await.unwrap_err().contains("尚未完成启动"));
    assert!(emby.automatic_server_id(&collector.client).await.is_err());
    {
        let mut library = fixture.library.lock().unwrap();
        library.public_info = None;
        library.readiness_items = Some(json!({"Items": null}));
    }
    assert!(
        check()
            .await
            .unwrap_err()
            .contains("媒体库读取未返回有效结果")
    );
}

#[tokio::test]
async fn episode_writeback_publishes_localized_nfo_and_retries_invalid_artwork() {
    use crate::output::{WritebackJob, WritebackState, plan_writeback, prepare_writeback};

    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let emby = collector
        .emby_settings()
        .unwrap()
        .automatic_connection(None)
        .unwrap();
    let paths = collector.output_settings().unwrap().media_paths();
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("episode.mkv"), "original video").unwrap();
    let mut child = episode("21", "episode.mkv", 1, 1, "2026-01-03");
    child["ImageTags"]["Primary"] = json!("still");
    child["SeriesName"] = Value::Null;
    let source = json!({
        "provider":"Emby",
        "url":fixture.base.join("emby/web/index.html#!/item?id=10").unwrap(),
        "data":{
            "item":{"Id":"10","Type":"Series","Path":"/media/show","ImageTags":{"Primary":"poster"}},
            "children":[child]
        }
    });
    let record = json!({
        "media_type":"series",
        "fields":{
            "title":"剧集标题",
            "episodes":[{"id":"21","title":"分集译名","description":"分集简介"}]
        },
        "sources":[source]
    });
    let preview = |state: &WritebackState| {
        let (files, _, _) = plan_writeback(&paths, &source["data"], &record, state);
        WritebackJob {
            id: "fixture".into(),
            owner: "fixture".into(),
            stage: "fixture".into(),
            files,
        }
    };
    fixture
        .library
        .lock()
        .unwrap()
        .artwork
        .insert("21".into(), b"invalid image bytes");
    let error = write_automatic(&collector.client, &emby, &paths, &record, false)
        .await
        .unwrap_err();
    assert!(error.contains("无法下载 Emby 图片"), "{error}");
    assert!(!show.join("tvshow.nfo").exists());

    let state = WritebackState::default();
    let no_mount = OutputSettings::load(fixture.directory.path().join("unmapped.json"), None)
        .unwrap()
        .media_paths();
    let staged = prepare_writeback(
        &collector.client,
        Some(&emby),
        &no_mount,
        &preview(&state),
        &record,
        &state,
    )
    .await;
    let (blocked, report) = staged.publish("mapping-changed", WritebackState::default());
    assert_eq!(report.failed, 4);
    assert!(
        serde_json::to_value(blocked).unwrap()["files"]["nfo\n/media/show"]["error"]
            .as_str()
            .unwrap()
            .contains("写回映射已变更")
    );
    assert!(!show.join("tvshow.nfo").exists());

    let staged = prepare_writeback(
        &collector.client,
        Some(&emby),
        &paths,
        &preview(&state),
        &record,
        &state,
    )
    .await;
    let (state, report) = staged.publish("first", state);
    assert_eq!(report.failed, 2);
    assert_eq!(report.written, 2);
    assert!(!show.join("episode-thumb.png").exists());
    let nfo = show.join("episode.nfo");
    let xml = fs::read_to_string(&nfo).unwrap();
    assert!(xml.contains("<title>分集译名</title>"), "{xml}");
    assert!(xml.contains("<plot>分集简介</plot>"), "{xml}");
    assert!(xml.contains("<showtitle>剧集标题</showtitle>"), "{xml}");
    let failures = serde_json::to_value(&state).unwrap();
    assert!(
        failures["files"]["poster\n/media/show"]["error"]
            .as_str()
            .unwrap()
            .contains("无法下载 Emby 图片")
    );
    assert!(
        failures["files"]["still\n/media/show/episode.mkv"]["error"]
            .as_str()
            .unwrap()
            .contains("图片格式无效")
    );

    let png = b"\x89PNG\r\n\x1a\npixels";
    {
        let mut library = fixture.library.lock().unwrap();
        library.artwork.insert("10".into(), png);
        library.artwork.insert("21".into(), png);
    }
    fs::write(&nfo, "manual revision").unwrap();
    let staged = prepare_writeback(
        &collector.client,
        Some(&emby),
        &paths,
        &preview(&state),
        &record,
        &state,
    )
    .await;
    let (_, report) = staged.publish("retry", state);
    assert_eq!(report.failed, 0);
    assert_eq!(report.written, 4);
    for name in ["poster.png", "episode-thumb.png"] {
        assert_eq!(fs::read(show.join(name)).unwrap(), png);
    }
    assert_eq!(fs::read_to_string(nfo).unwrap(), "manual revision");
    assert_eq!(
        fs::read_to_string(show.join("episode.mkv")).unwrap(),
        "original video"
    );
}

#[tokio::test]
async fn manual_anime_repairs_validate_the_field_and_confirm_saved_metadata() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let emby = collector
        .emby_settings()
        .unwrap()
        .automatic_connection(None)
        .unwrap();
    let mut original = episode("21", "episode.mkv", 1, 1, "2026-01-03");
    original["Path"] = json!("/media/番剧/episode.mkv");
    fixture.item(original.clone());
    for (field, text) in [("Path", "/other"), ("Overview", " ")] {
        let error = emby
            .repair_anime_metadata(&collector.client, &original, field, text)
            .await
            .unwrap_err();
        assert!(error.contains("修复字段或文字无效"));
    }

    fixture
        .library
        .lock()
        .unwrap()
        .rejected_fields
        .insert("21".into(), "LockedFields");
    let error = emby
        .repair_anime_metadata(&collector.client, &original, "Name", "核对后的标题")
        .await
        .unwrap_err();
    assert!(error.contains("没有确认补全写入"));
    assert_eq!(fixture.saved("21")["Name"], "核对后的标题");
    assert!(fixture.saved("21")["LockedFields"].is_null());

    fixture.library.lock().unwrap().rejected_fields.clear();
    fixture.item(original.clone());
    assert!(
        emby.repair_anime_metadata(&collector.client, &original, "Name", "核对后的标题")
            .await
            .unwrap()
    );
    assert_eq!(
        fixture.saved("21")["LockedFields"],
        json!(["Name", "SortName"])
    );
    assert_eq!(fixture.saved("21")["ProviderIds"], original["ProviderIds"]);
}

#[derive(Default)]
struct MemoryPersistence {
    season: Option<(&'static str, &'static str, i32, Vec<u32>)>,
    episodes: Vec<Value>,
    airdate_verified: bool,
    archive_error: Option<&'static str>,
    snapshot_error: Option<&'static str>,
    saved: Mutex<HashMap<(String, String), Value>>,
}

impl MemoryPersistence {
    fn saved(&self, server: &str, item: &str) -> Option<Value> {
        self.saved
            .lock()
            .unwrap()
            .get(&(server.to_owned(), item.to_owned()))
            .cloned()
    }
}

impl ScrapePersistence for MemoryPersistence {
    async fn ensure_available(&self) -> Result<(), String> {
        Ok(())
    }

    async fn verified_bangumi_season(
        &self,
        server: &str,
        series: &str,
        season: i32,
    ) -> Result<Option<Vec<u32>>, String> {
        Ok(self
            .season
            .as_ref()
            .filter(|(s, id, n, _)| (*s, *id, *n) == (server, series, season))
            .map(|(_, _, _, subjects)| subjects.clone()))
    }

    async fn archived_bangumi_episodes(&self, subjects: &[u32]) -> Result<Vec<Value>, String> {
        assert_eq!(subjects, self.season.as_ref().unwrap().3);
        self.archive_error
            .map_or_else(|| Ok(self.episodes.clone()), |error| Err(error.into()))
    }

    async fn verified_episode_airdate(
        &self,
        server: &str,
        item: &Value,
        bangumi_id: u32,
        local: Date,
        official: Date,
    ) -> Result<bool, String> {
        assert_eq!(server, "100");
        assert_eq!(item["Id"], "21");
        assert_eq!(bangumi_id, 81);
        assert_eq!(local.to_string(), "2026-01-02");
        assert_eq!(official.to_string(), "2026-01-03");
        Ok(self.airdate_verified)
    }

    async fn save_emby_item(
        &self,
        server: &str,
        item: &str,
        document: &Value,
    ) -> Result<(), String> {
        self.saved
            .lock()
            .unwrap()
            .insert((server.to_owned(), item.to_owned()), document.clone());
        Ok(())
    }

    async fn save_snapshot(&self, _: crate::LookupResponse) -> Option<String> {
        self.snapshot_error.map(str::to_owned)
    }
}

struct Database {
    admin: PgPool,
    pool: PgPool,
    url: reqwest::Url,
    schema: String,
}

impl Database {
    async fn new() -> Self {
        let database = env::var("COLLECT_METADATA_TEST_POSTGRES").unwrap();
        let options: PgConnectOptions = database.parse().unwrap();
        let admin = PgPool::connect_with(options.clone()).await.unwrap();
        let schema = format!("auto_workflow_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .unwrap();
        let pool = PgPool::connect_with(options.options([("search_path", &schema)]))
            .await
            .unwrap();
        let mut url = reqwest::Url::parse(&database).unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        Self {
            admin,
            pool,
            url,
            schema,
        }
    }

    async fn close(self) {
        self.pool.close().await;
        sqlx::query(AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn auto_backfill_keeps_existing_values_and_uses_verified_part_and_sequel_metadata() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let persistence = MemoryPersistence::default();
    let service = fixture.service(true, r#"{"10":{"2":[12,13]}}"#);
    let mut parent = json!({
        "Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show","ProviderIds":{"Bangumi":"11"}
    });
    fixture.item(parent.clone());
    for (id, title, date) in [
        (11, "First", "2025-01-01"),
        (12, "Second", "2026-01-03"),
        (13, "Third", "2026-07-01"),
    ] {
        fixture.source(&format!("/bangumi/subjects/{id}"), json!({
            "id":id, "type":2, "platform":"TV", "name":title, "date":date, "summary":format!("{title} season synopsis")
        }));
    }
    fixture.item(json!({
        "Id":"22","Type":"Season","Path":"/media/show/Season 02","Name":"我的季标题","IndexNumber":2,
        "SeriesId":"10","ImageTags":{"Primary":"old-tag"},"ProviderIds":{}
    }));
    let mapped = fixture.event("Season-22.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &mapped, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    let saved = fixture.saved("22");
    assert_eq!(saved["Name"], "我的季标题");
    assert_eq!(saved["ImageTags"]["Primary"], "old-tag");
    assert_eq!(
        saved["Overview"],
        "Second season synopsis\n\nThird season synopsis"
    );
    assert!(saved["ProviderIds"]["Bangumi"].is_null());
    let archived = persistence.saved("100", "22").unwrap();
    assert_eq!(archived["Overview"], saved["Overview"]);
    assert_eq!(archived["ImageTags"], saved["ImageTags"]);

    parent["ProviderIds"]["Tmdb"] = json!("999");
    fixture.item(parent);
    fixture.item(json!({
        "Id":"23","Type":"Season","Path":"/media/show/Season 03","Name":"第三季","IndexNumber":3,
        "SeriesId":"10","ProviderIds":{}
    }));
    let episodes = [
        bangumi_episode(131, 13, 1, "2026-07-01", "起点"),
        bangumi_episode(132, 13, 2, "2026-07-08", "旅途"),
        bangumi_episode(133, 13, 3, "2026-07-15", "终点"),
    ];
    fixture.episode_catalog(13, &episodes);
    for (id, entry) in ["31", "32", "33"].into_iter().zip(episodes) {
        let mut item = episode(
            id,
            "e.mkv",
            3,
            entry["sort"].as_u64().unwrap() as u32,
            entry["airdate"].as_str().unwrap(),
        );
        item["ParentId"] = json!("23");
        fixture.item(item);
    }
    fixture.source(
        "/bangumi/subjects/11/subjects",
        json!([{"id":13,"type":2,"relation":"续集"}]),
    );
    fixture.source(
        "/bangumi/subjects/13/subjects",
        json!([{"id":11,"type":2,"relation":"前传"}]),
    );
    let sequel = fixture.event("Season-23.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &sequel, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    assert_eq!(fixture.saved("23")["Overview"], "Third season synopsis");
    assert_eq!(fixture.saved("23")["ProviderIds"]["Bangumi"], "13");

    let mut unmapped = episode("40", "unmapped.mkv", 4, 1, "2026-12-01");
    unmapped["SeriesId"] = Value::Null;
    fixture.item(unmapped);
    let missing = fixture.event("Episode-40.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &missing, &persistence)
            .await
            .unwrap(),
        Outcome::NoVerifiedSource
    ));
    assert!(persistence.saved("100", "40").is_none());
    assert_eq!(fixture.library.lock().unwrap().writes, ["22", "23"]);
}

fn airdate_review_fixture(fixture: &Fixture) -> (AutoScrape, Value) {
    let mut service = fixture.service(true, r#"{"10":{"2":[999]}}"#);
    service.database_season_map = true;
    fixture.item(json!({"Id":"10","Type":"Series","Name":"Fixture Show","Path":"/media/show"}));
    fixture.episode_catalog(
        12,
        &[
            bangumi_episode(81, 12, 1, "2026-01-03", "归来"),
            bangumi_episode(82, 12, 2, "2026-01-10", "出发"),
        ],
    );
    let mut item = episode("21", "S02E01.mkv", 2, 1, "2026-01-01T16:00:00Z");
    item["Name"] = json!("人工核对标题");
    item["Overview"] = json!("原简介继续保留");
    item["ImageTags"]["Primary"] = json!("existing");
    item["ProviderIds"]["Tvdb"] = json!("901");
    fixture.item(item.clone());
    (service, item)
}

#[tokio::test]
async fn mapped_episode_uses_verified_airdate_or_timezone_before_writing() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let (service, mut item) = airdate_review_fixture(&fixture);
    let mut persistence = MemoryPersistence {
        season: Some(("100", "10", 2, vec![12])),
        episodes: vec![
            bangumi_episode(81, 12, 1, "2026-01-03", "归来"),
            bangumi_episode(82, 12, 2, "2026-01-10", "出发"),
        ],
        ..MemoryPersistence::default()
    };
    let event = fixture.event("Episode-21.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::AwaitingAirdateReview
    ));
    assert_eq!(fixture.saved("21"), item);
    assert!(persistence.saved("100", "21").is_none());

    persistence.airdate_verified = true;
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    item["ProviderIds"]["Bangumi"] = json!("81");
    let saved = fixture.saved("21");
    for field in [
        "Name",
        "Overview",
        "ImageTags",
        "PremiereDate",
        "ProviderIds",
    ] {
        assert_eq!(saved[field], item[field], "{field}");
    }
    assert_eq!(persistence.saved("100", "21").unwrap(), saved);
    let writes = fixture.library.lock().unwrap().writes.len();
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    assert_eq!(fixture.library.lock().unwrap().writes.len(), writes);

    let mut boundary = episode("22", "S02E02.mkv", 2, 2, "2026-01-09T16:00:00Z");
    boundary["Name"] = json!("已有标题");
    fixture.item(boundary);
    let event = fixture.event("Episode-22.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    assert_eq!(fixture.saved("22")["ProviderIds"]["Bangumi"], "82");
}

#[tokio::test]
async fn automatic_intake_uses_the_live_catalog_when_the_archive_is_unavailable() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let (service, complete) = airdate_review_fixture(&fixture);
    let persistence = MemoryPersistence {
        season: Some(("100", "10", 2, vec![12])),
        archive_error: Some("catalog unavailable"),
        snapshot_error: Some("snapshot write failed"),
        ..MemoryPersistence::default()
    };
    let event = fixture.event("Episode-21.event");
    let error = service
        .process_with(&collector, &event, &persistence)
        .await
        .err()
        .unwrap();
    assert_eq!(error, "catalog unavailable");
    assert_eq!(fixture.saved("21"), complete);
    assert!(persistence.saved("100", "21").is_none());

    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("S02E02.mkv"), "video").unwrap();
    let mut item = episode("22", "S02E02.mkv", 2, 2, "2026-01-10");
    item["SortName"] = json!("保持原排序名");
    item["LockedFields"] = json!(["SortName"]);
    fixture.item(item);
    let event = fixture.event("Episode-22.event");
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Written(1)
    ));
    let saved = fixture.saved("22");
    assert_eq!(saved["Name"], "出发");
    assert_eq!(saved["SortName"], "保持原排序名");
    assert_eq!(saved["ProviderIds"]["Bangumi"], "82");
    assert_eq!(persistence.saved("100", "22").unwrap(), saved);
    let xml = fs::read_to_string(show.join("S02E02.nfo")).unwrap();
    assert!(xml.contains("<title>出发</title>"), "{xml}");
    assert!(fixture.library.lock().unwrap().refreshes.is_empty());
}

#[tokio::test]
#[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
async fn postgres_auto_intake_requires_verified_airdate_before_updating_mapped_episode_identity() {
    let fixture = Fixture::new().await;
    let database = Database::new().await;
    let collector = fixture.collector(Some(database.url.as_str()));
    let store = collector.require_store().await.unwrap();
    sqlx::raw_sql(include_str!("../../tools/anime-catalog/schema.sql"))
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO collect_metadata_anime_verified_seasons
            (server_id, emby_series_id, season_number, series_title, bangumi_subject_ids, evidence)
         VALUES ('100','10',2,'Fixture Show',ARRAY[12],'[]');
         INSERT INTO collect_metadata_anime_subjects
            (bangumi_id,title,format,is_adult,expected_episodes,archive_version)
         VALUES (12,'Second','TV',false,2,'fixture');
         INSERT INTO collect_metadata_anime_episodes
            (bangumi_episode_id,bangumi_id,kind,source_number,airdate,archive_version)
         VALUES (81,12,0,1,'2026-01-03','fixture'), (82,12,0,2,'2026-01-10','fixture');",
    )
    .execute(&database.pool)
    .await
    .unwrap();

    let (service, mut item) = airdate_review_fixture(&fixture);
    let event = fixture.event("Episode-21.backfill");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::AwaitingAirdateReview
    ));
    assert_eq!(fixture.saved("21"), item);
    assert!(store.emby_item("100", "21").await.unwrap().is_none());
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    assert_eq!(state["jobs"][0]["status"], "review");
    assert_eq!(state["jobs"][0]["error_kind"], "airdate_review");
    assert!(event.path.with_extension("backfill-review").exists());

    sqlx::query(
        "INSERT INTO collect_metadata_anime_verified_episode_airdates
            (server_id,emby_episode_id,emby_series_id,season_number,episode_number,
             bangumi_episode_id,emby_local_airdate,official_airdate,emby_title,tvdb_id,evidence_url)
         VALUES ('100','21','10',2,1,81,'2026-01-02','2026-01-03','人工核对标题','901','https://example.org/episode')",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Enriched
    ));
    item["ProviderIds"]["Bangumi"] = json!("81");
    assert_eq!(fixture.saved("21")["ProviderIds"], item["ProviderIds"]);
    assert_eq!(fixture.saved("21")["Name"], item["Name"]);
    assert_eq!(fixture.saved("21")["Overview"], item["Overview"]);
    let archived = store.emby_item("100", "21").await.unwrap().unwrap();
    assert_eq!(archived["ProviderIds"], item["ProviderIds"]);
    assert_eq!(archived["Overview"], item["Overview"]);
    let writes = fixture.library.lock().unwrap().writes.len();
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Enriched
    ));
    assert_eq!(fixture.library.lock().unwrap().writes.len(), writes);

    let mut boundary = episode("22", "S02E02.mkv", 2, 2, "2026-01-09T16:00:00Z");
    boundary["Name"] = json!("已有标题");
    fixture.item(boundary);
    let next = fixture.event("Episode-22.backfill");
    assert!(matches!(
        service.process(&collector, &next).await.unwrap(),
        Outcome::Enriched
    ));
    assert_eq!(fixture.saved("22")["ProviderIds"]["Bangumi"], "82");
    drop(collector);
    database.close().await;
}

#[tokio::test]
async fn auto_intake_tries_catalog_backup_and_verifies_tmdb_episode_credits() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let persistence = MemoryPersistence::default();
    let service = fixture.service(true, "{}");

    fixture.item(json!({
        "Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "Overview":"原有简介","ProviderIds":{"Tmdb":"1000", "Bangumi":"11"},
    }));
    fixture.source("/bangumi/subjects/11", json!({
        "id":11,"type":2,"platform":"TV","name":"Fixture Show","date":"2026-01-01",
        "summary":"备用来源简介","infobox":[{"key":"官方网站","value":"https://example.org/show"}]
    }));
    fixture.source("/bangumi/subjects/11/persons", json!([]));
    fixture.source("/bangumi/subjects/11/characters", json!([]));
    fixture.episode_catalog(11, &[bangumi_episode(83, 11, 1, "2026-01-01", "序章")]);
    let series = fixture.event("Series-10.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &series, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    let archived = persistence.saved("100", "10").unwrap();
    assert_eq!(archived["Overview"], "原有简介");
    assert_eq!(archived["ProviderIds"]["Tmdb"], "1000");

    let mut item = episode("25", "S01E01.mkv", 1, 1, "2026-01-01");
    item["Name"] = json!("保留我的标题");
    item["ProviderIds"]["Tmdb"] = json!("2001");
    fixture.item(item);
    fixture.source(
        "/tmdb/tv/1000",
        json!({
            "id":1000,"name":"Fixture Show","original_language":"ja","original_name":"Fixture Show",
        }),
    );
    fixture.source(
        "/tmdb/tv/1000/season/1/episode/1",
        json!({
            "id":2001,"episode_number":1,"season_number":1,"name":"序章",
            "air_date":"2026-01-01","overview":"公开分集简介","vote_average":8.0,
        }),
    );
    fixture.source(
        "/tmdb/tv/1000/season/1/episode/1/credits",
        json!({
            "id":2001,"cast":[{"name":"表演者","character":"角色"}],
            "guest_stars":[{"name":"客串演员","character":"朋友"}]
        }),
    );
    let existing = fixture.event("Episode-25.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &existing, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    let saved = fixture.saved("25");
    assert_eq!(saved["Name"], "保留我的标题");
    assert_eq!(saved["Overview"], "公开分集简介");
    assert_eq!(saved["People"][0]["Name"], "表演者");
    assert_eq!(saved["People"][0]["Role"], "角色");
    assert_eq!(
        persistence.saved("100", "25").unwrap()["People"],
        saved["People"]
    );

    let mut last = episode("26", "S01E02.mkv", 1, 2, "2026-01-08");
    last["IndexNumberEnd"] = json!(3);
    fixture.item(last);
    let double = fixture.event("Episode-26.event");
    let failure = service
        .process_with(&collector, &double, &persistence)
        .await
        .err()
        .unwrap();
    assert!(failure.contains("多集文件需要确认季集匹配"), "{failure}");
    assert!(persistence.saved("100", "26").is_none());
}

#[tokio::test]
#[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
async fn postgres_auto_queue_persists_completed_waiting_review_retry_and_expired_jobs() {
    let fixture = Fixture::new().await;
    let database = Database::new().await;
    let collector = fixture.collector(Some(database.url.as_str()));
    let service = fixture.service(true, r#"{"10":{"2":[12]}}"#);
    fixture.episode_catalog(
        12,
        &[
            bangumi_episode(81, 12, 1, "2026-01-03", "归来"),
            bangumi_episode(82, 12, 2, "2026-01-10", "第 2 集"),
        ],
    );
    fixture.item(json!({"Id":"10","Type":"Series","Name":"Fixture Show","Path":"/media/show"}));
    fixture.item(episode("21", "mapped.mkv", 2, 1, "2026-01-03"));
    fixture.item(episode("22", "waiting.mkv", 2, 2, "2026-01-10"));
    fixture.item(episode("23", "unmapped.mkv", 3, 1, "2026-07-10"));
    fixture.item(json!({"Id":"40","Type":"Movie"}));
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("mapped.mkv"), b"media").unwrap();

    let completed = fixture.event("Episode-21.event");
    let awaiting = fixture.event("Episode-22.event");
    let review = fixture.event("Episode-23.backfill");
    let inaccessible = fixture.event("Movie-40.event");
    let missing = fixture.event("Movie-41.event");

    for _ in 0..2 {
        service.durable_cycle(&collector).await.unwrap();
    }
    assert!(!completed.path.exists());
    assert_eq!(fixture.saved("21")["Name"], "归来");
    assert!(awaiting.path.exists());
    assert!(inaccessible.path.exists());
    assert!(missing.path.exists());
    assert!(!review.path.exists());
    assert!(review.path.with_extension("backfill-review").exists());
    let state = collector.auto_scrape_state().await.unwrap();
    assert_eq!(
        state["counts"],
        json!({"completed":1,"waiting_source":1,"review":1,"retry":2})
    );
    for (id, kind, failures) in [
        ("22", "awaiting_title", 0),
        ("23", "unverified_source", 0),
        ("40", "transient_error", 1),
        ("41", "missing_item", 0),
    ] {
        let job = state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["item_id"] == id)
            .unwrap();
        assert_eq!(job["error_kind"], kind);
        assert_eq!(job["attempts"], 1);
        assert_eq!(job["failures"], failures);
    }

    sqlx::query(
        "UPDATE collect_metadata_auto_jobs
         SET created_at=now()-interval '8 days', next_retry_at=now()-interval '1 second'
         WHERE item_id IN ('40','41')",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    assert_eq!(
        state["counts"],
        json!({
            "completed":1,"waiting_source":1,"review":1,"failed":1,"missing":1,
        })
    );
    for (event, id, status, kind) in [
        (inaccessible, "40", "failed", "retry_window_exhausted"),
        (missing, "41", "missing", "missing_item"),
    ] {
        let job = state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["item_id"] == id)
            .unwrap();
        assert_eq!(job["status"], status);
        assert_eq!(job["error_kind"], kind);
        assert_eq!(job["attempts"], 2);
        assert!(!event.path.exists());
        assert!(
            event
                .path
                .with_extension(format!("{status}-{}", job["id"]))
                .exists()
        );
    }

    let mut unsaved = episode("24", "unsaved.mkv", 2, 1, "2026-01-03");
    unsaved["Name"] = json!("不应改动的标题");
    unsaved["Overview"] = json!("");
    fixture.item(unsaved);
    fixture
        .library
        .lock()
        .unwrap()
        .rejected_fields
        .insert("24".into(), "Overview");
    let rejected = fixture.event("Episode-24.backfill");
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    let job = state["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["item_id"] == "24")
        .unwrap();
    assert_eq!(job["status"], "review");
    assert_eq!(job["error_kind"], "writeback_incomplete");
    assert!(job["last_error"].as_str().unwrap().contains("Overview"));
    assert!(rejected.path.with_extension("backfill-review").exists());
    assert_eq!(fixture.saved("24")["Name"], "不应改动的标题");
    assert_eq!(fixture.saved("24")["Overview"], "");

    drop(collector);
    database.close().await;
}

#[tokio::test]
async fn batch_worker_requeries_the_ai_title_and_requires_year_consistency() {
    use crate::batch::Claim;

    let fixture = Fixture::new().await;
    let config = fixture.directory.path().join("ai.json");
    fs::write(
        &config,
        json!({
            "url":fixture.base.join("ai").unwrap(),"model":"fixture","api_key":"test-token",
            "emby_excluded_roots":[" /private/ "],
        })
        .to_string(),
    )
    .unwrap();
    let collector = fixture
        .collector(None)
        .with_ai_config_file(&config)
        .unwrap();
    let found = json!({"id":901,"title":"Catalog title","original_title":"Catalog title",
        "release_date":"2026-01-01","overview":"From the verified catalog"});
    fixture.source(
        "/tmdb/search/movie?query=Rough title",
        json!({"results":[]}),
    );
    fixture.source(
        "/tmdb/search/movie?query=Refined title",
        json!({"results":[found.clone()]}),
    );
    fixture.source("/tmdb/movie/901", found);
    fixture.source("/bangumi/search/subjects", json!({"data":[]}));
    fixture.source(
        "/ai?title=Rough title&candidates=0",
        json!({
            "index":null,"confident":false,"search_title":" Refined title ",
        }),
    );
    fixture.source(
        "/ai?title=Rough title&candidates=1",
        json!({
            "index":0,"confident":true,"search_title":null,
        }),
    );
    assert!(collector.ai_configured());
    assert!(collector.emby_path_excluded(&json!("/private/video.mkv")));
    assert!(collector.emby_path_excluded(&json!(null)));
    assert!(!collector.emby_path_excluded(&json!("/private-other/video.mkv")));

    let mut claim = Claim {
        id: 1,
        batch: 2,
        attempt: 1,
        kind: MediaKind::Movie,
        query: "Rough title (2026)".into(),
        candidate: None,
        remember_match: true,
        use_ai: true,
        fill_missing: false,
        export_revision: 0,
        folder_root: None,
        export_plan: None,
        source_files: Vec::new(),
        writeback: None,
    };
    let matched = collector.process_claim(None, &claim).await.unwrap();
    assert_eq!(matched.stage, "success");
    let result = matched.result.unwrap();
    assert_eq!(result.record["fields"]["tmdb_id"], "901");
    assert_eq!(result.record["match_method"], "ai");

    claim.query = "Rough title (2025)".into();
    let different_year = collector.process_claim(None, &claim).await.unwrap();
    assert_eq!(different_year.stage, "choice");
    assert!(different_year.result.is_none());
    assert_eq!(different_year.candidates.unwrap()[0]["id"], "901");
    assert!(different_year.message.unwrap().contains("核对片名和年份"));

    claim.query = "No usable candidate (2026)".into();
    let empty = collector.process_claim(None, &claim).await.unwrap();
    assert!(empty.result.is_none());
    assert!(empty.message.unwrap().contains("AI 服务返回 HTTP 404"));
}

#[tokio::test]
async fn movie_intake_checks_exact_catalog_year_and_enriches_cross_source_ids() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    let movie = json!({"id":55,"title":"Catalog film","original_title":"Catalog film",
        "release_date":"2026-02-01","overview":"Catalog synopsis","runtime":105});
    let bangumi =
        json!({"id":77,"type":2,"platform":"剧场版","name":"Catalog film","date":"2026-02-01"});
    fixture.source(
        "/tmdb/search/movie",
        json!({"results":[
            {"id":54,"title":"Catalog film","release_date":"2025-02-01"},
            movie.clone(),
        ]}),
    );
    fixture.source("/tmdb/movie/55", movie);
    fixture.source(
        "/bangumi/search/subjects",
        json!({"data":[bangumi.clone()]}),
    );
    fixture.source("/bangumi/subjects/77", bangumi);
    fixture.episode_catalog(77, &[]);
    fixture.source("/bangumi/subjects/77/characters", json!([]));
    fixture.source("/bangumi/subjects/77/persons", json!([]));
    let mut item = json!({"Id":"22","Type":"Movie","Name":"手工中文名","OriginalTitle":"Catalog film",
        "ProductionYear":2026,"Path":"/media/catalog.mkv","ProviderIds":{}});
    fixture.item(item.clone());
    fs::write(fixture.directory.path().join("catalog.mkv"), b"media").unwrap();
    let event = fixture.event("Movie-22.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let nfo = fixture.directory.path().join("catalog.nfo");
    let document = fs::read_to_string(&nfo).unwrap();
    for evidence in [
        "手工中文名",
        "Catalog synopsis",
        "type=\"tmdb\"",
        ">55<",
        "type=\"bangumi\"",
        ">77<",
    ] {
        assert!(document.contains(evidence), "{evidence}: {document}");
    }
    item["ProviderIds"]["Tmdb"] = json!("55");
    fixture.item(item);
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(0)
    ));
    assert_eq!(fs::read_to_string(nfo).unwrap(), document);
}

#[test]
fn automatic_configuration_rejects_missing_roots_bad_dates_and_invalid_mapping_files() {
    let directory = tempfile::tempdir().unwrap();
    let mapping = directory.path().join("season.json");
    let mut settings = HashMap::<&str, OsString>::new();
    let load =
        |settings: &HashMap<_, _>| AutoScrape::from_settings(|key| settings.get(key).cloned());
    let error = |settings: &HashMap<_, _>| load(settings).err().unwrap();
    assert!(load(&settings).unwrap().is_none());
    settings.insert("EMBY_AUTO_SCRAPE_SPOOL", "relative".into());
    assert!(error(&settings).contains("绝对目录"));
    settings.insert("EMBY_AUTO_SCRAPE_SPOOL", directory.path().into());
    assert!(error(&settings).contains("指定 Emby 媒体库路径"));
    settings.insert("EMBY_AUTO_SCRAPE_ROOTS", "/".into());
    assert!(error(&settings).contains("明确的绝对媒体库路径"));
    settings.insert(
        "EMBY_AUTO_SCRAPE_ROOTS",
        " /media/Anime ; /media/Movies ".into(),
    );
    settings.insert("EMBY_AUTO_SCRAPE_PRIVATE_ROOTS", "/private".into());
    settings.insert("EMBY_AUTO_SCRAPE_TIMEZONE", "wrong timezone".into());
    assert!(error(&settings).contains("IANA 时区"));
    settings.insert("EMBY_AUTO_SCRAPE_TIMEZONE", "Asia/Shanghai".into());
    settings.insert("EMBY_AUTO_SCRAPE_SEASON_MAP", mapping.as_os_str().into());
    assert!(error(&settings).contains("无法读取季集映射"));
    fs::write(&mapping, r#"{"10":{"1":[]}}"#).unwrap();
    assert!(error(&settings).contains("Bangumi 条目 ID"));
    fs::write(&mapping, r#"{"10":{"1":[12]}}"#).unwrap();
    settings.insert("EMBY_AUTO_SCRAPE_SKIP_SPECIALS", "TRUE".into());
    settings.insert("EMBY_AUTO_SCRAPE_DB_SEASON_MAP", "true".into());
    settings.insert("EMBY_AUTO_SCRAPE_CENTRAL_METADATA", "True".into());
    settings.insert("EMBY_AUTO_SCRAPE_URL", "  ".into());
    let service = load(&settings).unwrap().unwrap();
    assert_eq!(
        service.roots,
        [
            PathBuf::from("/media/Anime"),
            PathBuf::from("/media/Movies")
        ]
    );
    assert_eq!(service.private_roots, [PathBuf::from("/private")]);
    assert!(service.url.is_none());
    assert!(service.skip_specials && service.database_season_map && service.central_metadata);
    assert_eq!(
        service
            .season_map
            .subjects(&episode("21", "first.mkv", 1, 1, "2026-01-01")),
        [12]
    );
}

#[tokio::test]
async fn alternate_order_intake_reads_the_original_tmdb_episode_and_rejects_conflicts() {
    let fixture = Fixture::new().await;
    let group = "1234567890abcdef12345678";
    let subgroup = "abcdef1234567890abcdef12";
    let mapping = json!({"1000":{"3":{"group":group,"subgroup":subgroup}}}).to_string();
    let make_collector = || {
        fixture
            .collector(None)
            .with_tmdb_settings(Arc::new(TmdbSettings::group_fixture(
                fixture.base.join("tmdb/").unwrap(),
                &mapping,
            )))
    };
    let collector = make_collector();
    let service = fixture.service(false, "{}");
    let group_path = format!("/tmdb/tv/episode_group/{group}");
    fixture.source(&group_path, json!({"id":group,"groups":[{"id":subgroup,"order":3,"episodes":[
        {"order":0,"show_id":1000,"season_number":1,"episode_number":40,"id":2040,"air_date":"2026-01-02"},
    ]}]}));
    fixture.source(
        "/tmdb/tv/1000",
        json!({"id":1000,"name":"Fixture Show","original_language":"ja"}),
    );
    fixture.source("/tmdb/tv/1000/season/1/episode/40", json!({
        "id":2040,"season_number":1,"episode_number":40,"air_date":"2026-01-02","name":"原序列第四十集",
        "overview":"匹配已核对的分组。",
    }));
    fixture.item(
        json!({"Id":"10","Name":"Fixture Show","Type":"Series","Path":"/media/show",
        "ProviderIds":{"Tmdb":"1000"}}),
    );
    let mut item = episode("30", "S03E01.mkv", 3, 1, "2026-01-01T16:00:00Z");
    fixture.item(item.clone());
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    let media = show.join("S03E01.mkv");
    fs::write(&media, b"media").unwrap();
    let event = fixture.event("Episode-30.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let nfo = media.with_extension("nfo");
    let saved = fs::read_to_string(&nfo).unwrap();
    assert!(saved.contains("<title>原序列第四十集</title>"));
    assert!(saved.contains("<season>3</season>"));
    assert!(saved.contains("<episode>1</episode>"));
    fixture.source(&group_path, json!({"id":"unrelated-group"}));
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(0)
    ));
    let failure = service
        .process(&make_collector(), &event)
        .await
        .err()
        .unwrap();
    assert!(failure.contains("分集分组 ID 不符"), "{failure}");
    item["ProviderIds"]["Tmdb"] = json!("4050");
    fixture.item(item);
    let failure = service.process(&collector, &event).await.err().unwrap();
    assert!(
        failure.contains("来源尚未提供可核对的分集标题"),
        "{failure}"
    );
    assert_eq!(fs::read_to_string(nfo).unwrap(), saved);
}

#[tokio::test]
async fn first_season_intake_uses_bangumi_when_tmdb_cannot_supply_the_episode_title() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    fixture.item(
        json!({"Id":"10","Name":"Fixture Show","Type":"Series","Path":"/media/show",
        "ProviderIds":{"Tmdb":"1000","Bangumi":"11"}}),
    );
    fixture.episode_catalog(11, &[bangumi_episode(81, 11, 1, "2026-01-01", "目录标题")]);
    let mut item = episode("22", "first.mkv", 1, 1, "2026-01-01");
    fixture.item(item.clone());
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    let media = show.join("first.mkv");
    fs::write(&media, b"media").unwrap();
    let event = fixture.event("Episode-22.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let nfo = media.with_extension("nfo");
    assert!(
        fs::read_to_string(&nfo)
            .unwrap()
            .contains("<title>目录标题</title>")
    );
    item["Name"] = json!("手工标题");
    item["PremiereDate"] = json!("2026-01-20");
    fixture.item(item);
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(error.contains("分集来源暂无可用资料"), "{error}");
    assert!(
        fs::read_to_string(&nfo)
            .unwrap()
            .contains("<title>目录标题</title>")
    );
}

#[tokio::test]
async fn mapped_season_repairs_only_the_empty_plot_in_the_existing_nfo() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, r#"{"10":{"2":[12]}}"#);
    fixture.source(
        "/bangumi/subjects/12",
        json!({
            "id":12, "type":2, "platform":"TV", "name":"Season", "date":"2026-01-03",
            "summary":"只补空白简介"
        }),
    );
    fixture.item(
        json!({"Id":"21","Type":"Season","Path":"/media/show/Season 02","Name":"手工季标题",
        "IndexNumber":2,"SeriesId":"10","ProviderIds":{}}),
    );
    let directory = fixture.directory.path().join("show/Season 02");
    fs::create_dir_all(&directory).unwrap();
    let nfo = directory.join("season.nfo");
    fs::write(
        &nfo,
        "<season><seasonnumber>2</seasonnumber><title>手工季标题</title><plot></plot></season>",
    )
    .unwrap();
    let event = fixture.event("Season-21.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let saved = fs::read_to_string(&nfo).unwrap();
    assert!(saved.contains("<plot>只补空白简介</plot>"));
    assert!(saved.contains("<title>手工季标题</title>"));
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(0)
    ));
    assert_eq!(fs::read_to_string(nfo).unwrap(), saved);
}

#[tokio::test]
async fn backup_catalog_requires_one_exact_title_and_year_across_aliases() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let mut home =
        json!({"OriginalTitle":"Original","Name":"Localized","PremiereDate":"2026-01-01"});
    let matches =
        |id, name| json!({"id":id,"type":2,"platform":"TV","name":name,"date":"2026-01-01"});
    fixture.source(
        "/bangumi/search/subjects",
        json!({"data":[matches(11, "Original")]}),
    );
    let choice = collector
        .automatic_backup(MediaKind::Series, &home, MediaProvider::Tmdb)
        .await
        .unwrap();
    assert!(choice.provider == MediaProvider::Bangumi);
    assert_eq!(choice.id, "11");
    fixture.source(
        "/bangumi/search/subjects",
        json!({"data":[matches(11, "Original"), matches(12, "Localized")]}),
    );
    let error = collector
        .automatic_backup(MediaKind::Series, &home, MediaProvider::Tmdb)
        .await
        .err()
        .unwrap();
    assert!(error.contains("没有唯一的同名同年份"), "{error}");
    fixture.source(
        "/tmdb/search/tv?query=Original",
        json!({"results":[
            {"id":99,"name":"Original","first_air_date":"2025-01-01"},
            {"id":100,"name":"Original","first_air_date":"2026-01-01"}
        ]}),
    );
    fixture.source(
        "/tmdb/search/tv?query=Localized",
        json!({"results":[
            {"id":100,"name":"Localized","first_air_date":"2026-01-01"}
        ]}),
    );
    let choice = collector
        .automatic_backup(MediaKind::Series, &home, MediaProvider::Bangumi)
        .await
        .unwrap();
    assert!(choice.provider == MediaProvider::Tmdb);
    assert_eq!(choice.id, "100");
    home["PremiereDate"] = Value::Null;
    assert!(
        collector
            .automatic_backup(MediaKind::Series, &home, MediaProvider::Bangumi)
            .await
            .err()
            .unwrap()
            .contains("无年份")
    );
}

#[tokio::test]
async fn auto_backfill_links_first_season_using_native_title_and_catalog_airdate() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let persistence = MemoryPersistence::default();
    let service = fixture.service(true, "{}");
    fixture.item(json!({
        "Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Tmdb":"1000","Bangumi":"11"}
    }));
    fixture.item(episode("21", "first.mkv", 1, 1, "2026-01-01"));
    fixture.item(episode("22", "second.mkv", 1, 2, "2026-01-08"));
    fixture.source(
        "/tmdb/tv/1000",
        json!({"id":1000,"name":"Fixture Show","original_language":"ja"}),
    );
    fixture.source("/tmdb/tv/1000/season/1/episode/1", json!({
        "id":101,"season_number":1,"episode_number":1,"air_date":"2026-01-01","name":"局部译名",
        "overview":"来源简介","translations":{"translations":[{"iso_639_1":"ja","data":{"name":"同一原文标题"}}]},
    }));
    fixture.source("/tmdb/tv/1000/season/1/episode/2", json!({
        "id":102,"season_number":1,"episode_number":2,"air_date":"2026-01-08","name":"第二话",
        "overview":"来源简介","translations":{"translations":[{"iso_639_1":"ja","data":{"name":"不一致的标题"}}]},
    }));
    fixture.episode_catalog(
        11,
        &[
            bangumi_episode(81, 11, 1, "2026-01-02", "同一原文标题"),
            bangumi_episode(82, 11, 2, "2026-01-08", "按日期与集号匹配"),
        ],
    );
    for (id, bangumi, tmdb) in [("21", "81", "101"), ("22", "82", "102")] {
        let event = fixture.event(&format!("Episode-{id}.backfill"));
        assert!(matches!(
            service
                .process_with(&collector, &event, &persistence)
                .await
                .unwrap(),
            Outcome::Enriched
        ));
        let saved = fixture.saved(id);
        assert_eq!(saved["ProviderIds"]["Bangumi"], bangumi);
        assert_eq!(saved["ProviderIds"]["Tmdb"], tmdb);
        assert_eq!(saved["Overview"], "来源简介");
    }
}

#[tokio::test]
async fn auto_season_intake_uses_three_episode_ids_when_season_dates_disagree() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let persistence = MemoryPersistence::default();
    let service = fixture.service(true, "{}");
    fixture.item(
        json!({"Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Tmdb":"500"}}),
    );
    fixture.item(
        json!({"Id":"20","Type":"Season","Path":"/media/show/Season 02","Name":"保持原季标题",
        "IndexNumber":2,"SeriesId":"10","PremiereDate":"2026-01-01","ProviderIds":{}}),
    );
    let mut catalog_episodes = Vec::new();
    for (index, date) in [(1, "2026-02-01"), (2, "2026-02-08"), (3, "2026-02-15")] {
        let mut item = episode(&(20 + index).to_string(), "episode.mkv", 2, index, date);
        item["ParentId"] = json!("20");
        item["ProviderIds"]["Tmdb"] = json!((200 + index).to_string());
        fixture.item(item);
        catalog_episodes.push(
            json!({"season_number":2,"episode_number":index,"id":200 + index,"air_date":date}),
        );
    }
    fixture.source(
        "/tmdb/tv/500/season/2",
        json!({
            "id":101,"name":"Verified season","season_number":2,"air_date":"2026-02-01",
            "overview":"可核对季简介","external_ids":{"tvdb_id":250},"vote_average":8,
            "episodes":catalog_episodes,
        }),
    );
    let event = fixture.event("Season-20.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    let saved = fixture.saved("20");
    assert_eq!(saved["Name"], "保持原季标题");
    assert_eq!(saved["Overview"], "可核对季简介");
    assert_eq!(saved["PremiereDate"], "2026-01-01");
    assert_eq!(saved["ProviderIds"], json!({"Tmdb":"101","Tvdb":"250"}));

    let mut mismatch = saved.clone();
    mismatch["Id"] = json!("30");
    mismatch["ProviderIds"]["Tmdb"] = json!("999");
    fixture.item(mismatch.clone());
    let event = fixture.event("Season-30.backfill");
    let error = service
        .process_with(&collector, &event, &persistence)
        .await
        .err()
        .unwrap();
    assert!(error.contains("需要核对分季映射"), "{error}");
    assert_eq!(fixture.saved("30"), mismatch);
}

#[tokio::test]
async fn cast_portraits_only_fetch_unambiguous_unfilled_people() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let emby = collector
        .emby_settings()
        .unwrap()
        .automatic_connection(None)
        .unwrap();
    for (id, name) in [
        ("51", "Only"),
        ("52", "Ambiguous"),
        ("53", "Has image"),
        ("54", "Wrong role"),
        ("55", "No match"),
    ] {
        fixture.item(json!({"Id":id,"Type":"Person","Name":name}));
    }
    let item = json!({"Type":"Series","People":[
        {"Id":"not/an/id","Name":"Only","Type":"Actor"},
        {"Id":"51","Name":"Only","Type":"Actor"},
        {"Id":"52","Name":"Ambiguous","Type":"Actor"},
        {"Id":"53","Name":"Has image","Type":"Actor","PrimaryImageTag":"active"},
        {"Id":"54","Name":"Wrong role","Type":"Director"},
        {"Id":"55","Name":"No match","Type":"Actor"},
        {"Id":"56","Type":"Actor"}
    ]});
    let actor = |name, path| json!({"name":name,"images":{"large":format!("https://lain.bgm.tv/pic/{path}")}});
    let record = json!({"sources":[{"provider":"Bangumi","data":{
        "subject":{"type":2},"characters":[{"actors":[
            actor("Only","only.jpg"), actor("Ambiguous","first.jpg"), actor("Ambiguous","second.jpg"),
            actor("Has image","present.jpg"), actor("Wrong role","actor.jpg")
        ]}]
    }}]});
    fill_cast_portraits(&collector, &emby, &item, &record).await;
    let library = fixture.library.lock().unwrap();
    assert_eq!(library.edit_reads, ["51"]);
    assert!(library.writes.is_empty());
}

#[tokio::test]
#[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
async fn postgres_auto_worker_defers_events_during_an_emby_outage_and_recovers() {
    let fixture = Fixture::new().await;
    let database = Database::new().await;
    let collector = Arc::new(fixture.collector(Some(database.url.as_str())));
    let service = fixture.service(false, "{}");
    let event = fixture.event("Movie-30.event");
    fixture
        .library
        .lock()
        .unwrap()
        .item_errors
        .insert("30".into(), StatusCode::SERVICE_UNAVAILABLE);
    let worker = tokio::spawn(service.run(Arc::clone(&collector)));
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = collector.auto_scrape_state().await.unwrap();
            if state["counts"]["retry"] == 1 {
                assert_eq!(state["jobs"][0]["error_kind"], "emby_unavailable");
                assert_eq!(state["dependencies"][0]["ready"], false);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(event.path.exists());

    let service = fixture.service(false, "{}");
    let later = fixture.event("Movie-31.event");
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    assert_eq!(state["counts"], json!({"queued":1,"retry":1}));
    assert!(later.path.exists());
    {
        let mut library = fixture.library.lock().unwrap();
        library.item_errors.clear();
    }
    for id in ["30", "31"] {
        fixture.item(json!({"Id":id,"Type":"Movie","Path":"/media/m.mkv","IsLocked":true}));
    }
    sqlx::query(
        "UPDATE collect_metadata_auto_dependencies SET next_check_at=now()-interval '1 second'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE collect_metadata_auto_jobs SET next_retry_at=now()-interval '1 second'")
        .execute(&database.pool)
        .await
        .unwrap();
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    assert_eq!(state["counts"], json!({"skipped":2}));
    assert_eq!(state["dependencies"][0]["ready"], true);
    assert_eq!(state["dependencies"][0]["failures"], 0);
    assert!(!event.path.exists());
    assert!(!later.path.exists());
    drop(collector);
    database.close().await;
}

#[tokio::test]
async fn bangumi_catalog_keeps_canonical_work_fields_and_partial_source_status() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    fixture.source("/bangumi/subjects/61", json!({
        "id":61,"type":6,"platform":"电影","name":"Original Film","name_cn":"译名",
        "date":"1982-01-02","summary":"原始简介","rating":{"score":8.3,"total":99},
        "images":{"large":"https://lain.bgm.tv/pic/cover/l/61.jpg"},
        "tags":[{"name":"剧情"}],
        "infobox":[
            {"key":"别名","value":[{"v":"Alt Film"}]},
            {"key":"语言","value":[{"v":"汉语、普通话、粤语/英語、法语、德语、俄语、西班牙语、意大利语、泰语、韩语、日语／日语、未知"}]},
            {"key":"播放电视台","value":"Network"},
            {"key":"官方网站","value":[{"v":"file:///unsafe"},{"v":"https://example.org/official"}]}
        ]
    }));
    fixture.source(
        "/bangumi/subjects/61/persons",
        json!([
            {"name":"演出者","relation":"主演"}, {"name":"演出者","relation":"配角"},
            {"name":"导演","relation":"导演"}, {"name":"编剧","relation":"编剧"},
            {"name":"制片","relation":"制片人"}, {"name":"工作室","relation":"制作"}
        ]),
    );
    fixture.episode_catalog(
        61,
        &[json!({"id":609,"type":0,"ep":1,"duration_seconds":6336})],
    );
    let candidates = collector
        .search_media(MediaKind::Movie, "bangumi:61")
        .await
        .unwrap();
    let hit = &candidates.candidates[0];
    assert_eq!(hit.title, "译名");
    assert_eq!(hit.year, Some(1982));
    assert!(hit.match_titles.iter().any(|title| title == "Alt Film"));
    let wrong = collector
        .search_media(MediaKind::Series, "bangumi:61")
        .await
        .unwrap();
    assert!(wrong.candidates.is_empty());

    let found = collector
        .collect_media(MediaKind::Movie, MediaProvider::Bangumi, "61")
        .await
        .unwrap();
    assert_eq!(found.record["sources"][0]["status"], "partial");
    assert!(
        found.record["sources"][0]["note"]
            .as_str()
            .unwrap()
            .contains("角色与配音")
    );
    let fields = &found.record["fields"];
    assert_eq!(fields["runtime_minutes"], 106);
    assert_eq!(fields["rating"]["count"], 99);
    assert_eq!(fields["cast"], json!(["演出者"]));
    assert_eq!(fields["directors"], json!(["导演"]));
    assert_eq!(fields["writers"], json!(["编剧"]));
    assert_eq!(fields["producers"], json!(["制片"]));
    assert_eq!(fields["studio"], json!(["工作室"]));
    assert_eq!(fields["network"], "Network");
    assert_eq!(fields["official_url"], "https://example.org/official");
    assert_eq!(
        fields["cover_url"],
        "https://lain.bgm.tv/pic/cover/l/61.jpg"
    );
    assert_eq!(
        fields["original_languages"],
        json!([
            "zh", "cmn", "yue", "en", "fr", "de", "ru", "es", "it", "th", "ko", "ja"
        ])
    );
}

#[tokio::test]
async fn emby_episode_overview_uses_linked_bangumi_only_when_local_text_needs_it() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    fixture.item(
        json!({"Id":"10","Type":"Series","Name":"Fixture Show","Path":"/media/show",
        "ProviderIds":{"Bangumi":"11"}}),
    );
    let mut item = episode("21", "first.mkv", 1, 1, "2026-01-01");
    item["Name"] = json!("首集标题");
    fixture.item(item.clone());
    fixture.episode_catalog(11, &[bangumi_episode(81, 11, 1, "2026-01-01", "首集标题")]);
    let found = collector.emby_episode_overview("21").await.unwrap();
    assert_eq!(found.source.as_deref(), Some("Bangumi"));
    assert_eq!(found.overview.as_deref(), Some("来自分集目录的简介"));
    item["Overview"] = json!("本地编辑写入的简介");
    fixture.item(item);
    let kept = collector.emby_episode_overview("21").await.unwrap();
    assert_eq!(kept.source.as_deref(), Some("Emby"));
    assert_eq!(kept.overview.as_deref(), Some("本地编辑写入的简介"));
}

#[tokio::test]
async fn season_file_hint_and_catalog_supplement_do_not_override_known_fields() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    fixture.source(
        "/bangumi/subjects/71",
        json!({
            "id":71,"type":2,"platform":"剧场版","name":"Fixture Film",
            "date":"2026-03-01","summary":"Bangumi synopsis"
        }),
    );
    fixture.source("/bangumi/subjects/71/persons", json!([]));
    fixture.source("/bangumi/subjects/71/characters", json!([]));
    fixture.episode_catalog(71, &[]);
    let tmdb = json!({"id":31,"title":"Fixture Film","original_title":"Fixture Film",
        "release_date":"2026-03-01","overview":"TMDB synopsis","runtime":97});
    fixture.source("/tmdb/search/movie", json!({"results":[tmdb.clone()]}));
    fixture.source("/tmdb/movie/31", tmdb);
    let item = json!({"Id":"72","Type":"Movie","Path":"/media/Fixture Film [bangumi-71].mkv",
        "Name":"已有电影名称","ProviderIds":{}});
    fixture.item(item);
    let media = fixture
        .directory
        .path()
        .join("Fixture Film [bangumi-71].mkv");
    fs::write(&media, b"media").unwrap();
    let event = fixture.event("Movie-72.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let xml = fs::read_to_string(media.with_extension("nfo")).unwrap();
    for text in [
        "已有电影名称",
        "Bangumi synopsis",
        "type=\"bangumi\"",
        ">71<",
        "type=\"tmdb\"",
        ">31<",
    ] {
        assert!(xml.contains(text), "{text}: {xml}");
    }
}

#[tokio::test]
async fn writeback_refuses_moved_locked_or_unconfirmed_episode_updates() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let emby = collector
        .emby_settings()
        .unwrap()
        .automatic_connection(None)
        .unwrap();
    let snapshot = episode("21", "first.mkv", 1, 1, "2026-01-01");
    fixture.item(snapshot.clone());
    let record = json!({"fields":{"description":"new plot","title":"new title"}});
    let write = |original: Value| {
        let emby = &emby;
        let client = &collector.client;
        let record = &record;
        async move {
            emby.enrich_automatic_item(client, &original, record, &[], &TimeZone::UTC, false)
                .await
        }
    };
    fixture.library.lock().unwrap().native_sidecars = true;
    assert!(
        write(snapshot.clone())
            .await
            .unwrap_err()
            .contains("关闭原生旁挂保存")
    );
    fixture.library.lock().unwrap().native_sidecars = false;

    let mut moved = snapshot.clone();
    moved["Path"] = json!("/media/show/renamed.mkv");
    fixture.item(moved);
    assert!(
        write(snapshot.clone())
            .await
            .unwrap_err()
            .contains("条目在采集期间发生变化")
    );
    assert!(
        emby.fill_episode_bangumi_id(&collector.client, &snapshot, 81)
            .await
            .unwrap_err()
            .contains("核对映射期间发生变化")
    );
    let mut locked = snapshot.clone();
    locked["LockData"] = json!({"Enabled":true});
    fixture.item(locked);
    assert!(
        write(snapshot.clone())
            .await
            .unwrap_err()
            .contains("条目已锁定")
    );
    assert!(
        !emby
            .fill_episode_bangumi_id(&collector.client, &snapshot, 81)
            .await
            .unwrap()
    );
    assert!(fixture.library.lock().unwrap().writes.is_empty());

    fixture.item(snapshot.clone());
    fixture
        .library
        .lock()
        .unwrap()
        .write_errors
        .insert("21".into(), StatusCode::SERVICE_UNAVAILABLE);
    let error = write(snapshot.clone()).await.unwrap_err();
    assert!(error.contains("写入资料失败（HTTP 503）"), "{error}");
    assert_eq!(fixture.saved("21"), snapshot);
    fixture.library.lock().unwrap().write_errors.clear();
    fixture
        .library
        .lock()
        .unwrap()
        .rejected_fields
        .insert("21".into(), "People");
    let people = [json!({"Name":"确认过的演员","Type":"Actor"})];
    let error = emby
        .enrich_automatic_item(
            &collector.client,
            &snapshot,
            &record,
            &people,
            &TimeZone::UTC,
            false,
        )
        .await
        .unwrap_err();
    assert!(error.contains("People"), "{error}");
    assert_eq!(fixture.saved("21")["People"], snapshot["People"]);

    fixture.item(snapshot.clone());
    fixture
        .library
        .lock()
        .unwrap()
        .rejected_fields
        .insert("21".into(), "Name");
    let error = write(snapshot.clone()).await.unwrap_err();
    assert!(error.contains("Name"), "{error}");
    assert_eq!(fixture.saved("21")["Name"], snapshot["Name"]);
    fixture.item(snapshot.clone());
    fixture
        .library
        .lock()
        .unwrap()
        .rejected_fields
        .insert("21".into(), "ProviderIds");
    let error = emby
        .fill_episode_bangumi_id(&collector.client, &snapshot, 81)
        .await
        .unwrap_err();
    assert!(error.contains("没有保留全部分集来源 ID"));
    assert_eq!(fixture.saved("21")["ProviderIds"], snapshot["ProviderIds"]);
}

#[tokio::test]
async fn conflicting_mapped_sources_only_fall_back_to_independently_verified_episodes() {
    let fixture = Fixture::new().await;
    let service = fixture.service(false, r#"{"10":{"2":[11]}}"#);
    let group = "1234567890abcdef12345678";
    let subgroup = "abcdef1234567890abcdef12";
    let mapping = json!({"1000":{"2":{"group":group,"subgroup":subgroup}}}).to_string();
    let make_collector = || {
        fixture
            .collector(None)
            .with_tmdb_settings(Arc::new(TmdbSettings::group_fixture(
                fixture.base.join("tmdb/").unwrap(),
                &mapping,
            )))
    };
    fixture.item(
        json!({"Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Tmdb":"1000"}}),
    );
    fixture.item(episode("21", "E01.mkv", 2, 1, "2026-01-02"));
    let group_path = format!("/tmdb/tv/episode_group/{group}");
    fixture.source(&group_path, json!({"id":group,"groups":[{"id":subgroup,"order":2,"episodes":[
        {"order":0,"show_id":1000,"season_number":1,"episode_number":3,"id":103,"air_date":"2026-01-02"}
    ]}]}));
    fixture.source(
        "/tmdb/tv/1000",
        json!({"id":1000,"name":"Fixture Show","original_language":"ja"}),
    );
    let tmdb_episode = "/tmdb/tv/1000/season/1/episode/3";
    fixture.source(tmdb_episode, json!({
        "id":103,"season_number":1,"episode_number":3,"air_date":"2026-01-02","name":"已核对的 TMDB 分集"
    }));
    fixture.episode_catalog(11, &[]);
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    let media = show.join("E01.mkv");
    fs::write(&media, b"media").unwrap();
    let nfo = media.with_extension("nfo");
    let event = fixture.event("Episode-21.event");
    let collector = make_collector();
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    assert!(
        fs::read_to_string(&nfo)
            .unwrap()
            .contains("已核对的 TMDB 分集")
    );

    fs::remove_file(&nfo).unwrap();
    fixture.source(tmdb_episode, json!({
        "id":103,"season_number":1,"episode_number":3,"air_date":"2026-01-10","name":"日期不符的分集"
    }));
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(error.contains("无法用已核对的 Bangumi 分季映射"), "{error}");
    assert!(!nfo.exists());

    fixture.source(&group_path, json!({"id":"wrong"}));
    let error = service
        .process(&make_collector(), &event)
        .await
        .err()
        .unwrap();
    assert!(error.contains("分集分组 ID 不符"), "{error}");
    fixture
        .library
        .lock()
        .unwrap()
        .source_errors
        .insert(group_path.clone(), StatusCode::SERVICE_UNAVAILABLE);
    let error = service
        .process(&make_collector(), &event)
        .await
        .err()
        .unwrap();
    assert!(
        error.contains("TMDB 分集分组") && error.contains("503"),
        "{error}"
    );
    fixture.library.lock().unwrap().source_errors.clear();
    fixture.episode_catalog(
        11,
        &[bangumi_episode(
            81,
            11,
            1,
            "2026-01-02",
            "已核对的 Bangumi 分集",
        )],
    );
    let collector = make_collector();
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    assert!(
        fs::read_to_string(&nfo)
            .unwrap()
            .contains("已核对的 Bangumi 分集")
    );
}

#[tokio::test]
async fn auto_episode_identity_checks_tmdb_lookup_and_only_retries_source_failures() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let persistence = MemoryPersistence::default();
    let service = fixture.service(true, "{}");
    let mut item = episode("21", "E01.mkv", 1, 1, "2026-01-02");
    item["ProviderIds"]["Imdb"] = json!("tt2001");
    fixture.item(item.clone());
    fixture.item(
        json!({"Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Tmdb":"1000"}}),
    );
    let found = |series| json!({"tv_episode_results":[{"show_id":series,"season_number":1,"episode_number":1,"id":201}]});
    fixture.source("/tmdb/find/tt2001", found(999));
    let event = fixture.event("Episode-21.backfill");
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::NoVerifiedSource
    ));
    assert_eq!(fixture.saved("21"), item);
    fixture.source("/tmdb/find/tt2001", found(1000));
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::NoVerifiedSource
    ));

    let endpoint = "/tmdb/tv/1000/season/1/episode/1";
    fixture
        .library
        .lock()
        .unwrap()
        .source_errors
        .insert(endpoint.into(), StatusCode::SERVICE_UNAVAILABLE);
    let error = service
        .process_with(&collector, &event, &persistence)
        .await
        .err()
        .unwrap();
    assert!(error.contains("503"), "{error}");
    assert_eq!(fixture.saved("21"), item);

    fixture.library.lock().unwrap().source_errors.clear();
    fixture.source(endpoint, json!({
        "id":201,"season_number":1,"episode_number":1,"air_date":"2026-01-02","name":"确认身份的首集",
        "overview":"所属剧集暂不可用时仍保留已验证的单集资料。",
        "external_ids":{"imdb_id":"tt2001"}
    }));
    assert!(matches!(
        service
            .process_with(&collector, &event, &persistence)
            .await
            .unwrap(),
        Outcome::Enriched
    ));
    let saved = fixture.saved("21");
    assert_eq!(saved["Name"], item["Name"]);
    assert_eq!(saved["ProviderIds"], json!({"Imdb":"tt2001","Tmdb":"201"}));
    assert_eq!(
        saved["Overview"],
        "所属剧集暂不可用时仍保留已验证的单集资料。"
    );
}

#[tokio::test]
async fn placeholder_intake_respects_the_title_retry_window_and_never_touches_existing_nfo() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, r#"{"10":{"2":[11]}}"#);
    fixture.item(episode("21", "E02.mkv", 2, 2, "2026-01-08"));
    fixture.episode_catalog(
        11,
        &[
            bangumi_episode(81, 11, 1, "2026-01-01", "第一集正式标题"),
            bangumi_episode(82, 11, 2, "2026-01-08", "第 2 集"),
        ],
    );
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("E02.mkv"), b"media").unwrap();
    let nfo = show.join("E02.nfo");
    let original = b"<episodedetails><title>existing placeholder</title></episodedetails>";
    fs::write(&nfo, original).unwrap();
    let event = fixture.event("Episode-21.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::AwaitingTitle
    ));
    assert!(fixture.library.lock().unwrap().refreshes.is_empty());
    fs::remove_file(&event.path).unwrap();
    assert!(
        service
            .process(&collector, &event)
            .await
            .err()
            .unwrap()
            .contains("无法读取入库事件时间")
    );

    let event = fixture.event("Episode-21.event");
    fs::File::options()
        .write(true)
        .open(&event.path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - RETRY_WINDOW - Duration::from_secs(60))
        .unwrap();
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(0)
    ));
    assert_eq!(fs::read(nfo).unwrap(), original);
    assert_eq!(fixture.library.lock().unwrap().refreshes, ["21"]);
}

#[tokio::test]
async fn bangumi_sequel_requires_distinct_dates_and_one_matching_follow_up() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    fixture.item(
        json!({"Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Bangumi":"11"}}),
    );
    fixture.item(
        json!({"Id":"20","Type":"Season","Path":"/media/show/Season 02","Name":"第二季",
        "IndexNumber":2,"SeriesId":"10","ProviderIds":{}}),
    );
    let event = fixture.event("Season-20.event");
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(error.contains("至少三个不同放送日"), "{error}");
    let dates = ["2026-02-01", "2026-02-08", "2026-02-15"];
    for (index, date) in (1..=3).zip(dates) {
        let mut item = episode(&(20 + index).to_string(), "fixture.mkv", 2, index, date);
        item["ParentId"] = json!("20");
        fixture.item(item);
    }
    for (subject, date) in [(11, "2025-01-01"), (12, dates[0]), (13, dates[0])] {
        fixture.source(&format!("/bangumi/subjects/{subject}"), json!({
            "id":subject,"type":2,"platform":"TV","name":format!("Season {subject}"),"date":date,
        }));
        let entries: Vec<_> = (1..=3)
            .zip(dates)
            .map(|(index, date)| {
                bangumi_episode(1000 + subject * 10 + index, subject, index, date, "Title")
            })
            .collect();
        fixture.episode_catalog(subject, &entries);
        fixture.source(&format!("/bangumi/subjects/{subject}/subjects"), json!([]));
    }
    fixture.source(
        "/bangumi/subjects/11/subjects",
        json!([
            {"id":12,"type":2,"relation":"续集"},
            {"id":13,"type":2,"relation":"续集"},
        ]),
    );
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(
        error.contains("未找到分集日期和集次一致的唯一续集"),
        "{error}"
    );
    assert!(fixture.library.lock().unwrap().refreshes.is_empty());
    assert!(fixture.library.lock().unwrap().writes.is_empty());
}

#[tokio::test]
async fn intake_does_not_publish_metadata_when_catalog_enrichment_cannot_be_verified() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    let service = fixture.service(false, "{}");
    fixture.item(json!({
        "Id":"21","Type":"Movie","Name":"本地电影名称","OriginalTitle":"Catalog film",
        "ProductionYear":2026,"Path":"/media/movie.mkv","ProviderIds":{"Tmdb":"41"}
    }));
    fixture.source(
        "/tmdb/movie/41",
        json!({"id":41,"title":"Catalog film","overview":"Catalog synopsis"}),
    );
    let path = fixture.directory.path().join("movie.mkv");
    fs::write(&path, b"media").unwrap();
    let event = fixture.event("Movie-21.event");
    fixture.library.lock().unwrap().source_errors.insert(
        "/bangumi/search/subjects".into(),
        StatusCode::SERVICE_UNAVAILABLE,
    );
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(
        error.contains("Bangumi") && error.contains("503"),
        "{error}"
    );
    assert!(!path.with_extension("nfo").exists());
    fixture.library.lock().unwrap().source_errors.clear();
    fixture.source(
        "/bangumi/search/subjects",
        json!({"data":[
            {"id":81,"type":2,"platform":"剧场版","name":"Catalog film","date":"2026-01-01"}
        ]}),
    );
    let error = service.process(&collector, &event).await.err().unwrap();
    assert!(
        error.contains("主来源") && error.contains("备用来源"),
        "{error}"
    );
    assert!(!path.with_extension("nfo").exists());
    assert!(fixture.library.lock().unwrap().refreshes.is_empty());
}

#[tokio::test]
#[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
async fn postgres_auto_queue_preserves_waiting_title_schedule_and_archives_configuration_errors() {
    let fixture = Fixture::new().await;
    let database = Database::new().await;
    let collector = fixture.collector(Some(database.url.as_str()));
    let service = fixture.service(false, "{}");
    fixture.item(
        json!({"Id":"10","Type":"Series","Path":"/media/show","Name":"Fixture Show",
        "ProviderIds":{"Tmdb":"1000"}}),
    );
    fixture.item(episode("21", "E01.mkv", 1, 1, "2026-01-01"));
    let episode = fixture.event("Episode-21.event");
    let unauthorized = fixture.event("Movie-31.event");
    fixture
        .library
        .lock()
        .unwrap()
        .item_errors
        .insert("31".into(), StatusCode::FORBIDDEN);
    service.durable_cycle(&collector).await.unwrap();
    let state = collector.auto_scrape_state().await.unwrap();
    let job = state["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["item_id"] == "31")
        .unwrap();
    assert_eq!(job["status"], "review");
    assert_eq!(job["error_kind"], "configuration_review");
    assert!(!unauthorized.path.exists());
    assert!(episode.path.exists());

    for (attempt, seconds) in [(1, 1800), (2, 21600)] {
        if attempt == 2 {
            sqlx::query("UPDATE collect_metadata_auto_jobs SET next_retry_at=now()-interval '1 second' WHERE item_id='21'")
                .execute(&database.pool).await.unwrap();
            service.durable_cycle(&collector).await.unwrap();
        }
        let state = collector.auto_scrape_state().await.unwrap();
        let job = state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["item_id"] == "21")
            .unwrap();
        assert_eq!(job["status"], "waiting_source");
        assert_eq!(job["error_kind"], "awaiting_title");
        assert_eq!(job["attempts"], attempt);
        assert_eq!(job["failures"], 0);
        let remaining: f64 = sqlx::query_scalar("SELECT extract(epoch FROM next_retry_at-now())::float8 FROM collect_metadata_auto_jobs WHERE item_id='21'")
            .fetch_one(&database.pool).await.unwrap();
        assert!((seconds as f64 - 30.0..=seconds as f64).contains(&remaining));
    }
    assert!(episode.path.exists());
    drop(collector);
    database.close().await;
}

#[tokio::test]
async fn bangumi_never_treats_malformed_pagination_as_a_complete_catalog() {
    let fixture = Fixture::new().await;
    let collector = fixture.collector(None);
    fixture.source("/bangumi/subjects/62", json!({
        "id":62,"type":2,"platform":"TV","name":"Fixture Show","summary":"来自目录的作品简介",
        "date":"2026-01-01","eps":2,"infobox":[{"key":"单集片长","value":[{"v":"未知"},{"v":"0分"},{"v":"24分钟"}]}],
    }));
    fixture.source("/bangumi/subjects/62/persons", json!([]));
    fixture.source("/bangumi/subjects/62/characters", json!([]));
    let episodes = [
        bangumi_episode(621, 62, 1, "2026-01-01", "首集"),
        bangumi_episode(622, 62, 2, "2026-01-08", "第二集"),
    ];
    let path = "/bangumi/episodes?subject_id=62";
    for response in [
        json!({"offset":0,"data":episodes}),
        json!({"offset":10,"total":2,"data":episodes}),
        json!({"offset":0,"total":2,"data":{}}),
        json!({"offset":0,"total":2,"data":[]}),
    ] {
        fixture.source(path, response);
        let result = collector
            .collect_media(MediaKind::Series, MediaProvider::Bangumi, "62")
            .await
            .unwrap();
        assert_eq!(result.record["sources"][0]["status"], "partial");
        assert!(
            result.record["sources"][0]["note"]
                .as_str()
                .unwrap()
                .contains("分集")
        );
        assert_eq!(result.record["fields"]["runtime_minutes"], 24);
        assert!(result.record["fields"]["episodes"].is_null());
    }
    fixture.episode_catalog(62, &episodes);
    let corrected = collector
        .collect_media(MediaKind::Series, MediaProvider::Bangumi, "62")
        .await
        .unwrap();
    assert_eq!(corrected.record["sources"][0]["status"], "ok");
    assert_eq!(
        corrected.record["fields"]["episodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn automatic_sidecar_names_the_episode_using_the_confirmed_emby_title() {
    let fixture = Fixture::new().await;
    let service = fixture.service(false, r#"{"10":{"2":[11]}}"#);
    let collector = fixture.collector(None);
    let mut item = episode("21", "E01.mkv", 2, 1, "2026-01-01");
    item["Name"] = json!("人工整理的正式标题");
    item["Overview"] = json!("正在使用的本地简介");
    fixture.item(item);
    fixture.episode_catalog(
        11,
        &[bangumi_episode(81, 11, 1, "2026-01-01", "外部来源标题")],
    );
    let show = fixture.directory.path().join("show");
    fs::create_dir(&show).unwrap();
    fs::write(show.join("E01.mkv"), b"media").unwrap();
    let event = fixture.event("Episode-21.event");
    assert!(matches!(
        service.process(&collector, &event).await.unwrap(),
        Outcome::Written(1)
    ));
    let xml = fs::read_to_string(show.join("E01.nfo")).unwrap();
    assert!(xml.contains("<title>人工整理的正式标题</title>"));
    assert!(xml.contains("<plot>正在使用的本地简介</plot>"));
    assert!(!xml.contains("<title>外部来源标题</title>"));
}
