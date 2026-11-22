use std::{collections::BTreeMap, env, sync::Arc, time::Duration};

use reqwest::Client;
use sqlx::postgres::PgConnectOptions;
use tokio::sync::{OnceCell, Semaphore};

mod identifier;
mod localization;
mod model;
mod tmdb_settings;

#[cfg(test)]
mod tests;

use crate::{
    EmbySettings, HistoryItem, MediaKind, MediaProvider, OutputSettings, SearchResult,
    providers::{av, media},
    storage::Store,
};
pub(crate) use identifier::Identifier;
pub use localization::{LanguageSettings, LanguageSettingsView};
use model::Lookup;
pub use model::LookupResponse;
pub(crate) use model::{Fields, Source};
pub use tmdb_settings::{TmdbSettings, TmdbStatus};

pub struct Collector {
    pub(crate) client: Client,
    database: Option<PgConnectOptions>,
    store: OnceCell<Store>,
    searches: Semaphore,
    entities: media::EntityCache,
    tmdb: Arc<TmdbSettings>,
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
            entities: media::EntityCache::default(),
            tmdb: Arc::new(TmdbSettings::from_environment()),
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

    pub(crate) async fn gather_emby(
        &self,
        emby: &media::emby::Emby,
        kind: MediaKind,
        query: &str,
        (id, mut home): (String, Source),
        candidate: Option<&crate::CandidateChoice>,
        prefer_emby: bool,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let is_episode = query.starts_with(media::emby::EPISODE_QUERY_PREFIX);
        let sources = if let Some(candidate) = candidate {
            let mut external = if is_episode {
                let tmdb = self.tmdb.provider().ok_or("请先配置 TMDB API 凭据。")?;
                let series = candidate
                    .id
                    .parse::<u32>()
                    .map_err(|_| "剧集 TMDB ID 无效。")?;
                let episode = tmdb
                    .emby_episode_with_series(
                        &self.client,
                        emby,
                        &home.data["item"],
                        &targets[0],
                        Some(series),
                    )
                    .await
                    .ok_or("此分集缺少单集季集号。")?;
                vec![episode]
            } else {
                self.media_sources(kind, candidate.provider, &candidate.id, &targets[0])
                    .await?
            };
            if external.iter().all(|source| source.fields.is_empty()) {
                return Err(external
                    .into_iter()
                    .find_map(|source| source.note)
                    .unwrap_or_else(|| "来源未返回可用资料。".into()));
            }
            // Keep Emby episode IDs and file associations while applying known titles and stills.
            if kind == MediaKind::Series && !is_episode {
                merge_emby_episodes(&mut home, &mut external, prefer_emby);
            }
            if prefer_emby {
                external.insert(0, home);
            } else {
                external.push(home);
            }
            external
        } else {
            let mut sources = vec![home];
            if !is_episode
                && kind != MediaKind::Av
                && let Some(bangumi_id) = sources[0]
                    .fields
                    .get("bangumi_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(media::bangumi::id)
            {
                let enrichment =
                    media::bangumi::details(&self.client, &self.entities, kind, bangumi_id).await;
                if !enrichment.fields.is_empty() {
                    sources.push(enrichment);
                }
            }
            if is_episode && let Some(tmdb) = self.tmdb.provider() {
                let episode = &sources[0].data["item"];
                if let Some(enrichment) = tmdb
                    .emby_episode(&self.client, emby, episode, &targets[0])
                    .await
                {
                    sources.push(enrichment);
                }
            }
            sources
        };
        let mut lookup = make_lookup(kind, query, Some(&format!("Emby-{id}")), sources)?;
        if is_episode {
            lookup.record["scope"] = serde_json::json!("episode");
        }
        if prefer_emby {
            lookup.record["fill_missing"] = serde_json::json!(true);
        }
        Ok(self.localize_lookup(lookup, targets).await)
    }

    pub async fn upsert_emby_item(
        &self,
        server: &str,
        id: &str,
        document: &serde_json::Value,
        preferred_language: Option<&str>,
    ) -> Result<(), String> {
        let store = self.require_store().await?;
        let previous = store.emby_item(server, id).await?.unwrap_or_default();
        let document = localization::records::sync_document(
            &self.languages,
            &self.client,
            document,
            &previous,
            preferred_language,
        )
        .await;
        store.upsert_emby_item(server, id, &document).await
    }

    async fn store(&self) -> Result<Option<&Store>, String> {
        let Some(database) = &self.database else {
            return Ok(None);
        };
        self.store
            .get_or_try_init(|| Store::connect(database))
            .await
            .map(Some)
    }

    pub(crate) async fn require_store(&self) -> Result<&Store, String> {
        self.store()
            .await?
            .ok_or("批量后台采集需要连接 PostgreSQL。".into())
    }

    pub async fn init_storage(&self) -> Result<(), String> {
        self.store().await.map(|_| ())
    }

    pub async fn check_storage(&self) -> Result<(), String> {
        self.require_store().await?.check_connection().await
    }

    pub fn storage_configured(&self) -> bool {
        self.database.is_some()
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
            self.tmdb.provider().as_deref(),
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
            self.tmdb.provider().as_deref(),
            kind,
            provider,
            id,
            language,
        )
        .await
    }

