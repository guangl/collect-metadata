use serde_json::json;

use super::*;

#[tokio::test]
async fn desktop_commands_work_without_a_database_and_keep_preferences() {
    let app = tauri::test::mock_app();
    let files = tempfile::tempdir().unwrap();
    let root = files.path();
    diagnostics::init_desktop(root.join("logs"));
    let tmdb = Arc::new(TmdbSettings::from_file(root.join("token")).unwrap());
    let emby = Arc::new(EmbySettings::load(root.join("emby.json")).unwrap());
    let output = Arc::new(OutputSettings::load(root.join("output.json"), None).unwrap());
    let collector = Collector::from_environment(None)
        .unwrap()
        .with_tmdb_settings(Arc::clone(&tmdb))
        .with_emby_settings(Arc::clone(&emby))
        .with_output_settings(Arc::clone(&output));
    app.manage(DesktopState {
        runtime: RwLock::new(Runtime::start(collector)),
        config_lock: tokio::sync::Mutex::new(()),
        database_file: root.join("database-url"),
        tmdb,
        output,
        emby,
    });
    let state = || app.state::<DesktopState>();

    assert!(!storage_configured(state()));
    let connection_error = check_storage(state()).await.unwrap_err();
    let payload = serde_json::to_value(connection_error).unwrap();
    let id = payload["diagnostic_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(id).is_ok());
    assert_eq!(payload["code"], "STORAGE_CHECK_FAILED");
    assert!(payload["error"].as_str().unwrap().contains("PostgreSQL"));
    let log_directory = desktop_log_directory().await.unwrap();
    let log =
        fs::read_to_string(PathBuf::from(log_directory).join("collect-metadata.log")).unwrap();
    let entry: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(entry["event"], "desktop_command_failed");
    assert_eq!(entry["request_id"], id);
    assert_eq!(entry["details"]["command"], "check_storage");
    assert_eq!(entry["details"]["code"], payload["code"]);
    assert!(configure_database(state(), "  ".into()).await.is_err());
    assert!(history(state(), None, None, None).await.unwrap().is_empty());
    assert!(history_languages(state()).await.unwrap().is_empty());
    assert!(load_snapshot(state(), 1).await.is_err());
    assert!(create_batch(state(), vec![]).await.is_err());
    assert!(list_batches(state(), None).await.is_err());
    assert!(batch_rows(state(), 1).await.is_err());
    assert!(batch_view(state(), 1).await.is_err());
    assert!(batch_files(state(), 1, 2).await.is_err());
    assert!(batch_result(state(), 1, 2).await.is_err());
    assert!(control_batch(state(), 1, BatchAction::Pause).await.is_err());
    assert!(retry_batch_row(state(), 1, 2).await.is_err());
    assert!(
        choose_batch_candidate(
            state(),
            1,
            2,
            serde_json::from_value(json!({
                "provider":"tmdb", "id":"10"
            }))
            .unwrap()
        )
        .await
        .is_err()
    );
    assert!(lookup(state(), "../".into()).await.is_err());
    let search_error = search_media(state(), MediaKind::Movie, " ".into())
        .await
        .err()
        .unwrap();
    assert_eq!(search_error.code, ErrorCode::MediaSearchFailed);
    assert_eq!(
        search_error.error,
        "请输入电影或剧集名称（最多 160 个字符）。"
    );
    assert!(
        search_media(state(), MediaKind::Av, "abc".into())
            .await
            .is_err()
    );
    assert!(
        collect_media(state(), MediaKind::Av, MediaProvider::Tmdb, "10".into())
            .await
            .is_err()
    );

    assert!(serde_json::to_value(tmdb_status(state())).is_ok());
    assert!(
        test_tmdb(state(), Some("invalid key".into()))
            .await
            .is_err()
    );
    assert!(save_tmdb(state(), "invalid key".into()).await.is_err());
    assert_eq!(
        serde_json::to_value(emby_settings(state())).unwrap()["configured"],
        false
    );
    let emby_input =
        || serde_json::from_value(json!({"enabled":true,"server":"file:///local"})).unwrap();
    assert!(test_emby(state(), emby_input()).await.is_err());
    assert!(save_emby(state(), emby_input()).await.is_err());
    assert!(emby_libraries(state()).await.is_err());
    assert!(import_emby(state(), "7".into()).await.is_err());

    let initial = serde_json::to_value(output_settings(state())).unwrap();
    assert_eq!(initial["options"]["enabled"], false);
    let preferences = serde_json::from_value(json!({
        "enabled":true,"directory":root,"directory_template":"{title}","filename_prefix":"{id}"
    }))
    .unwrap();
    save_output_settings(state(), preferences).await.unwrap();
    let stored = OutputSettings::load(root.join("output.json"), None).unwrap();
    let saved = serde_json::to_value(stored.view()).unwrap();
    assert_eq!(saved["options"]["directory"], root.to_str().unwrap());
    assert_eq!(saved["options"]["directory_template"], "{title}");
    assert_eq!(saved["options"]["filename_prefix"], "{id}");
    assert!(!root.join("database-url").exists());

    let media = root.join("Library");
    fs::create_dir_all(media.join("Series/Season 01")).unwrap();
    fs::create_dir_all(media.join("Series/extras")).unwrap();
    fs::write(media.join("Series/Season 01/S01E01.mkv"), []).unwrap();
    fs::write(media.join("Series/extras/trailer.mkv"), []).unwrap();
    assert_eq!(
        scan_folder_paths(&media).unwrap(),
        ["Library/Series/Season 01/S01E01.mkv"]
    );
    assert!(scan_folder_paths(&root.join("missing")).is_err());
    state().runtime.read().unwrap().worker.abort();
}
