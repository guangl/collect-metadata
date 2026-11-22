#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "collect-metadata/batches.rs"]
mod batches;
#[path = "collect-metadata/emby.rs"]
mod emby;
#[path = "collect-metadata/logs.rs"]
mod logs;
#[path = "collect-metadata/metadata.rs"]
mod metadata;
#[path = "collect-metadata/settings.rs"]
mod settings;

#[cfg(test)]
#[path = "../tests/desktop.rs"]
mod tests;

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use collect_metadata::{
    BangumiSettings, Collector, EmbySettings, LanguageSettings, OutputSettings, TmdbSettings,
    diagnostics,
};
use serde_json::json;
use tauri::{
    AppHandle, Manager, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

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
    let bangumi = Arc::new(
        diagnostics::startup_step(
            "bangumi_config",
            BangumiSettings::from_file(path.with_file_name("bangumi-token")),
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
    .with_bangumi_settings(bangumi)
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
            settings::configure_database,
            logs::desktop_log_directory,
            logs::open_desktop_logs,
            settings::tmdb_status,
            settings::test_tmdb,
            settings::save_tmdb,
            settings::bangumi_status,
            settings::test_bangumi,
            settings::save_bangumi,
            settings::output_settings,
            settings::emby_settings,
            settings::language_settings,
            settings::save_languages,
            settings::ai_status,
            batches::create_ai_batch,
            emby::import_emby_selection,
            batches::correct_batch_match,
            emby::emby_libraries,
            emby::preview_emby,
            emby::import_emby,
            emby::emby_catalog,
            emby::import_emby_catalog,
            emby::import_emby_item,
            emby::overwrite_emby,
            emby::refresh_emby_metadata,
            emby::emby_preview_image,
            emby::emby_series_preview,
            emby::emby_episode_overview,
            emby::emby_missing_episode_overviews,
            emby::emby_credits,
            emby::emby_work_sources,
            settings::test_emby,
            settings::save_emby,
            settings::save_output_settings,
            settings::pick_output_directory,
            batches::create_batch,
            batches::choose_media_folder,
            batches::create_folder_batch,
            batches::scrape_folder_work,
            batches::list_batches,
            batches::batch_rows,
            batches::batch_view,
            batches::batch_files,
            batches::control_batch,
            batches::batch_result,
            batches::retry_batch_row,
            batches::preview_writeback,
            batches::queue_writeback,
            batches::choose_batch_candidate,
            metadata::lookup,
            metadata::search_media,
            metadata::collect_media,
            metadata::history_languages,
            metadata::history,
            metadata::load_snapshot,
            metadata::clear_history,
            settings::storage_configured,
            settings::check_storage,
            metadata::export_json
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