    async fn save_lookup(&self, mut response: LookupResponse) -> LookupResponse {
        if response.has_fields() {
            match self.store().await {
                Ok(Some(store)) => match store.save(response.code(), &response.record).await {
                    Ok(item) => response.saved = Some(item),
                    Err(error) => response.storage_error = Some(error),
                },
                Ok(None) => {}
                Err(error) => response.storage_error = Some(error),
            }
        }
        response
    }

    pub async fn history(
        &self,
        before: Option<i64>,
        kind: Option<MediaKind>,
        language: Option<&str>,
    ) -> Result<Vec<HistoryItem>, String> {
        if let Some(code) = language
            && code != "und"
            && (!matches!(code.len(), 2 | 3) || !code.bytes().all(|byte| byte.is_ascii_lowercase()))
        {
            return Err("语言代码无效。".into());
        }
        match self.store().await? {
            Some(store) => store.history(before, kind, language).await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn history_languages(&self) -> Result<Vec<String>, String> {
        match self.store().await? {
            Some(store) => store.history_languages().await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn load(&self, id: i64) -> Result<LookupResponse, String> {
        let store = self.store().await?.ok_or("未连接 PostgreSQL。")?;
        let (saved, record) = store.load(id).await?;
        Ok(LookupResponse {
            record,
            saved: Some(saved),
            storage_error: None,
        })
    }
}

fn merge_emby_episodes(home: &mut Source, external: &mut [Source], prefer_emby: bool) {
    let mut by_number = BTreeMap::new();
    for source in external {
        if let Some(episodes) = source.fields.remove("episodes") {
            for episode in episodes.as_array().into_iter().flatten() {
                if let Some(number) = episode["season"].as_u64().zip(episode["number"].as_u64()) {
                    by_number.entry(number).or_insert_with(|| episode.clone());
                }
            }
        }
        source.fields.remove("season_count");
    }
    if let Some(episodes) = home
        .fields
        .get_mut("episodes")
        .and_then(serde_json::Value::as_array_mut)
    {
        for episode in episodes {
            let number = episode["season"].as_u64().zip(episode["number"].as_u64());
            if let Some(source) = number.and_then(|number| by_number.get(&number)) {
                for key in [
                    "title",
                    "description",
                    "date",
                    "runtime_minutes",
                    "still_url",
                    "url",
                ] {
                    if prefer_emby
                        && !episode[key].is_null()
                        && episode[key]
                            .as_str()
                            .is_none_or(|text| !text.trim().is_empty())
                    {
                        continue;
                    }
                    if let Some(value) = source.get(key).filter(|value| {
                        !value.is_null()
                            && value.as_str().is_none_or(|text| !text.trim().is_empty())
                    }) {
                        episode[key] = value.clone();
                    }
                }
            }
        }
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
