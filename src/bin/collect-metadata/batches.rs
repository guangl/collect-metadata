use collect_metadata::{
    AiBatchInput, AppError, BatchAction, BatchItemInput, BatchRow, BatchSummary, BatchView,
    CandidateChoice, ErrorCode, FolderBatchResult, FolderSelection, FolderWorkResult,
    LookupResponse, diagnostics, scan_native_folder,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use super::DesktopState;

#[tauri::command]
pub(crate) async fn create_ai_batch(
    state: State<'_, DesktopState>,
    input: AiBatchInput,
) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "create_ai_batch",
        ErrorCode::AiBatchCreateFailed,
        state.collector().create_ai_batch(input),
    )
    .await
}

#[tauri::command]
pub(crate) async fn correct_batch_match(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    candidate: CandidateChoice,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "correct_batch_match",
        ErrorCode::BatchMatchFailed,
        state.collector().correct_batch_match(batch, row, candidate),
    )
    .await
}

#[tauri::command]
pub(crate) async fn create_batch(
    state: State<'_, DesktopState>,
    items: Vec<BatchItemInput>,
) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "create_batch",
        ErrorCode::BatchCreateFailed,
        state.collector().create_batch(items),
    )
    .await
}

#[tauri::command]
pub(crate) async fn choose_media_folder(
    app: AppHandle,
) -> Result<Option<FolderSelection>, AppError> {
    let work = async {
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
    };
    diagnostics::desktop_command("choose_media_folder", ErrorCode::FolderScanFailed, work).await
}

#[tauri::command]
pub(crate) async fn scrape_folder_work(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    paths: Vec<String>,
    candidate: Option<CandidateChoice>,
) -> Result<FolderWorkResult, AppError> {
    diagnostics::desktop_command(
        "scrape_folder_work",
        ErrorCode::FolderScrapeFailed,
        state
            .collector()
            .scrape_folder_work(batch, row, paths, candidate),
    )
    .await
}

#[tauri::command]
pub(crate) async fn create_folder_batch(
    state: State<'_, DesktopState>,
    paths: Vec<String>,
) -> Result<FolderBatchResult, AppError> {
    diagnostics::desktop_command(
        "create_folder_batch",
        ErrorCode::BatchCreateFailed,
        state.collector().create_folder_batch(paths),
    )
    .await
}

#[tauri::command]
pub(crate) async fn list_batches(
    state: State<'_, DesktopState>,
    before: Option<i64>,
) -> Result<Vec<BatchSummary>, AppError> {
    diagnostics::desktop_command(
        "list_batches",
        ErrorCode::BatchReadFailed,
        state.collector().list_batches(before),
    )
    .await
}

#[tauri::command]
pub(crate) async fn batch_rows(
    state: State<'_, DesktopState>,
    batch: i64,
) -> Result<Vec<BatchRow>, AppError> {
    diagnostics::desktop_command(
        "batch_rows",
        ErrorCode::BatchReadFailed,
        state.collector().batch_rows(batch),
    )
    .await
}

#[tauri::command]
pub(crate) async fn batch_view(
    state: State<'_, DesktopState>,
    batch: i64,
) -> Result<BatchView, AppError> {
    diagnostics::desktop_command(
        "batch_view",
        ErrorCode::BatchReadFailed,
        state.collector().batch_view(batch),
    )
    .await
}

#[tauri::command]
pub(crate) async fn batch_files(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<Vec<String>, AppError> {
    diagnostics::desktop_command(
        "batch_files",
        ErrorCode::BatchReadFailed,
        state.collector().batch_files(batch, row),
    )
    .await
}

#[tauri::command]
pub(crate) async fn control_batch(
    state: State<'_, DesktopState>,
    batch: i64,
    action: BatchAction,
) -> Result<u64, AppError> {
    diagnostics::desktop_command(
        "control_batch",
        ErrorCode::BatchUpdateFailed,
        state.collector().control_batch(batch, action),
    )
    .await
}

#[tauri::command]
pub(crate) async fn batch_result(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<LookupResponse, AppError> {
    diagnostics::desktop_command(
        "batch_result",
        ErrorCode::BatchReadFailed,
        state.collector().batch_row_result(batch, row),
    )
    .await
}

#[tauri::command]
pub(crate) async fn preview_writeback(
    state: State<'_, DesktopState>,
    batch: i64,
    row: Option<i64>,
) -> Result<collect_metadata::WritebackPreview, AppError> {
    diagnostics::desktop_command(
        "preview_writeback",
        ErrorCode::WritebackPreviewFailed,
        state.collector().preview_writeback(batch, row),
    )
    .await
}

#[tauri::command]
pub(crate) async fn queue_writeback(
    state: State<'_, DesktopState>,
    batch: i64,
    row: Option<i64>,
    token: String,
) -> Result<usize, AppError> {
    diagnostics::desktop_command(
        "queue_writeback",
        ErrorCode::WritebackQueueFailed,
        state.collector().queue_writeback(batch, row, &token),
    )
    .await
}

#[tauri::command]
pub(crate) async fn retry_batch_row(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "retry_batch_row",
        ErrorCode::BatchUpdateFailed,
        state.collector().retry_batch_row(batch, row),
    )
    .await
}

#[tauri::command]
pub(crate) async fn choose_batch_candidate(
    state: State<'_, DesktopState>,
    batch: i64,
    row: i64,
    candidate: CandidateChoice,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "choose_batch_candidate",
        ErrorCode::BatchMatchFailed,
        state
            .collector()
            .select_batch_candidate(batch, row, candidate),
    )
    .await
}
