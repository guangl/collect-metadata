use std::{collections::BTreeMap, env, sync::Arc, time::Duration};

use reqwest::Client;
use sqlx::postgres::PgConnectOptions;
use tokio::sync::{OnceCell, Semaphore};

mod bangumi_settings;
mod emby;
mod history;
mod identifier;
mod localization;
mod model;
mod tmdb_settings;

#[cfg(test)]
mod tests;

use crate::{
    EmbySettings, MediaKind, MediaProvider, OutputSettings, SearchResult,
    providers::{av, media},
    storage::Store,
};
pub use bangumi_settings::{BangumiSettings, BangumiStatus};
pub(crate) use identifier::Identifier;
pub use localization::{LanguageSettings, LanguageSettingsView};
use model::Lookup;
pub use model::{EmbyEpisodeOverview, LookupResponse};
pub(crate) use model::{Fields, Source};
pub use tmdb_settings::{TmdbSettings, TmdbStatus};

pub struct Collector {
    pub(crate) client: Client,
    database: Option<PgConnectOptions>,
    store: OnceCell<Store>,
    searches: Semaphore,
    #[cfg(feature = "web")]
    automatic_writes: [tokio::sync::Mutex<()>; 64],
    entities: media::EntityCache,
    tmdb: Arc<TmdbSettings>,
    bangumi: Arc<BangumiSettings>,
    output: Option<Arc<OutputSettings>>,
    emby: Option<Arc<EmbySettings>>,
    languages: Arc<LanguageSettings>,
    pub(crate) ai: Option<crate::batch::ai::Ai>,
}

