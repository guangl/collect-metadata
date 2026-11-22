use collect_metadata::{AppError, ErrorCode, diagnostics};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub(crate) async fn desktop_log_directory() -> Result<String, AppError> {
    let work = async {
        Ok(diagnostics::desktop_log_directory()?
            .to_string_lossy()
            .into_owned())
    };
    diagnostics::desktop_command("desktop_log_directory", ErrorCode::LogAccessFailed, work).await
}

#[tauri::command]
pub(crate) async fn open_desktop_logs(app: AppHandle) -> Result<(), AppError> {
    let work = async {
        let directory = diagnostics::desktop_log_directory()?;
        app.opener()
            .open_path(directory.to_string_lossy(), None::<&str>)
            .map_err(|_| "无法打开日志目录，请在文件管理器中打开显示的路径。".into())
    };
    diagnostics::desktop_command("open_desktop_logs", ErrorCode::LogAccessFailed, work).await
}
