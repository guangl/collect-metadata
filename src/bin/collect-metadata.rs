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
    BatchAction, BatchItemInput, BatchRow, BatchSummary, BatchView, CandidateChoice, Collector,
    EmbyInput, EmbyLibrary, EmbySettings, EmbySettingsView, EmbyWorkPreview, FolderBatchResult,
    FolderSelection, HistoryItem, LanguageSettings, LanguageSettingsView, LookupResponse,
    MediaKind, MediaProvider, OutputOptions, OutputSettings, OutputSettingsView, SearchResult,
    TmdbSettings, TmdbStatus, scan_native_folder,
};
use serde_json::Value;
use tauri::{
    AppHandle, Manager, State, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_dialog::DialogExt;

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
    save().map_err(|_| "已连接 PostgreSQL，但无法保存本机配置。")?;
    let mut runtime = state.runtime.write().expect("desktop runtime lock");
    runtime.worker.abort();
    *runtime = Runtime::start(collector);
    Ok(())
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
    state.collector().test_tmdb(credential.as_deref()).await
}

#[tauri::command]
async fn save_tmdb(
    state: State<'_, DesktopState>,
    credential: String,
) -> Result<TmdbStatus, String> {
    state.collector().save_tmdb(credential).await
}

#[tauri::command]
async fn emby_libraries(state: State<'_, DesktopState>) -> Result<Vec<EmbyLibrary>, String> {
    state.collector().emby_libraries().await
}

#[tauri::command]
async fn preview_emby(
    state: State<'_, DesktopState>,
    library: String,
) -> Result<Vec<EmbyWorkPreview>, String> {
    state.collector().preview_emby_library(&library).await
}

#[tauri::command]
async fn import_emby(state: State<'_, DesktopState>, library: String) -> Result<i64, String> {
    state.collector().import_emby_library(&library).await
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
    let settings = Arc::clone(state.collector().language_settings());
    tauri::async_runtime::spawn_blocking(move || settings.save(languages))
        .await
        .map_err(|_| "无法保存元数据语言。".to_owned())?
}

#[tauri::command]
fn emby_settings(state: State<'_, DesktopState>) -> EmbySettingsView {
    state.emby.view()
}

#[tauri::command]
async fn test_emby(state: State<'_, DesktopState>, input: EmbyInput) -> Result<(), String> {
    state.collector().test_emby(input).await
}

#[tauri::command]
async fn save_emby(
    state: State<'_, DesktopState>,
    input: EmbyInput,
) -> Result<EmbySettingsView, String> {
    state.collector().save_emby(input).await
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
    let settings = Arc::clone(&state.output);
    tauri::async_runtime::spawn_blocking(move || settings.save(options))
        .await
        .map_err(|_| "无法保存输出设置。".to_owned())?
}

#[tauri::command]
async fn pick_output_directory(app: AppHandle) -> Result<Option<String>, String> {
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
}

#[tauri::command]
async fn create_batch(
    state: State<'_, DesktopState>,
    items: Vec<BatchItemInput>,
) -> Result<i64, String> {
    state.collector().create_batch(items).await
}

#[tauri::command]
async fn choose_media_folder(app: AppHandle) -> Result<Option<FolderSelection>, String> {
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
}

#[tauri::command]
async fn create_folder_batch(
    state: State<'_, DesktopState>,
    paths: Vec<String>,
) -> Result<FolderBatchResult, String> {
    state.collector().create_folder_batch(paths).await
}

#[tauri::command]
async fn list_batches(
    state: State<'_, DesktopState>,
    before: Option<i64>,
) -> Result<Vec<BatchSummary>, String> {
    state.collector().list_batches(before).await
}

#[tauri::command]
async fn batch_rows(state: State<'_, DesktopState>, batch: i64) -> Result<Vec<BatchRow>, String> {
    state.collector().batch_rows(batch).await
}

