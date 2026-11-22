use serde::Serialize;

use crate::diagnostics;

// Serialized names are part of the Web and desktop error contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidInput,
    ResourceNotFound,
    UnsupportedMethod,
    RequestTooLarge,
    UnsupportedMediaType,
    Unauthorized,
    AccessDenied,
    RateLimited,
    ServiceUnavailable,
    InternalError,
    StorageCheckFailed,
    StorageSettingsFailed,
    TmdbConnectionFailed,
    TmdbSettingsFailed,
    BangumiConnectionFailed,
    BangumiSettingsFailed,
    LanguageSettingsFailed,
    OutputSettingsFailed,
    OutputFolderFailed,
    EmbyConnectionFailed,
    EmbySettingsFailed,
    EmbyLibraryFailed,
    EmbyImportFailed,
    EmbyOverwriteFailed,
    EmbyRefreshFailed,
    EmbyDetailsFailed,
    EmbyImageUnavailable,
    EmbySyncFailed,
    MediaLookupFailed,
    MediaSearchFailed,
    MediaCollectionFailed,
    HistoryReadFailed,
    HistoryClearFailed,
    BatchCreateFailed,
    AiBatchCreateFailed,
    BatchReadFailed,
    BatchUpdateFailed,
    BatchMatchFailed,
    FolderScanFailed,
    FolderScrapeFailed,
    WritebackPreviewFailed,
    WritebackQueueFailed,
    JsonExportFailed,
    LogAccessFailed,
}

#[derive(Debug, Serialize)]
pub struct AppError {
    pub code: ErrorCode,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_id: Option<String>,
}

impl AppError {
    pub fn new(code: ErrorCode, error: impl Into<String>) -> Self {
        Self {
            code,
            error: error.into(),
            diagnostic_id: diagnostics::current_request_id(),
        }
    }
}
