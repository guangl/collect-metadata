#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
#[path = "../tests/desktop.rs"]
mod tests;

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, RwLock},
};

#[cfg(test)]
use collect_metadata::scan_folder_paths;
use collect_metadata::{
    AiBatchInput, BatchAction, BatchItemInput, BatchRow, BatchSummary, BatchView, CandidateChoice,
    Collector, EmbyInput, EmbyLibrary, EmbyLibraryPreview, EmbyOverwriteInput, EmbyPreviewImage,
    EmbyScrapeScope, EmbySelectionInput, EmbySeriesPreview, EmbySettings, EmbySettingsView,
    EmbyWorkPreview, FolderBatchResult, FolderSelection, HistoryItem, LanguageSettings,
    LanguageSettingsView, LookupResponse, MediaKind, MediaProvider, OutputOptions, OutputSettings,
    OutputSettingsView, SearchResult, TmdbSettings, TmdbStatus, diagnostics, scan_native_folder,
};
use serde_json::{Value, json};
use tauri::{
    AppHandle, Manager, State, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

struct Runtime {
    collector: Arc<Collector>,
    worker: tauri::async_runtime::JoinHandle<()>,
}

struct DesktopState {
    runtime: RwLock<Runtime>,
    config_lock: tokio::sync::Mutex<()>,
    database_file: PathBuf,
    tmdb: Arc<TmdbSettings>,
    output: Arc<OutputSettings>,
    emby: Arc<EmbySettings>,
}

impl Runtime {
    fn start(collector: Collector) -> Self {
        let collector = Arc::new(collector);
        let worker = tauri::async_runtime::spawn(Arc::clone(&collector).run_batch_worker());
        Self { collector, worker }
    }
}

impl DesktopState {
    fn collector(&self) -> Arc<Collector> {
        Arc::clone(&self.runtime.read().expect("desktop runtime lock").collector)
    }
}

#[tauri::command]
async fn configure_database(state: State<'_, DesktopState>, url: String) -> Result<(), String> {
    diagnostics::desktop_command("configure_database", async {
        let _config = state.config_lock.lock().await;
        if env::var("DATABASE_URL")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
            || env::var_os("PGHOST").is_some()
        {
            return Err("当前使用环境变量配置 PostgreSQL，请修改环境变量后重新启动。".into());
        }
        let url = url.trim();
        if url.is_empty() {
            return Err("请输入 PostgreSQL 连接地址。".into());
        }
        let collector = Collector::from_database_url(url)?
            .with_ai_config_file(&state.database_file.with_file_name("ai.json"))?
            .with_tmdb_settings(Arc::clone(&state.tmdb))
            .with_output_settings(Arc::clone(&state.output))
            .with_emby_settings(Arc::clone(&state.emby))
            .with_language_settings(Arc::clone(state.collector().language_settings()));
        collector.init_storage().await?;
        let path = &state.database_file;
        let parent = path.parent().ok_or("配置路径无效。")?;
        let save = || -> std::io::Result<()> {
            fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            writeln!(file, "{url}")?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        };
        save().map_err(
            |_| "PostgreSQL 连接成功，但本机配置保存失败。请检查应用配置目录权限和磁盘空间。",
        )?;
        let mut runtime = state.runtime.write().expect("desktop runtime lock");
        runtime.worker.abort();
        *runtime = Runtime::start(collector);
        Ok(())
    })
    .await
}

#[tauri::command]
fn tmdb_status(state: State<'_, DesktopState>) -> TmdbStatus {
    state.collector().tmdb_status()
}

#[tauri::command]
async fn test_tmdb(
    state: State<'_, DesktopState>,
    credential: Option<String>,
) -> Result<(), String> {
    diagnostics::desktop_command(
        "test_tmdb",
        state.collector().test_tmdb(credential.as_deref()),
    )
    .await
}

#[tauri::command]
async fn save_tmdb(
    state: State<'_, DesktopState>,
    credential: String,
) -> Result<TmdbStatus, String> {
    diagnostics::desktop_command("save_tmdb", state.collector().save_tmdb(credential)).await
}

#[tauri::command]
fn ai_status(state: State<'_, DesktopState>) -> bool {
    state.collector().ai_configured()
}

#[tauri::command]
async fn create_ai_batch(
    state: State<'_, DesktopState>,
    input: AiBatchInput,
) -> Result<i64, String> {
    diagnostics::desktop_command("create_ai_batch", state.collector().create_ai_batch(input)).await
}

#[tauri::command]
async fn import_emby_selection(
    state: State<'_, DesktopState>,
    input: EmbySelectionInput,
) -> Result<i64, String> {
    diagnostics::desktop_command(
        "import_emby_selection",
        state.collector().import_emby_selection(input),
    )
    .await
}

#[tauri::command]
async fn correct_batch_match(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    candidate: CandidateChoice,
) -> Result<(), String> {
    diagnostics::desktop_command("correct_batch_match", async {
        state
            .collector()
            .correct_batch_match(batch, row, candidate)
            .await
    })
    .await
}

#[tauri::command]
async fn emby_libraries(state: State<'_, DesktopState>) -> Result<Vec<EmbyLibrary>, String> {
    diagnostics::desktop_command("emby_libraries", state.collector().emby_libraries()).await
}

#[tauri::command]
async fn preview_emby(
    state: State<'_, DesktopState>,
    library: String,
) -> Result<Vec<EmbyWorkPreview>, String> {
    diagnostics::desktop_command(
        "preview_emby",
        state.collector().preview_emby_library(&library),
    )
    .await
}

#[tauri::command]
async fn import_emby(state: State<'_, DesktopState>, library: String) -> Result<i64, String> {
    diagnostics::desktop_command(
        "import_emby",
        state.collector().import_emby_library(&library),
    )
    .await
}

#[tauri::command]
async fn emby_catalog(
    state: State<'_, DesktopState>,
) -> Result<Option<Vec<EmbyLibraryPreview>>, String> {
    diagnostics::desktop_command("emby_catalog", state.collector().emby_catalog()).await
}

#[tauri::command]
async fn import_emby_catalog(state: State<'_, DesktopState>) -> Result<i64, String> {
    diagnostics::desktop_command(
        "import_emby_catalog",
        state.collector().import_emby_catalog(),
    )
    .await
}

#[tauri::command]
async fn import_emby_item(
    state: State<'_, DesktopState>,
    id: String,
    scope: EmbyScrapeScope,
) -> Result<i64, String> {
    diagnostics::desktop_command(
        "import_emby_item",
        state.collector().import_emby_item(&id, scope),
    )
    .await
}

#[tauri::command]
async fn overwrite_emby(
    state: State<'_, DesktopState>,
    input: EmbyOverwriteInput,
) -> Result<(), String> {
    diagnostics::desktop_command("overwrite_emby", state.collector().overwrite_emby(input)).await
}

#[tauri::command]
async fn emby_preview_image(
    state: State<'_, DesktopState>,
    id: String,
    image: EmbyPreviewImage,
) -> Result<tauri::ipc::Response, String> {
    let (bytes, _) = state
        .collector()
        .emby_preview_image(&id, image)
        .await
        .ok_or("Emby 图片暂不可用。")?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
async fn emby_credits(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<Vec<collect_metadata::EmbyCredit>, String> {
    diagnostics::desktop_command("emby_credits", state.collector().emby_credits(&id)).await
}

#[tauri::command]
async fn emby_series_preview(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<EmbySeriesPreview, String> {
    diagnostics::desktop_command(
        "emby_series_preview",
        state.collector().emby_series_preview(&id),
    )
    .await
}

#[tauri::command]
fn language_settings(state: State<'_, DesktopState>) -> LanguageSettingsView {
    state.collector().language_settings().view()
}

#[tauri::command]
async fn save_languages(
    state: State<'_, DesktopState>,
    languages: Vec<String>,
) -> Result<LanguageSettingsView, String> {
    diagnostics::desktop_command("save_languages", async {
        let settings = Arc::clone(state.collector().language_settings());
        tauri::async_runtime::spawn_blocking(move || settings.save(languages))
            .await
            .map_err(|_| "无法保存元数据语言。".to_owned())?
    })
    .await
}

#[tauri::command]
fn emby_settings(state: State<'_, DesktopState>) -> EmbySettingsView {
    state.emby.view()
}

#[tauri::command]
async fn test_emby(state: State<'_, DesktopState>, input: EmbyInput) -> Result<(), String> {
    diagnostics::desktop_command("test_emby", state.collector().test_emby(input)).await
}

#[tauri::command]
async fn save_emby(
    state: State<'_, DesktopState>,
    input: EmbyInput,
) -> Result<EmbySettingsView, String> {
    diagnostics::desktop_command("save_emby", state.collector().save_emby(input)).await
}

#[tauri::command]
fn output_settings(state: State<'_, DesktopState>) -> OutputSettingsView {
    state.output.view()
}

#[tauri::command]
async fn save_output_settings(
    state: State<'_, DesktopState>,
    options: OutputOptions,
) -> Result<OutputSettingsView, String> {
    diagnostics::desktop_command("save_output_settings", async {
        let settings = Arc::clone(&state.output);
        tauri::async_runtime::spawn_blocking(move || settings.save(options))
            .await
            .map_err(|_| "无法保存输出设置。".to_owned())?
    })
    .await
}

#[tauri::command]
async fn pick_output_directory(app: AppHandle) -> Result<Option<String>, String> {
    diagnostics::desktop_command("pick_output_directory", async {
        tauri::async_runtime::spawn_blocking(move || {
            let root = app
                .dialog()
                .file()
                .set_title("选择刮削输出目录")
                .blocking_pick_folder();
            root.map(|path| {
                path.into_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|_| "无法读取输出目录。".to_owned())
            })
            .transpose()
        })
        .await
        .map_err(|_| "无法打开目录选择器。".to_owned())?
    })
    .await
}

#[tauri::command]
async fn create_batch(
    state: State<'_, DesktopState>,
    items: Vec<BatchItemInput>,
) -> Result<i64, String> {
    diagnostics::desktop_command("create_batch", state.collector().create_batch(items)).await
}

#[tauri::command]
async fn choose_media_folder(app: AppHandle) -> Result<Option<FolderSelection>, String> {
    diagnostics::desktop_command("choose_media_folder", async {
        tauri::async_runtime::spawn_blocking(move || {
            let Some(root) = app
                .dialog()
                .file()
                .set_title("选择媒体文件夹")
                .blocking_pick_folder()
            else {
                return Ok(None);
            };
            let root = root.into_path().map_err(|_| "无法获取所选文件夹的路径。")?;
            scan_native_folder(&root).map(Some)
        })
        .await
        .map_err(|_| "无法扫描所选文件夹。")?
    })
    .await
}

#[tauri::command]
async fn create_folder_batch(
    state: State<'_, DesktopState>,
    paths: Vec<String>,
) -> Result<FolderBatchResult, String> {
    diagnostics::desktop_command(
        "create_folder_batch",
        state.collector().create_folder_batch(paths),
    )
    .await
}

#[tauri::command]
async fn list_batches(
    state: State<'_, DesktopState>,
    before: Option<i64>,
) -> Result<Vec<BatchSummary>, String> {
    diagnostics::desktop_command("list_batches", state.collector().list_batches(before)).await
}

#[tauri::command]
async fn batch_rows(state: State<'_, DesktopState>, batch: i64) -> Result<Vec<BatchRow>, String> {
    diagnostics::desktop_command("batch_rows", state.collector().batch_rows(batch)).await
}

#[tauri::command]
async fn batch_view(state: State<'_, DesktopState>, batch: i64) -> Result<BatchView, String> {
    diagnostics::desktop_command("batch_view", state.collector().batch_view(batch)).await
}

#[tauri::command]
async fn batch_files(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<Vec<String>, String> {
    diagnostics::desktop_command("batch_files", state.collector().batch_files(batch, row)).await
}

#[tauri::command]
async fn control_batch(
    state: State<'_, DesktopState>,
    batch: i64,
    action: BatchAction,
) -> Result<u64, String> {
    diagnostics::desktop_command(
        "control_batch",
        state.collector().control_batch(batch, action),
    )
    .await
}

#[tauri::command]
async fn batch_result(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<LookupResponse, String> {
    diagnostics::desktop_command(
        "batch_result",
        state.collector().batch_row_result(batch, row),
    )
    .await
}

#[tauri::command]
async fn preview_writeback(
    state: State<'_, DesktopState>,
    batch: i64,
    row: Option<i64>,
) -> Result<collect_metadata::WritebackPreview, String> {
    diagnostics::desktop_command(
        "preview_writeback",
        state.collector().preview_writeback(batch, row),
    )
    .await
}

#[tauri::command]
async fn queue_writeback(
    state: State<'_, DesktopState>,
    batch: i64,
    row: Option<i64>,
    token: String,
) -> Result<usize, String> {
    diagnostics::desktop_command(
        "queue_writeback",
        state.collector().queue_writeback(batch, row, &token),
    )
    .await
}

#[tauri::command]
async fn retry_batch_row(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<(), String> {
    diagnostics::desktop_command(
        "retry_batch_row",
        state.collector().retry_batch_row(batch, row),
    )
    .await
}

#[tauri::command]
async fn choose_batch_candidate(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    candidate: CandidateChoice,
) -> Result<(), String> {
    diagnostics::desktop_command("choose_batch_candidate", async {
        state
            .collector()
            .select_batch_candidate(batch, row, candidate)
            .await
    })
    .await
}

#[tauri::command]
async fn lookup(state: State<'_, DesktopState>, query: String) -> Result<LookupResponse, String> {
    diagnostics::desktop_command("lookup", state.collector().lookup(&query)).await
}

#[tauri::command]
async fn search_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    query: String,
) -> Result<SearchResult, String> {
    diagnostics::desktop_command("search_media", state.collector().search_media(kind, &query)).await
}

#[tauri::command]
async fn collect_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    provider: MediaProvider,
    id: String,
) -> Result<LookupResponse, String> {
    diagnostics::desktop_command(
        "collect_media",
        state.collector().collect_media(kind, provider, &id),
    )
    .await
}

#[tauri::command]
async fn history_languages(state: State<'_, DesktopState>) -> Result<Vec<String>, String> {
    diagnostics::desktop_command("history_languages", state.collector().history_languages()).await
}

#[tauri::command]
async fn history(
    state: State<'_, DesktopState>,
    before: Option<i64>,
    kind: Option<MediaKind>,
    language: Option<String>,
) -> Result<Vec<HistoryItem>, String> {
    diagnostics::desktop_command("history", async {
        state
            .collector()
            .history(before, kind, language.as_deref())
            .await
    })
    .await
}

#[tauri::command]
async fn load_snapshot(state: State<'_, DesktopState>, id: i64) -> Result<LookupResponse, String> {
    diagnostics::desktop_command("load_snapshot", state.collector().load(id)).await
}

#[tauri::command]
fn storage_configured(state: State<'_, DesktopState>) -> bool {
    state.collector().storage_configured()
}

#[tauri::command]
async fn check_storage(state: State<'_, DesktopState>) -> Result<(), String> {
    diagnostics::desktop_command("check_storage", state.collector().check_storage()).await
}

#[tauri::command]
async fn export_json(app: AppHandle, code: String, document: Value) -> Result<bool, String> {
    diagnostics::desktop_command("export_json", async {
        let filename = code
            .chars()
            .take(64)
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>();
        tauri::async_runtime::spawn_blocking(move || {
            let Some(file) = app
                .dialog()
                .file()
                .set_title("导出采集结果")
                .add_filter("JSON", &["json"])
                .set_file_name(format!("{filename}.json"))
                .blocking_save_file()
            else {
                return Ok(false);
            };
            let mut file = file.into_path().map_err(|_| "无法读取所选导出路径。")?;
            if file.extension().is_none() {
                file.set_extension("json");
            }
            let directory = file.parent().ok_or("保存路径无效。")?;
            let mut temporary = tempfile::NamedTempFile::new_in(directory)
                .map_err(|_| "无法在目标目录创建导出文件。请检查路径和写入权限。")?;
            let write_error = "无法写入导出文件。请检查目标目录的磁盘空间和权限。";
            serde_json::to_writer_pretty(&mut temporary, &document).map_err(|_| write_error)?;
            temporary.write_all(b"\n").map_err(|_| write_error)?;
            temporary.as_file().sync_all().map_err(|_| write_error)?;
            temporary
                .persist(&file)
                .map_err(|_| "无法保存导出文件。请检查目标文件是否被占用或目录是否可写。")?;
            Ok(true)
        })
        .await
        .map_err(|_| "导出任务异常结束。请重试或查看应用日志。".to_owned())?
    })
    .await
}

#[tauri::command]
async fn desktop_log_directory() -> Result<String, String> {
    diagnostics::desktop_command("desktop_log_directory", async {
        Ok(diagnostics::desktop_log_directory()?
            .to_string_lossy()
            .into_owned())
    })
    .await
}

#[tauri::command]
async fn open_desktop_logs(app: AppHandle) -> Result<(), String> {
    diagnostics::desktop_command("open_desktop_logs", async {
        let directory = diagnostics::desktop_log_directory()?;
        app.opener()
            .open_path(directory.to_string_lossy(), None::<&str>)
            .map_err(|_| "无法打开日志目录，请在文件管理器中打开显示的路径。".into())
    })
    .await
}

fn setup_desktop(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let path = diagnostics::startup_step("config_directory", app.path().app_config_dir())?
        .join("database-url");
    let url = match fs::read_to_string(&path) {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return diagnostics::startup_step("database_config", Err(error.into())),
    };
    let tmdb = Arc::new(
        diagnostics::startup_step(
            "tmdb_config",
            TmdbSettings::from_file(path.with_file_name("tmdb-token")),
        )
        .map_err(std::io::Error::other)?,
    );
    let output = Arc::new(
        diagnostics::startup_step(
            "output_config",
            OutputSettings::load(path.with_file_name("output-settings.json"), None),
        )
        .map_err(std::io::Error::other)?,
    );
    let emby = Arc::new(
        diagnostics::startup_step(
            "emby_config",
            EmbySettings::load(path.with_file_name("emby-settings.json")),
        )
        .map_err(std::io::Error::other)?,
    );
    let languages = Arc::new(
        diagnostics::startup_step(
            "language_config",
            LanguageSettings::load(Some(path.with_file_name("metadata-languages.json"))),
        )
        .map_err(std::io::Error::other)?,
    );
    let collector = diagnostics::startup_step("collector_config", Collector::from_environment(url))
        .map_err(std::io::Error::other)?;
    let collector = diagnostics::startup_step(
        "ai_config",
        collector.with_ai_config_file(&path.with_file_name("ai.json")),
    )
    .map_err(std::io::Error::other)?
    .with_tmdb_settings(Arc::clone(&tmdb))
    .with_output_settings(Arc::clone(&output))
    .with_emby_settings(Arc::clone(&emby))
    .with_language_settings(languages);
    app.manage(DesktopState {
        runtime: RwLock::new(Runtime::start(collector)),
        config_lock: tokio::sync::Mutex::new(()),
        database_file: path,
        tmdb,
        output,
        emby,
    });
    let open = MenuItem::with_id(app, "open", "打开 Collect Metadata", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 Collect Metadata", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Collect Metadata")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            match app.path().app_log_dir() {
                Ok(directory) => diagnostics::init_desktop(directory),
                Err(_) => diagnostics::error("desktop_log_directory_unavailable", json!({})),
            }
            diagnostics::info(
                "app_starting",
                json!({"mode": "desktop", "version": env!("CARGO_PKG_VERSION")}),
            );
            setup_desktop(app).inspect_err(|_| {
                diagnostics::error("desktop_setup_failed", json!({}));
            })
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && let Some(state) = window.try_state::<DesktopState>()
                && state.collector().storage_configured()
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            configure_database,
            desktop_log_directory,
            open_desktop_logs,
            tmdb_status,
            test_tmdb,
            save_tmdb,
            output_settings,
            emby_settings,
            language_settings,
            save_languages,
            ai_status,
            create_ai_batch,
            import_emby_selection,
            correct_batch_match,
            emby_libraries,
            preview_emby,
            import_emby,
            emby_catalog,
            import_emby_catalog,
            import_emby_item,
            overwrite_emby,
            emby_preview_image,
            emby_series_preview,
            emby_credits,
            test_emby,
            save_emby,
            save_output_settings,
            pick_output_directory,
            create_batch,
            choose_media_folder,
            create_folder_batch,
            list_batches,
            batch_rows,
            batch_view,
            batch_files,
            control_batch,
            batch_result,
            retry_batch_row,
            preview_writeback,
            queue_writeback,
            choose_batch_candidate,
            lookup,
            search_media,
            collect_media,
            history_languages,
            history,
            load_snapshot,
            storage_configured,
            check_storage,
            export_json
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Collect Metadata")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_main(_app);
            }
        });
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
