pub(crate) mod bangumi;
pub(crate) mod emby;
#[cfg(feature = "web")]
pub(crate) mod episode_title;
mod tmdb;
mod tvmaze;
mod wikidata;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Failure, request};
use crate::collect::Source;
use bangumi::Bangumi;
pub(crate) use tmdb::Tmdb;

#[derive(Default)]
pub(crate) struct EntityCache(Mutex<VecDeque<CachedEntity>>);

struct CachedEntity {
    id: String,
    value: Arc<Value>,
    fetched_at: Instant,
}

impl EntityCache {
    fn get(&self, id: &str) -> Option<Arc<Value>> {
        let cache = self.0.lock().ok()?;
        cache
            .iter()
            .find(|entry| entry.id == id && entry.fetched_at.elapsed() < Duration::from_secs(180))
            .map(|entry| Arc::clone(&entry.value))
    }

    fn insert(&self, id: &str, value: Arc<Value>) {
        if let Ok(mut cache) = self.0.lock() {
            cache.retain(|entry| {
                entry.id != id && entry.fetched_at.elapsed() < Duration::from_secs(180)
            });
            cache.push_back(CachedEntity {
                id: id.to_owned(),
                value,
                fetched_at: Instant::now(),
            });
            if cache.len() > 24 {
                cache.pop_front();
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Av,
    Movie,
    Series,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaProvider {
    Wikidata,
    Tvmaze,
    Tmdb,
    Bangumi,
}

impl MediaKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Av => "av",
            Self::Movie => "movie",
            Self::Series => "series",
        }
    }
}

impl MediaProvider {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Wikidata => "wikidata",
            Self::Tvmaze => "tvmaze",
            Self::Tmdb => "tmdb",
            Self::Bangumi => "bangumi",
        }
    }
}

#[derive(Serialize)]
pub struct SearchResult {
    pub candidates: Vec<Candidate>,
    pub notes: Vec<String>,
}

#[derive(Serialize)]
pub struct Candidate {
    pub provider: MediaProvider,
    pub id: String,
    pub title: String,
    pub match_titles: Vec<String>,
    pub year: Option<u16>,
    pub original_languages: Vec<String>,
    pub cover_url: Option<String>,
    #[serde(skip)]
    pub(crate) tvmaze_id: Option<u64>,
    #[serde(skip)]
    pub(crate) tmdb_id: Option<u32>,
    #[serde(skip)]
    pub(crate) bangumi_id: Option<u32>,
}

pub(crate) fn tmdb_id_query(query: &str) -> Option<&str> {
    let (prefix, id) = query.split_once([':', '：'])?;
    prefix
        .trim()
        .eq_ignore_ascii_case("tmdb")
        .then_some(id.trim())
}

pub(crate) async fn search(
    client: &Client,
    cache: &EntityCache,
    (tmdb, bangumi): (Option<&Tmdb>, &Bangumi),
    kind: MediaKind,
    query: &str,
    language: &str,
) -> SearchResult {
    if let Some(raw_id) = tmdb_id_query(query) {
        let result = match (tmdb, tmdb::id(raw_id)) {
            (Some(tmdb), Some(id)) => search_within(tmdb.search_id(client, kind, id, language))
                .await
                .map_err(|error| format!("TMDB：{}", error.description())),
            (_, None) => Err("TMDB ID 无效。".into()),
            (None, _) => Err("请先在「设置」中配置 TMDB API 凭据。".into()),
        };
        return match result {
            Ok(candidates) => SearchResult {
                candidates,
                notes: Vec::new(),
            },
            Err(note) => SearchResult {
                candidates: Vec::new(),
                notes: vec![note],
            },
        };
    }
    if let Some(raw_id) = bangumi::id_query(query) {
        return match bangumi::id(raw_id) {
            Some(id) => match search_within(bangumi.search_id(client, kind, id)).await {
                Ok(candidates) => SearchResult {
                    candidates,
                    notes: Vec::new(),
                },
                Err(error) => SearchResult {
                    candidates: Vec::new(),
                    notes: vec![format!("Bangumi：{}", error.description())],
                },
            },
            None => SearchResult {
                candidates: Vec::new(),
                notes: vec!["Bangumi ID 无效。".into()],
            },
        };
    }
    let (wiki, maze, movie_db, bgm) = tokio::join!(
        search_within(wikidata::search(client, cache, kind, query)),
        search_within(async {
            if kind == MediaKind::Series && !wikidata::valid_id(query) {
                tvmaze::search(client, query).await
            } else {
                Ok(Vec::new())
            }
        }),
        search_within(async {
            match tmdb.filter(|_| !wikidata::valid_id(query)) {
                Some(tmdb) => tmdb.search(client, kind, query, language).await,
                None => Ok(Vec::new()),
            }
        }),
        search_within(async {
            if kind != MediaKind::Av && !wikidata::valid_id(query) {
                bangumi.search(client, kind, query).await
            } else {
                Ok(Vec::new())
            }
        }),
    );
    let SearchResult {
        mut candidates,
        mut notes,
    } = match wiki {
        Ok(result) => result,
        Err(error) => SearchResult {
            candidates: Vec::new(),
            notes: vec![format!("Wikidata：{}", error.description())],
        },
    };
    let maze = maze.unwrap_or_else(|error| {
        notes.push(format!("TVmaze：{}", error.description()));
        Vec::new()
    });
    for item in maze {
        if let Some(candidate) = candidates
            .iter_mut()
            .find(|candidate| candidate.tvmaze_id == item.tvmaze_id)
        {
            candidate.match_titles.extend(item.match_titles);
        } else {
            candidates.push(item);
        }
    }
    let movie_db = movie_db.unwrap_or_else(|error| {
        notes.push(format!("TMDB：{}", error.description()));
        Vec::new()
    });
    for item in movie_db {
        if let Some(candidate) = candidates
            .iter_mut()
            .find(|candidate| candidate.tmdb_id == item.tmdb_id)
        {
            candidate.match_titles.extend(item.match_titles);
            if candidate.original_languages.is_empty() {
                candidate.original_languages = item.original_languages;
            }
            candidate.cover_url = candidate.cover_url.take().or(item.cover_url);
            candidate.year = candidate.year.or(item.year);
        } else {
            candidates.push(item);
        }
    }
    let bgm = bgm.unwrap_or_else(|error| {
        notes.push(format!("Bangumi：{}", error.description()));
        Vec::new()
    });
    for item in bgm {
        if let Some(candidate) = candidates
            .iter_mut()
            .find(|candidate| candidate.bangumi_id == item.bangumi_id)
        {
            candidate.match_titles.extend(item.match_titles);
            if candidate.original_languages.is_empty() {
                candidate.original_languages = item.original_languages;
            }
            candidate.cover_url = candidate.cover_url.take().or(item.cover_url);
            candidate.year = candidate.year.or(item.year);
        } else {
            candidates.push(item);
        }
    }
    SearchResult { candidates, notes }
}

