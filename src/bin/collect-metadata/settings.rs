use collect_metadata::{
    AppError, BangumiStatus, Collector, EmbyInput, EmbySettingsView, ErrorCode,
    LanguageSettingsView, OutputOptions, OutputSettingsView, TmdbStatus, diagnostics,
};
use std::{env, fs, io::Write, sync::Arc};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use super::{DesktopState, Runtime};

#[tauri::command]
pub(crate) async fn configure_database(
    state: State<'_, DesktopState>,
    url: String,
) -> Result<(), AppError> {
    let work = async {
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
            .with_bangumi_settings(Arc::clone(state.collector().bangumi_settings()))
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
    };
    diagnostics::desktop_command("configure_database", ErrorCode::StorageSettingsFailed, work).await
}

#[tauri::command]
pub(crate) fn tmdb_status(state: State<'_, DesktopState>) -> TmdbStatus {
    state.collector().tmdb_status()
}

#[tauri::command]
pub(crate) async fn test_tmdb(
    state: State<'_, DesktopState>,
    credential: Option<String>,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "test_tmdb",
        ErrorCode::TmdbConnectionFailed,
        state.collector().test_tmdb(credential.as_deref()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn save_tmdb(
    state: State<'_, DesktopState>,
    credential: String,
) -> Result<TmdbStatus, AppError> {
    diagnostics::desktop_command(
        "save_tmdb",
        ErrorCode::TmdbSettingsFailed,
        state.collector().save_tmdb(credential),
    )
    .await
}

#[tauri::command]
pub(crate) fn bangumi_status(state: State<'_, DesktopState>) -> BangumiStatus {
    state.collector().bangumi_status()
}

#[tauri::command]
pub(crate) async fn test_bangumi(
    state: State<'_, DesktopState>,
    credential: Option<String>,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "test_bangumi",
        ErrorCode::BangumiConnectionFailed,
        state.collector().test_bangumi(credential.as_deref()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn save_bangumi(
    state: State<'_, DesktopState>,
    credential: String,
) -> Result<BangumiStatus, AppError> {
    diagnostics::desktop_command(
        "save_bangumi",
        ErrorCode::BangumiSettingsFailed,
        state.collector().save_bangumi(credential),
    )
    .await
}

#[tauri::command]
pub(crate) fn ai_status(state: State<'_, DesktopState>) -> bool {
    state.collector().ai_configured()
}

#[tauri::command]
pub(crate) fn language_settings(state: State<'_, DesktopState>) -> LanguageSettingsView {
    state.collector().language_settings().view()
}

#[tauri::command]
pub(crate) async fn save_languages(
    state: State<'_, DesktopState>,
    languages: Vec<String>,
) -> Result<LanguageSettingsView, AppError> {
    let work = async {
        let settings = Arc::clone(state.collector().language_settings());
        tauri::async_runtime::spawn_blocking(move || settings.save(languages))
            .await
            .map_err(|_| "无法保存元数据语言。".to_owned())?
    };
    diagnostics::desktop_command("save_languages", ErrorCode::LanguageSettingsFailed, work).await
}

#[tauri::command]
pub(crate) fn emby_settings(state: State<'_, DesktopState>) -> EmbySettingsView {
    state.emby.view()
}

#[tauri::command]
pub(crate) async fn test_emby(
    state: State<'_, DesktopState>,
    input: EmbyInput,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "test_emby",
        ErrorCode::EmbyConnectionFailed,
        state.collector().test_emby(input),
    )
    .await
}

#[tauri::command]
pub(crate) async fn save_emby(
    state: State<'_, DesktopState>,
    input: EmbyInput,
) -> Result<EmbySettingsView, AppError> {
    diagnostics::desktop_command(
        "save_emby",
        ErrorCode::EmbySettingsFailed,
        state.collector().save_emby(input),
    )
    .await
}

#[tauri::command]
pub(crate) fn output_settings(state: State<'_, DesktopState>) -> OutputSettingsView {
    state.output.view()
}

#[tauri::command]
pub(crate) async fn save_output_settings(
    state: State<'_, DesktopState>,
    options: OutputOptions,
) -> Result<OutputSettingsView, AppError> {
    let work = async {
        let settings = Arc::clone(&state.output);
        tauri::async_runtime::spawn_blocking(move || settings.save(options))
            .await
            .map_err(|_| "无法保存输出设置。".to_owned())?
    };
    diagnostics::desktop_command(
        "save_output_settings",
        ErrorCode::OutputSettingsFailed,
        work,
    )
    .await
}

#[tauri::command]
pub(crate) async fn pick_output_directory(app: AppHandle) -> Result<Option<String>, AppError> {
    let work = async {
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
    };
    diagnostics::desktop_command("pick_output_directory", ErrorCode::OutputFolderFailed, work).await
}

#[tauri::command]
pub(crate) fn storage_configured(state: State<'_, DesktopState>) -> bool {
    state.collector().storage_configured()
}

#[tauri::command]
pub(crate) async fn check_storage(state: State<'_, DesktopState>) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "check_storage",
        ErrorCode::StorageCheckFailed,
        state.collector().check_storage(),
    )
    .await
}
