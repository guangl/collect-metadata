use collect_metadata::{BatchAction, ErrorCode, MediaKind, MediaProvider, scan_folder_paths};
use serde_json::{Value, json};

use super::{
    *,
    batches::{batch_files, batch_result, batch_rows, batch_view, choose_batch_candidate, control_batch, create_batch, list_batches, retry_batch_row},
    emby::{emby_libraries, import_emby},
    logs::desktop_log_directory,
    metadata::{clear_history, collect_media, history, history_languages, load_snapshot, lookup, search_media},
    settings::{check_storage, configure_database, emby_settings, output_settings, save_emby, save_output_settings, save_tmdb, storage_configured, test_emby, test_tmdb, tmdb_status},
};

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
        .with_output_settings(Arc::clone(&output))
        .with_language_settings(Arc::new(LanguageSettings::load(Some(root.join("languages.json"))).unwrap()));
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
    assert_eq!(
        clear_history(state()).await.unwrap_err().code,
        ErrorCode::HistoryClearFailed
    );
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

    Box::pin(assert_desktop_commands_reject_unconfigured_requests(&state)).await;

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

async fn assert_desktop_commands_reject_unconfigured_requests<'a>(
    state: &impl Fn() -> tauri::State<'a, DesktopState>,
) {
    use batches::{
        correct_batch_match, create_ai_batch, create_folder_batch, preview_writeback,
        queue_writeback, scrape_folder_work,
    };
    use collect_metadata::{AppError, EmbyPreviewImage, EmbyScrapeScope, EmbyScrapeTarget};
    use emby::{
        emby_catalog, emby_credits, emby_episode_overview, emby_missing_episode_overviews,
        emby_preview_image, emby_series_preview, emby_work_sources, import_emby_catalog,
        import_emby_item, import_emby_selection, overwrite_emby, preview_emby,
        refresh_emby_metadata,
    };
    use settings::{ai_status, bangumi_status, language_settings, save_bangumi, save_languages, test_bangumi};

    fn code<T>(result: Result<T, AppError>, expected: ErrorCode) {
        assert_eq!(result.err().expect("request should be rejected").code, expected);
    }

    assert!(!ai_status(state()));
    assert_eq!(serde_json::to_value(bangumi_status(state())).unwrap(), "unconfigured");
    code(test_bangumi(state(), Some("invalid token".into())).await, ErrorCode::BangumiConnectionFailed);
    code(save_bangumi(state(), "invalid token".into()).await, ErrorCode::BangumiSettingsFailed);
    let before = serde_json::to_value(language_settings(state())).unwrap();
    code(save_languages(state(), vec!["français".into()]).await, ErrorCode::LanguageSettingsFailed);
    assert_eq!(serde_json::to_value(language_settings(state())).unwrap(), before);
    let view = save_languages(state(), vec!["ja".into(), "zh-CN".into()]).await.unwrap();
    let current = serde_json::to_value(language_settings(state())).unwrap();
    assert_eq!(current, serde_json::to_value(view).unwrap());
    assert_ne!(current, before);

    code(create_ai_batch(state(), serde_json::from_value(json!({"items":[]})).unwrap()).await,
        ErrorCode::AiBatchCreateFailed);
    code(create_folder_batch(state(), vec![]).await, ErrorCode::BatchCreateFailed);
    code(Box::pin(scrape_folder_work(state(), 1, 2, vec!["../outside.mkv".into()], None)).await,
        ErrorCode::FolderScrapeFailed);
    code(correct_batch_match(state(), 1, 2, serde_json::from_value(json!({"provider":"tmdb","id":"10"})).unwrap()).await,
        ErrorCode::BatchMatchFailed);
    code(preview_writeback(state(), 1, None).await, ErrorCode::WritebackPreviewFailed);
    code(queue_writeback(state(), 1, None, "invalid".into()).await, ErrorCode::WritebackQueueFailed);

    assert!(emby_catalog(state()).await.unwrap().is_none());
    code(preview_emby(state(), "7".into()).await, ErrorCode::EmbyLibraryFailed);
    code(emby_work_sources(state(), "7".into()).await, ErrorCode::EmbyDetailsFailed);
    code(emby_credits(state(), "7".into()).await, ErrorCode::EmbyDetailsFailed);
    code(emby_series_preview(state(), "7".into()).await, ErrorCode::EmbyDetailsFailed);
    code(emby_episode_overview(state(), "7".into()).await, ErrorCode::EmbyDetailsFailed);
    code(emby_missing_episode_overviews(state(), "7".into()).await, ErrorCode::EmbyDetailsFailed);
    code(emby_preview_image(state(), "7".into(), EmbyPreviewImage::Poster).await,
        ErrorCode::EmbyImageUnavailable);
    code(import_emby_catalog(state()).await, ErrorCode::EmbyImportFailed);
    code(import_emby_item(state(), "7".into(), EmbyScrapeScope::Series).await,
        ErrorCode::EmbyImportFailed);
    code(import_emby_selection(state(), serde_json::from_value(json!({"items":[]})).unwrap()).await,
        ErrorCode::EmbyImportFailed);
    let target = || EmbyScrapeTarget::Item { id: "7".into(), scope: EmbyScrapeScope::Series };
    code(refresh_emby_metadata(state(), target()).await, ErrorCode::EmbyRefreshFailed);
    let overwrite = serde_json::from_value(json!({"target":{"type":"item","id":"7","scope":"series"}})).unwrap();
    code(overwrite_emby(state(), overwrite).await, ErrorCode::EmbyOverwriteFailed);
}