impl Collector {
    pub fn from_environment(database_file: Option<String>) -> Result<Self, String> {
        let url = env::var("DATABASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or(database_file);
        let options = if let Some(url) = url {
            Some(
                url.trim()
                    .parse()
                    .map_err(|_| "PostgreSQL 连接地址格式无效。")?,
            )
        } else if env::var_os("PGHOST").is_some() {
            Some(PgConnectOptions::new())
        } else {
            None
        };
        Self::new(options).map_err(|error| error.to_string())
    }

    pub fn from_database_url(url: &str) -> Result<Self, String> {
        let options = url
            .trim()
            .parse()
            .map_err(|_| "PostgreSQL 连接地址格式无效。")?;
        Self::new(Some(options)).map_err(|error| error.to_string())
    }

    fn new(database: Option<PgConnectOptions>) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(concat!(
                "guangl/collect-metadata/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/guangl/collect-metadata)"
            ))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            database,
            store: OnceCell::new(),
            searches: Semaphore::new(8),
            #[cfg(feature = "web")]
            automatic_writes: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
            entities: media::EntityCache::default(),
            tmdb: Arc::new(TmdbSettings::from_environment()),
            bangumi: Arc::new(BangumiSettings::from_environment()),
            output: None,
            emby: None,
            languages: Arc::new(LanguageSettings::load(None).expect("default language settings")),
            ai: crate::batch::ai::Ai::from_environment(),
        })
    }

    pub fn with_language_settings(mut self, settings: Arc<LanguageSettings>) -> Self {
        self.languages = settings;
        self
    }

    pub fn language_settings(&self) -> &Arc<LanguageSettings> {
        &self.languages
    }

    pub fn with_tmdb_settings(mut self, settings: Arc<TmdbSettings>) -> Self {
        self.tmdb = settings;
        self
    }

    pub fn with_bangumi_settings(mut self, settings: Arc<BangumiSettings>) -> Self {
        self.bangumi = settings;
        self
    }

    pub fn bangumi_settings(&self) -> &Arc<BangumiSettings> {
        &self.bangumi
    }

    pub fn bangumi_status(&self) -> BangumiStatus {
        self.bangumi.status()
    }

    pub async fn test_bangumi(&self, credential: Option<&str>) -> Result<(), String> {
        let _permit = self.permit()?;
        self.bangumi.test(&self.client, credential).await
    }

    pub async fn save_bangumi(&self, credential: String) -> Result<BangumiStatus, String> {
        let _permit = self.permit()?;
        self.bangumi.save(&self.client, credential).await
    }

    pub fn with_output_settings(mut self, settings: Arc<OutputSettings>) -> Self {
        self.output = Some(settings);
        self
    }

    pub fn output_settings(&self) -> Option<&Arc<OutputSettings>> {
        self.output.as_ref()
    }

    pub fn with_emby_settings(mut self, settings: Arc<EmbySettings>) -> Self {
        self.emby = Some(settings);
        self
    }

    pub fn emby_settings(&self) -> Option<&Arc<EmbySettings>> {
        self.emby.as_ref()
    }

    pub async fn test_emby(&self, options: crate::EmbyInput) -> Result<(), String> {
        self.emby
            .as_ref()
            .ok_or("Emby 设置不可用。")?
            .test(&self.client, options)
            .await
    }

    pub async fn save_emby(
        &self,
        options: crate::EmbyInput,
    ) -> Result<crate::EmbySettingsView, String> {
        self.emby
            .as_ref()
            .ok_or("Emby 设置不可用。")?
            .save(&self.client, options)
            .await
    }

    pub(crate) fn tmdb_configured(&self) -> bool {
        !matches!(self.tmdb_status(), TmdbStatus::Unconfigured)
    }

    pub fn tmdb_status(&self) -> TmdbStatus {
        self.tmdb.status()
    }

    pub async fn test_tmdb(&self, credential: Option<&str>) -> Result<(), String> {
        let _permit = self.permit()?;
        self.tmdb.test(&self.client, credential).await
    }

    pub async fn save_tmdb(&self, credential: String) -> Result<TmdbStatus, String> {
        let _permit = self.permit()?;
        self.tmdb.save(&self.client, credential).await
    }

    fn permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, String> {
        self.searches
            .try_acquire()
            .map_err(|_| "同时采集的请求过多，请稍后重试。".to_owned())
    }

    pub(crate) async fn batch_permit(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.searches
            .acquire()
            .await
            .expect("collector semaphore is never closed")
    }

    pub async fn lookup(&self, query: &str) -> Result<LookupResponse, String> {
        let _permit = self.permit()?;
        Ok(self.save_lookup(self.gather_av(query).await?).await)
    }

    pub(crate) async fn gather_av(&self, query: &str) -> Result<LookupResponse, String> {
        let code = Identifier::parse(query)?;
        let sources = av::collect(&self.client, &code).await;
        let lookup = make_lookup(MediaKind::Av, &code.formatted, None, sources)?;
        Ok(self
            .localize_lookup(lookup, self.languages.targets(None))
            .await)
    }

    pub async fn search_media(&self, kind: MediaKind, query: &str) -> Result<SearchResult, String> {
        let _permit = self.permit()?;
        self.gather_candidates(kind, query).await
    }

    pub(crate) async fn gather_candidates(
        &self,
        kind: MediaKind,
        query: &str,
    ) -> Result<SearchResult, String> {
        let query = query
            .trim()
            .trim_start_matches('《')
            .trim_end_matches('》')
            .trim();
        if kind == MediaKind::Av
            || query.is_empty()
            || query.chars().count() > 160
            || !query.chars().any(char::is_alphanumeric)
        {
            return Err("请输入电影或剧集名称（最多 160 个字符）。".into());
        }
        Ok(media::search(
            &self.client,
            &self.entities,
            (self.tmdb.provider().as_deref(), &self.bangumi.provider()),
            kind,
            query,
            &self.languages.targets(None)[0],
        )
        .await)
    }

    pub async fn collect_media(
        &self,
        kind: MediaKind,
        provider: MediaProvider,
        id: &str,
    ) -> Result<LookupResponse, String> {
        let _permit = self.permit()?;
        Ok(self
            .save_lookup(self.gather_media(kind, provider, id).await?)
            .await)
    }

    pub(crate) async fn gather_media(
        &self,
        kind: MediaKind,
        provider: MediaProvider,
        id: &str,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let sources = self.media_sources(kind, provider, id, &targets[0]).await?;
        let key = match provider {
            MediaProvider::Wikidata => format!("Wikidata-{id}"),
            MediaProvider::Tvmaze => format!("TVmaze-{id}"),
            MediaProvider::Tmdb => format!("TMDB-{}-{id}", kind.code()),
            MediaProvider::Bangumi => format!(
                "Bangumi-{}",
                media::bangumi::id(id).ok_or("Bangumi ID 无效。")?
            ),
        };
        let query = sources
            .iter()
            .find_map(|source| source.fields.get("title")?.as_str())
            .unwrap_or(id)
            .to_owned();
        let lookup = make_lookup(kind, &query, Some(&key), sources)?;
        Ok(self.localize_lookup(lookup, targets).await)
    }

    async fn media_sources(
        &self,
        kind: MediaKind,
        provider: MediaProvider,
        id: &str,
        language: &str,
    ) -> Result<Vec<Source>, String> {
        media::collect(
            &self.client,
            &self.entities,
            (self.tmdb.provider().as_deref(), &self.bangumi.provider()),
            kind,
            provider,
            id,
            language,
        )
        .await
    }
}

fn make_lookup(
    media_type: MediaKind,
    query: &str,
    work_id: Option<&str>,
    sources: Vec<Source>,
) -> Result<LookupResponse, String> {
    let mut fields = Fields::new();
    let mut field_sources = BTreeMap::new();
    for source in &sources {
        for (&key, value) in &source.fields {
            fields.entry(key).or_insert_with(|| value.clone());
            field_sources.entry(key).or_insert(source.provider);
        }
    }

    let record = serde_json::to_value(Lookup {
        schema_version: 2,
        media_type,
        query,
        work_id,
        fields,
        field_sources,
        sources,
    })
    .map_err(|error| error.to_string())?;
    Ok(LookupResponse {
        record,
        saved: None,
        storage_error: None,
    })
}
