use std::collections::HashSet;

use futures_util::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;

use crate::{
    Collector, EmbyLibraryPreview, EmbyPreviewImage, EmbySeriesPreview, EmbyWorkPreview, MediaKind, ScrapeSourceLink,
    providers::media::emby::{EPISODE_QUERY_PREFIX, Emby, is_item_id, source_links::work_source_links},
};

use super::{BatchItemInput, MatchMode};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbyScrapeScope {
    Movie,
    Series,
    Episode,
}

impl EmbyScrapeScope {
    fn batch_query(self) -> (MediaKind, &'static str) {
        match self {
            Self::Movie => (MediaKind::Movie, "emby:"),
            Self::Series => (MediaKind::Series, "emby:"),
            Self::Episode => (MediaKind::Series, EPISODE_QUERY_PREFIX),
        }
    }

    fn item_type(self) -> &'static str {
        match self {
            Self::Movie => "Movie,Video",
            Self::Series => "Series",
            Self::Episode => "Episode",
        }
    }
}

#[derive(Deserialize)]
pub struct EmbySelectionInput {
    pub items: Vec<EmbySelectionItem>,
    #[serde(default)]
    pub use_ai: bool,
}

#[derive(Deserialize)]
pub struct EmbySelectionItem {
    pub id: String,
    pub scope: EmbyScrapeScope,
}

#[derive(Deserialize)]
pub struct EmbyOverwriteInput {
    target: EmbyScrapeTarget,
    #[serde(default)]
    replace_images: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbyScrapeTarget {
    Library { id: String },
    Item { id: String, scope: EmbyScrapeScope },
}

fn emby_batch_items(works: impl IntoIterator<Item = EmbyWorkPreview>) -> Vec<BatchItemInput> {
    works
        .into_iter()
        .map(|work| BatchItemInput {
            kind: work.kind,
            query: format!("emby:{}", work.id),
            source_files: Vec::new(),
        })
        .collect()
}

impl Collector {
    pub(super) fn emby_connection(&self) -> Result<std::sync::Arc<Emby>, String> {
        self.emby_settings()
            .and_then(|settings| settings.connection())
            .ok_or_else(|| "请先在「设置 → 家庭 Emby」配置服务地址和 API 密钥。".into())
    }

    pub async fn emby_libraries(&self) -> Result<Vec<crate::EmbyLibrary>, String> {
        self.emby_connection()?.libraries(&self.client).await
    }

    async fn read_emby_library(
        &self,
        id: &str,
    ) -> Result<(crate::EmbyLibrary, Vec<crate::EmbyWorkPreview>), String> {
        let emby = self.emby_connection()?;
        let library = emby.library(&self.client, id).await?;
        let works = emby.library_entries(&self.client, id).await?;
        Ok((library, works))
    }

    pub async fn preview_emby_library(
        &self,
        id: &str,
    ) -> Result<Vec<crate::EmbyWorkPreview>, String> {
        self.read_emby_library(id).await.map(|(_, works)| works)
    }

    async fn read_emby_catalog(&self, emby: &Emby) -> Result<Vec<EmbyLibraryPreview>, String> {
        let libraries = emby.libraries(&self.client).await?;
        stream::iter(libraries.into_iter().map(|library| async move {
            let works = emby
                .library_entries(&self.client, &library.id)
                .await
                .map_err(|error| format!("{}：{error}", library.name))?;
            Ok(EmbyLibraryPreview { library, works })
        }))
        .buffered(4)
        .try_collect()
        .await
    }

    pub async fn emby_catalog(&self) -> Result<Option<Vec<EmbyLibraryPreview>>, String> {
        let Some(emby) = self
            .emby_settings()
            .and_then(|settings| settings.connection())
        else {
            return Ok(None);
        };
        self.read_emby_catalog(&emby).await.map(Some)
    }

    pub async fn emby_preview_image(
        &self,
        id: &str,
        image: EmbyPreviewImage,
    ) -> Option<(Vec<u8>, &'static str)> {
        self.emby_connection()
            .ok()?
            .preview_image(&self.client, id, image)
            .await
    }

    pub async fn emby_work_sources(&self, id: &str) -> Result<Vec<ScrapeSourceLink>, String> {
        let item = self.emby_connection()?.source_link_item(&self.client, id).await?;
        let mut sidecar_urls = Vec::new();
        if item["Type"] != "Series"
            && let Some(path) = item["Path"].as_str()
            && let Some(settings) = self.output_settings()
        {
            let paths = settings.media_paths();
            let path = path.to_owned();
            sidecar_urls = tokio::task::spawn_blocking(move || paths.work_source_urls(&path))
                .await
                .map_err(|_| "无法读取 Emby 作品来源。")?;
        }
        Ok(work_source_links(&self.client, &item, &sidecar_urls).await)
    }

