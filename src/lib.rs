mod batch;
mod collect;
mod output;
mod providers;
mod storage;

pub use collect::{
    Collector, LanguageSettings, LanguageSettingsView, LookupResponse, TmdbSettings, TmdbStatus,
};
pub use output::{OutputOptions, OutputReport, OutputSettings, OutputSettingsView};
pub use providers::media::emby::{
    EmbyCredit, EmbyInput, EmbyLibrary, EmbyLibraryPreview, EmbyPreviewImage, EmbySeriesPreview, EmbySettings,
    EmbySettingsView, EmbyWorkPreview,
};
pub use providers::media::{MediaKind, MediaProvider, SearchResult};
pub use storage::HistoryItem;

pub use batch::{
    AiBatchInput, BatchAction, BatchItemInput, BatchRow, BatchSummary, BatchView, CandidateChoice,
    EmbyOverwriteInput, EmbyScrapeScope, EmbySelectionInput, FolderBatchResult, FolderPreview,
    FolderSelection, FolderWorkPreview, MAX_FOLDER_FILES, WritebackPreview, preview_folder,
};
#[cfg(feature = "web")]
pub use batch::{AutoScrape, browse_mounted_folders, scan_mounted_folder};
#[cfg(feature = "desktop")]
pub use batch::{scan_folder_paths, scan_native_folder};
