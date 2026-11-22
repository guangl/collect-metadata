use collect_metadata::{
    AppError, ErrorCode, HistoryItem, LookupResponse, MediaKind, MediaProvider, SearchResult,
    diagnostics,
};
use serde_json::Value;
use std::io::Write;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use super::DesktopState;

#[tauri::command]
pub(crate) async fn lookup(
    state: State<'_, DesktopState>,
    query: String,
) -> Result<LookupResponse, AppError> {
    diagnostics::desktop_command(
        "lookup",
        ErrorCode::MediaLookupFailed,
        state.collector().lookup(&query),
    )
    .await
}

#[tauri::command]
pub(crate) async fn search_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    query: String,
) -> Result<SearchResult, AppError> {
    diagnostics::desktop_command(
        "search_media",
        ErrorCode::MediaSearchFailed,
        state.collector().search_media(kind, &query),
    )
    .await
}

#[tauri::command]
pub(crate) async fn collect_media(
    state: State<'_, DesktopState>,
    kind: MediaKind,
    provider: MediaProvider,
    id: String,
) -> Result<LookupResponse, AppError> {
    diagnostics::desktop_command(
        "collect_media",
        ErrorCode::MediaCollectionFailed,
        state.collector().collect_media(kind, provider, &id),
    )
    .await
}

#[tauri::command]
pub(crate) async fn history_languages(
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, AppError> {
    diagnostics::desktop_command(
        "history_languages",
        ErrorCode::HistoryReadFailed,
        state.collector().history_languages(),
    )
    .await
}

#[tauri::command]
pub(crate) async fn history(
    state: State<'_, DesktopState>,
    before: Option<i64>,
    kind: Option<MediaKind>,
    language: Option<String>,
) -> Result<Vec<HistoryItem>, AppError> {
    diagnostics::desktop_command(
        "history",
        ErrorCode::HistoryReadFailed,
        state.collector().history(before, kind, language.as_deref()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn load_snapshot(
    state: State<'_, DesktopState>,
    id: i64,
) -> Result<LookupResponse, AppError> {
    diagnostics::desktop_command(
        "load_snapshot",
        ErrorCode::HistoryReadFailed,
        state.collector().load(id),
    )
    .await
}

#[tauri::command]
pub(crate) async fn export_json(
    app: AppHandle,
    code: String,
    document: Value,
) -> Result<bool, AppError> {
    let work = async {
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
    };
    diagnostics::desktop_command("export_json", ErrorCode::JsonExportFailed, work).await
}