async fn search_within<T>(query: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    tokio::time::timeout(Duration::from_secs(9), query)
        .await
        .unwrap_or_else(|_| Err(Failure::Unavailable("搜索超时，可稍后重试。".into())))
}

pub(crate) async fn collect(
    client: &Client,
    cache: &EntityCache,
    (tmdb, bangumi): (Option<&Tmdb>, &Bangumi),
    kind: MediaKind,
    provider: MediaProvider,
    id: &str,
    language: &str,
) -> Result<Vec<Source>, String> {
    match provider {
        MediaProvider::Wikidata if wikidata::valid_id(id) => {
            let entity = wikidata::entity(client, cache, id)
                .await
                .map_err(|error| error.description().to_owned())?;
            if !wikidata::matches_kind(&entity, kind) {
                return Err("条目类型与所选类别不一致。".into());
            }
            let bangumi_id = wikidata::bangumi_id(&entity);
            let (mut linked, bgm) = tokio::join!(
                wikidata_and_links(client, tmdb, kind, id, entity, language),
                async {
                    match bangumi_id {
                        Some(id) => Some(bangumi.details(client, kind, id).await),
                        None => None,
                    }
                },
            );
            if let Some(bgm) = bgm {
                linked.insert(0, bgm);
            }
            Ok(linked)
        }
        MediaProvider::Bangumi if kind != MediaKind::Av => {
            let id = bangumi::id(id).ok_or("Bangumi ID 无效。")?;
            let (bgm, wiki) = tokio::join!(
                bangumi.details(client, kind, id),
                search_within(wikidata::by_bangumi_id(client, cache, kind, id)),
            );
            let include_linked = !bgm.fields.is_empty();
            let mut sources = vec![bgm];
            if include_linked {
                match wiki {
                    Ok(Some((id, entity))) => sources.extend(
                        wikidata_and_links(client, tmdb, kind, &id, entity, language).await,
                    ),
                    Ok(None) => {}
                    Err(error) => sources.push(Source::failure(
                        "Wikidata",
                        &wikidata::bangumi_search_url(id),
                        error,
                    )),
                }
            }
            Ok(sources)
        }
        MediaProvider::Tvmaze if kind == MediaKind::Series => {
            let id = id
                .parse::<u64>()
                .ok()
                .filter(|&id| id > 0)
                .ok_or("TVmaze ID 无效。")?;
            Ok(vec![tvmaze::details(client, id).await])
        }
        MediaProvider::Tmdb if kind != MediaKind::Av => {
            let id = tmdb::id(id).ok_or("TMDB ID 无效。")?;
            let tmdb = tmdb.ok_or("请先在「设置」中配置 TMDB API 凭据。")?;
            Ok(vec![tmdb.details(client, kind, id, language).await])
        }
        _ => Err("该来源 ID 或作品类型无效。".into()),
    }
}

async fn wikidata_and_links(
    client: &Client,
    tmdb: Option<&Tmdb>,
    kind: MediaKind,
    id: &str,
    entity: Arc<Value>,
    language: &str,
) -> Vec<Source> {
    let maze_id = (kind == MediaKind::Series)
        .then(|| wikidata::tvmaze_id(&entity))
        .flatten();
    let tmdb_id = wikidata::tmdb_id(&entity, kind);
    let (wiki, maze, movie_db) = tokio::join!(
        wikidata::details(client, id, entity),
        async {
            match maze_id {
                Some(id) => Some(tvmaze::details(client, id).await),
                None => None,
            }
        },
        async {
            match (tmdb, tmdb_id) {
                (Some(tmdb), Some(id)) => Some(tmdb.details(client, kind, id, language).await),
                _ => None,
            }
        },
    );
    std::iter::once(wiki).chain(maze).chain(movie_db).collect()
}

async fn get_json(client: &Client, url: &Url) -> Result<Value, Failure> {
    let body = request(client, url, "application/json").await?;
    serde_json::from_str(&body).map_err(|_| Failure::format_changed())
}

fn year(date: &str) -> Option<u16> {
    date.get(..4)?.parse().ok()
}
