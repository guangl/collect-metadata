use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

use super::{CandidateChoice, folder};
use crate::{
    Collector, MediaKind, MediaProvider,
    output::write_automatic,
    providers::media::emby::{EPISODE_QUERY_PREFIX, is_item_id},
};

pub struct AutoScrape {
    spool: PathBuf,
    url: Option<String>,
    roots: Vec<PathBuf>,
    private_roots: Vec<PathBuf>,
    central_metadata: bool,
}

struct Event {
    path: PathBuf,
    id: String,
    item_type: &'static str,
}

impl Event {
    fn parse(path: PathBuf) -> Option<Self> {
        if path.extension()?.to_str()? != "event" {
            return None;
        }
        let (kind, id) = path.file_stem()?.to_str()?.split_once('-')?;
        let item_type = match kind {
            "Movie" => "Movie",
            "Series" => "Series",
            "Season" => "Season",
            "Episode" => "Episode",
            "Video" => "Video",
            _ => return None,
        };
        if !is_item_id(id) {
            return None;
        }
        let id = id.to_owned();
        Some(Self {
            path,
            id,
            item_type,
        })
    }
}

impl AutoScrape {
    pub fn from_environment() -> Result<Option<Self>, String> {
        let Some(spool) = env::var_os("EMBY_AUTO_SCRAPE_SPOOL").filter(|path| !path.is_empty())
        else {
            return Ok(None);
        };
        let spool = PathBuf::from(spool);
        if !spool.is_absolute() || !spool.is_dir() {
            return Err("EMBY_AUTO_SCRAPE_SPOOL 必须是已挂载的绝对目录。".into());
        }
        let roots = media_roots("EMBY_AUTO_SCRAPE_ROOTS")?;
        let private_roots = media_roots("EMBY_AUTO_SCRAPE_PRIVATE_ROOTS")?;
        if roots.is_empty() && private_roots.is_empty() {
            return Err("请为自动刮削指定 Emby 媒体库路径。".into());
        }
        let url = env::var("EMBY_AUTO_SCRAPE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let central_metadata = env::var("EMBY_AUTO_SCRAPE_CENTRAL_METADATA")
            .is_ok_and(|value| value.eq_ignore_ascii_case("true"));
        Ok(Some(Self {
            spool,
            url,
            roots,
            private_roots,
            central_metadata,
        }))
    }

    pub async fn run(self, collector: Arc<Collector>) {
        let mut retries: HashMap<PathBuf, (u32, Instant)> = HashMap::new();
        eprintln!(
            "Collect Metadata automatic scrape watching {}",
            self.spool.display()
        );
        loop {
            let events = match self.events().await {
                Ok(events) => events,
                Err(error) => {
                    eprintln!("Cannot read automatic scrape queue: {error}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            let now = Instant::now();
            let ready = events
                .into_iter()
                .filter(|event| {
                    retries
                        .get(&event.path)
                        .is_none_or(|(_, retry)| now >= *retry)
                })
                .take(32)
                .collect::<Vec<_>>();
            let mut results = stream::iter(ready)
                .map(|event| {
                    let service = &self;
                    let collector = &collector;
                    async move {
                        let result = service.process(collector, &event).await;
                        (event, result)
                    }
                })
                .buffer_unordered(4);
            while let Some((event, result)) = results.next().await {
                let error = match result {
                    Ok(done) => match tokio::fs::remove_file(&event.path).await {
                        Ok(()) => {
                            retries.remove(&event.path);
                            if let Some(count) = done {
                                eprintln!(
                                    "Automatic scrape {} {} completed with {count} new sidecar files.",
                                    event.item_type, event.id
                                );
                            }
                            continue;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => format!("无法移除已确认的入库事件：{error}"),
                    },
                    Err(error) => error,
                };
                let attempt = retries.get(&event.path).map_or(1, |(count, _)| count + 1);
                let delay = Duration::from_secs((5 * 2_u64.pow(attempt.min(9) - 1)).min(1800));
                retries.insert(event.path, (attempt, Instant::now() + delay));
                eprintln!(
                    "Automatic scrape {} {}: {error}; retry in {}s.",
                    event.item_type,
                    event.id,
                    delay.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
    }

    async fn events(&self) -> std::io::Result<Vec<Event>> {
        let mut events = tokio::fs::read_dir(&self.spool).await?;
        let mut result = Vec::new();
        while let Some(entry) = events.next_entry().await? {
            if entry.file_type().await?.is_file()
                && let Some(event) = Event::parse(entry.path())
            {
                result.push(event);
            }
        }
        result.sort_unstable_by(|left, right| {
            left.id
                .len()
                .cmp(&right.id.len())
                .then(left.id.cmp(&right.id))
        });
        Ok(result)
    }

    async fn process(&self, collector: &Collector, event: &Event) -> Result<Option<usize>, String> {
        let _permit = collector.batch_permit().await;
        let emby = collector
            .emby_settings()
            .ok_or("Emby 设置不可用。")?
            .automatic_connection(self.url.as_deref())?;
        let (kind, prefix, item_type) = match event.item_type {
            "Episode" => (MediaKind::Series, EPISODE_QUERY_PREFIX, "Episode"),
            "Series" => (MediaKind::Series, "emby:", "Series"),
            "Season" => (MediaKind::Series, "emby:", "Season"),
            _ => (MediaKind::Movie, "emby:", "Movie,Video"),
        };
        let query = format!("{prefix}{}", event.id);
        // The scan may still be committing the item when its event is queued.
        let home = emby
            .automatic_item(&collector.client, &event.id, item_type)
            .await?;
        let item = &home.data["item"];
        if item["LocationType"] == "Virtual" {
            return Ok(None);
        }
        let path = item["Path"].as_str().filter(|path| !path.is_empty()).map(Path::new)
            .ok_or("等待 Emby 完成入库并提供媒体路径。")?;
        let private = self.private_roots.iter().any(|root| path.starts_with(root));
        if !private && !self.roots.iter().any(|root| path.starts_with(root)) {
            return Ok(None);
        }
        if item["LockData"] == true
            || item["LockData"]["Enabled"] == true
            || item["IsLocked"] == true
        {
            return Ok(None);
        }
        let settings = collector.output_settings().ok_or("自动写回设置不可用。")?;
        let paths = settings.media_paths();
        if event.item_type == "Video" && !private {
            return Ok(None);
        }
        let mut result = if private {
            if kind != MediaKind::Movie {
                return Ok(None);
            }
            let code = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(folder::av_code)
                .or_else(|| {
                    path.parent()?
                        .file_name()?
                        .to_str()
                        .and_then(folder::av_code)
                })
                .ok_or("无法从私人影片文件名识别番号。")?;
            let mut result = collector.gather_av(&code).await?;
            if !result.has_fields() {
                return Err("未获得私人影片资料。".into());
            }
            let sources = result.record["sources"]
                .as_array_mut()
                .ok_or("采集记录缺少来源。")?;
            sources.push(json!(home));
            result
        } else if matches!(event.item_type, "Season" | "Episode") {
            let series_id = item["SeriesId"].as_str().ok_or("Emby 条目缺少所属剧集。")?;
            let series = emby
                .item(
                    &collector.client,
                    series_id,
                    "Series",
                    "Path,ProviderIds,OriginalTitle,ProductionYear",
                )
                .await?;
            let candidate = match_work(collector, MediaKind::Series, &series).await?;
            collector
                .gather_automatic_child(
                    &emby,
                    home,
                    &query,
                    &candidate,
                    event.item_type == "Season",
                )
                .await?
        } else {
            let candidate = match_work(collector, kind, item).await?;
            collector
                .gather_automatic_work(kind, event.id.clone(), &query, home, &candidate)
                .await?
        };
        if self.central_metadata && !private {
            collector.require_store().await?;
            let data = crate::output::emby_source(&result.record).ok_or("缺少 Emby 文件关联。")?;
            emby.enrich_automatic_item(&collector.client, &data["item"], &result.record).await?;
            let fresh = emby.automatic_item(&collector.client, &event.id, item_type).await?;
            let source = result.record["sources"].as_array_mut().into_iter().flatten()
                .find(|source| source["provider"] == "Emby")
                .ok_or("缺少 Emby 文件关联。")?;
            source["data"]["item"] = fresh.data["item"].clone();
            let server = emby.automatic_server_id(&collector.client).await?;
            collector.upsert_emby_item(&server, &event.id, &fresh.data["item"], None).await?;
        }
        let written = write_automatic(&collector.client, &emby, &paths, &result.record).await;
        if !self.central_metadata || private {
            // The legacy route loads local sidecars into Emby.
            emby.refresh_sidecars(&collector.client, &event.id).await?;
        }
        let written = written?;
        if written > 0 {
            let saved = collector.save_lookup(result).await;
            if let Some(error) = saved.storage_error {
                eprintln!("Automatic scrape {} snapshot: {error}", event.id);
            }
        }
        Ok(Some(written))
    }
}

fn media_roots(variable: &str) -> Result<Vec<PathBuf>, String> {
    env::var(variable)
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let path = PathBuf::from(part);
            if path.is_absolute()
                && path.parent().is_some()
                && path
                    .components()
                    .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
            {
                Ok(path)
            } else {
                Err(format!("{variable} 必须使用明确的绝对媒体库路径。"))
            }
        })
        .collect()
}

async fn match_work(
    collector: &Collector,
    kind: MediaKind,
    item: &Value,
) -> Result<CandidateChoice, String> {
    let ids = &item["ProviderIds"];
    if let Some(id) = ids["Tmdb"].as_str().filter(|id| id.parse::<u32>().is_ok_and(|n| n > 0)) {
        return Ok(CandidateChoice { provider: MediaProvider::Tmdb, id: id.into() });
    }
    if let Some(path) = item["Path"].as_str()
        && let Some(hint) = Path::new(path)
            .components()
            .rev()
            .take(2)
            .find_map(|part| folder::catalog_query(part.as_os_str().to_str()?))
        && let Some((provider, id)) = hint.split_once(':')
    {
        return Ok(CandidateChoice {
            provider: if provider == "tmdb" {
                MediaProvider::Tmdb
            } else {
                MediaProvider::Bangumi
            },
            id: id.into(),
        });
    }
    if let Some(id) = ids["Bangumi"].as_str().filter(|id| id.parse::<u32>().is_ok_and(|n| n > 0)) {
        return Ok(CandidateChoice { provider: MediaProvider::Bangumi, id: id.into() });
    }
    let title = item["OriginalTitle"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .or_else(|| item["Name"].as_str())
        .ok_or("Emby 条目缺少可匹配的片名。")?;
    let year = item["ProductionYear"]
        .as_u64()
        .and_then(|year| year.try_into().ok());
    if let Some(year) = year {
        let candidates = collector.automatic_tmdb_search(kind, title).await;
        let normalized = super::worker::normalize(title);
        let mut exact = candidates.iter().filter(|candidate| {
            candidate.year == Some(year)
                && std::iter::once(&candidate.title)
                    .chain(&candidate.match_titles)
                    .any(|title| super::worker::normalize(title) == normalized)
        });
        if let Some(candidate) = exact.next().filter(|_| exact.next().is_none()) {
            return Ok(CandidateChoice {
                provider: MediaProvider::Tmdb,
                id: candidate.id.clone(),
            });
        }
    }
    collector
        .search_match(kind, title, year, collector.ai_configured())
        .await
        .map(|(candidate, _)| candidate)
        .map_err(|outcome| {
            outcome
                .message
                .unwrap_or_else(|| "公开来源存在多个候选，请为文件或剧集目录添加来源 ID。".into())
        })
}