    pub async fn emby_credits(&self, id: &str) -> Result<Vec<crate::EmbyCredit>, String> {
        self.emby_connection()?.credits(&self.client, id).await
    }

    pub async fn emby_missing_episode_overviews(
        &self,
        library: &str,
    ) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
        let emby = self.emby_connection()?;
        emby.library(&self.client, library).await?;
        emby.missing_episode_overviews(&self.client, library).await
    }

    pub async fn emby_series_preview(&self, id: &str) -> Result<EmbySeriesPreview, String> {
        self.emby_connection()?
            .series_preview(&self.client, id)
            .await
    }

    pub async fn import_emby_catalog(&self) -> Result<i64, String> {
        let emby = self.emby_connection()?;
        let libraries = self.read_emby_catalog(&emby).await?;
        let mut seen = HashSet::new();
        let works = libraries
            .into_iter()
            .flat_map(|library| library.works)
            .filter(|work| seen.insert(work.id.clone()));
        self.save_emby_batch("Emby · 全部媒体库", emby_batch_items(works))
            .await
    }

    pub async fn import_emby_library(&self, id: &str) -> Result<i64, String> {
        let (library, works) = self.read_emby_library(id).await?;
        self.save_emby_batch(&format!("Emby · {}", library.name), emby_batch_items(works))
            .await
    }

    pub async fn import_emby_item(&self, id: &str, scope: EmbyScrapeScope) -> Result<i64, String> {
        let (kind, prefix) = scope.batch_query();
        let title = self
            .emby_connection()?
            .scrape_title(&self.client, id, scope.item_type())
            .await?;
        self.save_emby_batch(
            &format!("Emby · {title}"),
            vec![BatchItemInput {
                kind,
                query: format!("{prefix}{id}"),
                source_files: Vec::new(),
            }],
        )
        .await
    }

    pub async fn import_emby_selection(&self, input: EmbySelectionInput) -> Result<i64, String> {
        if !(1..=500).contains(&input.items.len()) {
            return Err("每批可选择 1 到 500 个 Emby 条目。".into());
        }
        self.check_ai(input.use_ai)?;
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for item in input.items {
            if !is_item_id(&item.id) {
                return Err("Emby 条目 ID 无效。".into());
            }
            if input.use_ai && matches!(item.scope, EmbyScrapeScope::Episode) {
                return Err("AI 刮削请选择电影或整部剧集。".into());
            }
            let (kind, prefix) = item.scope.batch_query();
            if seen.insert((prefix, item.id.clone())) {
                items.push(BatchItemInput {
                    kind,
                    query: format!("{prefix}{}", item.id),
                    source_files: Vec::new(),
                });
            }
        }
        self.emby_connection()?;
        let name = if input.use_ai {
            "Emby · AI 刮削"
        } else {
            "Emby · 所选条目"
        };
        let mode = if input.use_ai {
            MatchMode::Ai
        } else {
            MatchMode::FillMissing
        };
        self.save_emby_batch_mode(name, items, mode).await
    }

    pub async fn overwrite_emby(&self, input: EmbyOverwriteInput) -> Result<(), String> {
        self.update_emby_metadata(input.target, true, input.replace_images)
            .await
    }

    pub async fn refresh_emby_metadata(&self, target: EmbyScrapeTarget) -> Result<(), String> {
        self.update_emby_metadata(target, false, false).await
    }

    async fn update_emby_metadata(
        &self,
        target: EmbyScrapeTarget,
        overwrite: bool,
        replace_images: bool,
    ) -> Result<(), String> {
        let emby = self.emby_connection()?;
        let (id, recursive) = match target {
            EmbyScrapeTarget::Library { id } => {
                emby.library(&self.client, &id).await?;
                (id, true)
            }
            EmbyScrapeTarget::Item { id, scope } => {
                emby.item(&self.client, &id, scope.item_type(), "").await?;
                (id, matches!(scope, EmbyScrapeScope::Series))
            }
        };
        emby.refresh_metadata(&self.client, &id, recursive, overwrite, replace_images)
            .await
    }

    async fn save_emby_batch(&self, name: &str, items: Vec<BatchItemInput>) -> Result<i64, String> {
        self.save_emby_batch_mode(name, items, MatchMode::Existing)
            .await
    }

    async fn save_emby_batch_mode(
        &self,
        name: &str,
        items: Vec<BatchItemInput>,
        mode: MatchMode,
    ) -> Result<i64, String> {
        let store = self.require_store().await?;
        if items.is_empty() {
            return Err("此媒体库没有可刮削的电影、剧集或私人影片。".into());
        }
        let settings = self.output_settings().ok_or("后台输出配置不可用。")?;
        let owner = settings.durable_owner()?;
        store
            .create_batch_mode(items, false, Some(name), None, Some(&owner), mode)
            .await
            .map_err(|_| "无法保存 Emby 导入批次。".into())
    }
}
