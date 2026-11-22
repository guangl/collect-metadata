use collect_metadata::{
    AppError, EmbyEpisodeOverview, EmbyLibrary, EmbyLibraryPreview, EmbyOverwriteInput,
    EmbyPreviewImage, EmbyScrapeScope, EmbyScrapeTarget, EmbySelectionInput, EmbySeriesPreview,
    EmbyWorkPreview, ErrorCode, diagnostics,
};
use tauri::State;

use super::DesktopState;

#[tauri::command]
pub(crate) async fn import_emby_selection(
    state: State<'_, DesktopState>,
    input: EmbySelectionInput,
) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "import_emby_selection",
        ErrorCode::EmbyImportFailed,
        state.collector().import_emby_selection(input),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_libraries(
    state: State<'_, DesktopState>,
) -> Result<Vec<EmbyLibrary>, AppError> {
    diagnostics::desktop_command(
        "emby_libraries",
        ErrorCode::EmbyLibraryFailed,
        state.collector().emby_libraries(),
    )
    .await
}

#[tauri::command]
pub(crate) async fn preview_emby(
    state: State<'_, DesktopState>,
    library: String,
) -> Result<Vec<EmbyWorkPreview>, AppError> {
    diagnostics::desktop_command(
        "preview_emby",
        ErrorCode::EmbyLibraryFailed,
        state.collector().preview_emby_library(&library),
    )
    .await
}

#[tauri::command]
pub(crate) async fn import_emby(
    state: State<'_, DesktopState>,
    library: String,
) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "import_emby",
        ErrorCode::EmbyImportFailed,
        state.collector().import_emby_library(&library),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_catalog(
    state: State<'_, DesktopState>,
) -> Result<Option<Vec<EmbyLibraryPreview>>, AppError> {
    diagnostics::desktop_command(
        "emby_catalog",
        ErrorCode::EmbyLibraryFailed,
        state.collector().emby_catalog(),
    )
    .await
}

#[tauri::command]
pub(crate) async fn import_emby_catalog(state: State<'_, DesktopState>) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "import_emby_catalog",
        ErrorCode::EmbyImportFailed,
        state.collector().import_emby_catalog(),
    )
    .await
}

#[tauri::command]
pub(crate) async fn import_emby_item(
    state: State<'_, DesktopState>,
    id: String,
    scope: EmbyScrapeScope,
) -> Result<i64, AppError> {
    diagnostics::desktop_command(
        "import_emby_item",
        ErrorCode::EmbyImportFailed,
        state.collector().import_emby_item(&id, scope),
    )
    .await
}

#[tauri::command]
pub(crate) async fn overwrite_emby(
    state: State<'_, DesktopState>,
    input: EmbyOverwriteInput,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "overwrite_emby",
        ErrorCode::EmbyOverwriteFailed,
        state.collector().overwrite_emby(input),
    )
    .await
}

#[tauri::command]
pub(crate) async fn refresh_emby_metadata(
    state: State<'_, DesktopState>,
    target: EmbyScrapeTarget,
) -> Result<(), AppError> {
    diagnostics::desktop_command(
        "refresh_emby_metadata",
        ErrorCode::EmbyRefreshFailed,
        state.collector().refresh_emby_metadata(target),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_preview_image(
    state: State<'_, DesktopState>,
    id: String,
    image: EmbyPreviewImage,
) -> Result<tauri::ipc::Response, AppError> {
    let (bytes, _) = state
        .collector()
        .emby_preview_image(&id, image)
        .await
        .ok_or_else(|| AppError::new(ErrorCode::EmbyImageUnavailable, "Emby 图片暂不可用。"))?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub(crate) async fn emby_credits(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<Vec<collect_metadata::EmbyCredit>, AppError> {
    diagnostics::desktop_command(
        "emby_credits",
        ErrorCode::EmbyDetailsFailed,
        state.collector().emby_credits(&id),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_missing_episode_overviews(
    state: State<'_, DesktopState>,
    library: String,
) -> Result<std::collections::HashMap<String, Vec<String>>, AppError> {
    diagnostics::desktop_command(
        "emby_missing_episode_overviews",
        ErrorCode::EmbyDetailsFailed,
        state.collector().emby_missing_episode_overviews(&library),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_episode_overview(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<EmbyEpisodeOverview, AppError> {
    diagnostics::desktop_command(
        "emby_episode_overview",
        ErrorCode::EmbyDetailsFailed,
        state.collector().emby_episode_overview(&id),
    )
    .await
}

#[tauri::command]
pub(crate) async fn emby_series_preview(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<EmbySeriesPreview, AppError> {
    diagnostics::desktop_command(
        "emby_series_preview",
        ErrorCode::EmbyDetailsFailed,
        state.collector().emby_series_preview(&id),
    )
    .await
}