#[tauri::command]
async fn batch_view(state: State<'_, DesktopState>, batch: i64) -> Result<BatchView, String> {
    state.collector().batch_view(batch).await
}

#[tauri::command]
async fn batch_files(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<Vec<String>, String> {
    state.collector().batch_files(batch, row).await
}

#[tauri::command]
async fn control_batch(
    state: State<'_, DesktopState>,
    batch: i64,
    action: BatchAction,
) -> Result<u64, String> {
    state.collector().control_batch(batch, action).await
}

#[tauri::command]
async fn batch_result(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<LookupResponse, String> {
    state.collector().batch_row_result(batch, row).await
}

#[tauri::command]
async fn retry_batch_row(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<(), String> {
    state.collector().retry_batch_row(batch, row).await
}

#[tauri::command]
async fn choose_batch_candidate(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    candidate: CandidateChoice,
) -> Result<(), String> {
    state
        .collector()
        .select_batch_candidate(batch, row, candidate)
        .await
}

#[tauri::command]
async fn lookup(state: State<'_, DesktopState>, query: String) -> Result<LookupResponse, String> {
    state.collector().lookup(&query).await
}

#[tauri::command]
async fn search_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    query: String,
) -> Result<SearchResult, String> {
    state.collector().search_media(kind, &query).await
}

#[tauri::command]
async fn collect_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    provider: MediaProvider,
    id: String,
) -> Result<LookupResponse, String> {
    state.collector().collect_media(kind, provider, &id).await
}

#[tauri::command]
async fn history_languages(state: State<'_, DesktopState>) -> Result<Vec<String>, String> {
    state.collector().history_languages().await
}

#[tauri::command]
async fn history(
    state: State<'_, DesktopState>,
    before: Option<i64>,
    kind: Option<MediaKind>,
    language: Option<String>,
) -> Result<Vec<HistoryItem>, String> {
    state
        .collector()
        .history(before, kind, language.as_deref())
        .await
}

#[tauri::command]
async fn load_snapshot(state: State<'_, DesktopState>, id: i64) -> Result<LookupResponse, String> {
    state.collector().load(id).await
}

#[tauri::command]
fn storage_configured(state: State<'_, DesktopState>) -> bool {
    state.collector().storage_configured()
}

#[tauri::command]
async fn check_storage(state: State<'_, DesktopState>) -> Result<(), String> {
    state.collector().check_storage().await
}

#[tauri::command]
async fn export_json(app: AppHandle, code: String, document: Value) -> Result<bool, String> {
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
        let mut file = file.into_path().map_err(|error| error.to_string())?;
        if file.extension().is_none() {
            file.set_extension("json");
        }
        let directory = file.parent().ok_or("保存路径无效。")?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut temporary, &document)
            .map_err(|error| error.to_string())?;
        temporary
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        temporary
            .persist(&file)
            .map_err(|error| error.to_string())?;
        Ok(true)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("database-url");
            let url = match fs::read_to_string(&path) {
                Ok(url) if !url.trim().is_empty() => Some(url),
                Ok(_) => None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            let tmdb = Arc::new(
                TmdbSettings::from_file(path.with_file_name("tmdb-token"))
                    .map_err(std::io::Error::other)?,
            );
            let output = Arc::new(
                OutputSettings::load(path.with_file_name("output-settings.json"), None)
                    .map_err(std::io::Error::other)?,
            );
            let emby = Arc::new(
                EmbySettings::load(path.with_file_name("emby-settings.json"))
                    .map_err(std::io::Error::other)?,
            );
            let languages = Arc::new(
                LanguageSettings::load(Some(path.with_file_name("metadata-languages.json")))
                    .map_err(std::io::Error::other)?,
            );
            let collector = Collector::from_environment(url)
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
            tmdb_status,
            test_tmdb,
            save_tmdb,
            output_settings,
            emby_settings,
            language_settings,
            save_languages,
            emby_libraries,
            preview_emby,
            import_emby,
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
