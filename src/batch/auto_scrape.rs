mod durable;

use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use jiff::{
    Timestamp,
    civil::{Date, DateTime},
    tz::TimeZone,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{CandidateChoice, folder};
use crate::{
    Collector, MediaKind, MediaProvider,
    output::write_automatic,
    providers::media::{
        bangumi::{MappedAirdate, mapped_episode_candidate},
        emby::{
            EPISODE_QUERY_PREFIX, Emby, has_provider_id, is_item_id, missing_people, record_people,
        },
        episode_title::{needs_title, usable},
    },
};

const RETRY_WINDOW: Duration = Duration::from_secs(7 * 24 * 3600);
const MISSING_ITEM_GRACE: Duration = Duration::from_secs(15 * 60);

#[derive(Deserialize, Default)]
#[serde(transparent)]
struct SeasonMap(HashMap<String, HashMap<u64, Vec<u32>>>);

impl SeasonMap {
    fn parse(json: &str) -> Result<Self, String> {
        let map: Self =
            serde_json::from_str(json).map_err(|error| format!("季集映射格式无效：{error}"))?;
        for (series, seasons) in &map.0 {
            if !is_item_id(series)
                || seasons.iter().any(|(&season, subjects)| {
                    season == 0
                        || subjects.is_empty()
                        || subjects
                            .iter()
                            .enumerate()
                            .any(|(index, id)| *id == 0 || subjects[..index].contains(id))
                })
            {
                return Err(
                    "季集映射需要有效的 Emby 剧集 ID、正季号和不重复的 Bangumi 条目 ID。".into(),
                );
            }
        }
        Ok(map)
    }

    fn subjects(&self, item: &Value) -> &[u32] {
        let number = if item["Type"] == "Episode" {
            "ParentIndexNumber"
        } else {
            "IndexNumber"
        };
        item["SeriesId"]
            .as_str()
            .zip(item[number].as_u64())
            .and_then(|(series, season)| self.0.get(series)?.get(&season))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

enum Outcome {
    Skipped,
    Written(usize),
    Enriched,
    NoVerifiedSource,
    AwaitingTitle,
    AwaitingAirdateReview,
    MissingItem,
}

pub struct AutoScrape {
    spool: PathBuf,
    url: Option<String>,
    roots: Vec<PathBuf>,
    private_roots: Vec<PathBuf>,
    central_metadata: bool,
    season_map: SeasonMap,
    database_season_map: bool,
    emby_timezone: TimeZone,
}

struct Event {
    path: PathBuf,
    id: String,
    item_type: &'static str,
    backfill: bool,
}

impl Event {
    fn parse(path: PathBuf) -> Option<Self> {
        let backfill = match path.extension()?.to_str()? {
            "event" => false,
            "backfill" => true,
            _ => return None,
        };
        let (kind, id) = path.file_stem()?.to_str()?.split_once('-')?;
        let item_type = match kind {
            "Movie" => "Movie",
            "Series" => "Series",
            "Season" => "Season",
            "Episode" => "Episode",
            "Video" => "Video",
            _ => return None,
        };
        if !is_item_id(id) || (backfill && !matches!(item_type, "Series" | "Season" | "Episode")) {
            return None;
        }
        let id = id.to_owned();
        Some(Self {
            path,
            id,
            item_type,
            backfill,
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
        let timezone = env::var("EMBY_AUTO_SCRAPE_TIMEZONE").unwrap_or_else(|_| "UTC".into());
        let emby_timezone = TimeZone::get(timezone.trim())
            .map_err(|_| "EMBY_AUTO_SCRAPE_TIMEZONE 必须是有效的 IANA 时区。")?;
        let season_map =
            match env::var_os("EMBY_AUTO_SCRAPE_SEASON_MAP").filter(|path| !path.is_empty()) {
                Some(path) => SeasonMap::parse(
                    &std::fs::read_to_string(path)
                        .map_err(|error| format!("无法读取季集映射：{error}"))?,
                )?,
                None => SeasonMap::default(),
            };
        Ok(Some(Self {
            spool,
            url,
            roots,
            private_roots,
            central_metadata,
            season_map,
            database_season_map: env::var("EMBY_AUTO_SCRAPE_DB_SEASON_MAP")
                .is_ok_and(|value| value.eq_ignore_ascii_case("true")),
            emby_timezone,
        }))
    }

    pub async fn run(self, collector: Arc<Collector>) {
        self.run_durable(collector).await;
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
            left.backfill.cmp(&right.backfill)
                .then(left.id.len().cmp(&right.id.len()))
                .then(left.id.cmp(&right.id))
        });
        Ok(result)
    }

    async fn process(&self, collector: &Collector, event: &Event) -> Result<Outcome, String> {
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
        let Some(home) = emby
            .automatic_item(&collector.client, &event.id, item_type)
            .await?
        else {
            return Ok(Outcome::MissingItem);
        };
        let item = &home.data["item"];
        if item["LocationType"] == "Virtual" {
            return Ok(Outcome::Skipped);
        }
        let path = item["Path"].as_str().filter(|path| !path.is_empty()).map(Path::new)
            .ok_or("等待 Emby 完成入库并提供媒体路径。")?;
        let private = self.private_roots.iter().any(|root| path.starts_with(root));
        if !private && !self.roots.iter().any(|root| path.starts_with(root)) {
            return Ok(Outcome::Skipped);
        }
        if item["LockData"] == true
            || item["LockData"]["Enabled"] == true
            || item["IsLocked"] == true
        {
            return Ok(Outcome::Skipped);
        }
        if event.item_type == "Video" && !private {
            return Ok(Outcome::Skipped);
        }
        let central = self.central_metadata && !private;
        if event.backfill && !central {
            return Ok(Outcome::Skipped);
        }
        let episode_complete = event.item_type == "Episode" && complete_episode(item);
        let season_field = if event.item_type == "Episode" {
            "ParentIndexNumber"
        } else {
            "IndexNumber"
        };
        let mut server_id = None;
        let mut database_subjects = None;
        if self.database_season_map
            && !private
            && (!episode_complete || central)
            && matches!(event.item_type, "Season" | "Episode")
            && let Some(series) = item["SeriesId"].as_str()
            && let Some(season) = item[season_field]
                .as_i64()
                .and_then(|no| i32::try_from(no).ok())
            && season > 0
        {
            let server = emby.automatic_server_id(&collector.client).await?;
            database_subjects = collector
                .require_store()
                .await?
                .verified_bangumi_season(&server, series, season)
                .await?;
            if database_subjects.is_some() {
                eprintln!(
                    "Automatic scrape {} {} using PostgreSQL season mapping.",
                    event.item_type, event.id
                );
            }
            server_id = Some(server);
        }
        let mut archived = Vec::new();
        let mut mapped_airdate = MappedAirdate::Legacy;
        if event.item_type == "Episode"
            && let (Some(server), Some(subjects)) = (&server_id, &database_subjects)
        {
            if episode_complete && has_provider_id(item, "Bangumi") {
                return sync_mapped_episode(collector, &emby, server, item, None, central).await;
            }
            let store = collector.require_store().await?;
            archived = match store.archived_bangumi_episodes(subjects).await {
                Ok(episodes) => episodes,
                Err(error) if episode_complete => return Err(error),
                Err(_) => Vec::new(),
            };
            let local = emby_airdate(&item["PremiereDate"], &self.emby_timezone)?;
            mapped_airdate = MappedAirdate::Local(local);
            let candidate = mapped_episode_candidate(&archived, item);
            let bangumi_id = candidate
                .and_then(|episode| episode["id"].as_u64())
                .and_then(|id| u32::try_from(id).ok())
                .filter(|&id| id > 0);
            let catalog_date = candidate
                .and_then(|episode| episode["airdate"].as_str())
                .and_then(|date| date.parse::<Date>().ok());
            if central
                && let Some((local, catalog_date)) = local.zip(catalog_date)
                && local != catalog_date
            {
                let Some(id) = bangumi_id else {
                    return Ok(Outcome::AwaitingAirdateReview);
                };
                if usable(&item["Name"], item).is_none()
                    || !store
                        .verified_episode_airdate(server, item, id, local, catalog_date)
                        .await?
                {
                    return Ok(Outcome::AwaitingAirdateReview);
                }
                eprintln!(
                    "Automatic scrape Episode {} confirmed by official airdate {catalog_date}.",
                    event.id
                );
                return sync_mapped_episode(collector, &emby, server, item, Some(id), central)
                    .await;
            }
            // Preserve the ID-only route when conversion resolves an apparent
            // conflict, even if Emby is missing other metadata.
            let timezone_resolved_conflict = central
                && local.zip(catalog_date).is_some_and(|(local, catalog)| {
                    local == catalog
                        && item["PremiereDate"]
                            .as_str()
                            .and_then(|date| date.trim().get(..10))
                            .and_then(|date| date.parse::<Date>().ok())
                            .is_some_and(|stored| stored != local)
                });
            if episode_complete || timezone_resolved_conflict {
                return sync_mapped_episode(collector, &emby, server, item, bangumi_id, central)
                    .await;
            }
        }
        let settings = collector.output_settings().ok_or("自动写回设置不可用。")?;
        let paths = settings.media_paths();
        let mapped_subjects = database_subjects
            .as_deref()
            .unwrap_or_else(|| self.season_map.subjects(item));
        let mut result = if private {
            if kind != MediaKind::Movie {
                return Ok(Outcome::Skipped);
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
        } else if episode_complete {
            collector.existing_automatic_episode(home, &query, &event.id)?
        } else if !mapped_subjects.is_empty() {
            collector
                .gather_mapped_child(
                    home,
                    &query,
                    &event.id,
                    mapped_subjects,
                    &archived,
                    mapped_airdate,
                )
                .await?
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
                    &series,
                )
                .await?
        } else {
            let candidate = match_work(collector, kind, item).await?;
            collector
                .gather_automatic_work(kind, event.id.clone(), &query, home, &candidate)
                .await?
        };
        if central {
            collector.require_store().await?;
            if event.backfill && !result.record["sources"].as_array().into_iter().flatten().any(|source| {
                source["provider"] != "Emby" && matches!(source["status"].as_str(), Some("ok" | "partial"))
            }) {
                return Ok(Outcome::NoVerifiedSource);
            }
            let data = crate::output::emby_source(&result.record).ok_or("缺少 Emby 文件关联。")?;
            let item = &data["item"];
            let mut people = Vec::new();
            if missing_people(item) {
                people = record_people(&result.record);
                if people.is_empty() && event.item_type == "Episode" {
                    people = collector.verified_episode_people(&emby, item).await;
                }
            }
            let _write = collector.automatic_write_lock(&event.id).await;
            emby.enrich_automatic_item(&collector.client, item, &result.record, &people)
                .await?;
            let fresh = emby
                .automatic_item(&collector.client, &event.id, item_type)
                .await?
                .ok_or("Emby 条目在写入期间已消失。")?;
            let source = result.record["sources"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .find(|source| source["provider"] == "Emby")
                .ok_or("缺少 Emby 文件关联。")?;
            source["data"]["item"] = fresh.data["item"].clone();
            if event.item_type == "Episode" {
                // Sidecars use the title actually saved by Emby, including a concurrent manual edit.
                for (key, field) in [("title", "Name"), ("sort_title", "SortName")] {
                    let actual = &fresh.data["item"][field];
                    if result.record["fields"][key] != *actual {
                        result.record["fields"][key] = actual.clone();
                        result.record["field_sources"][key] = json!("Emby");
                    }
                }
            }
            if event.item_type == "Season" {
                let actual = &fresh.data["item"]["Overview"];
                if result.record["fields"]["description"] != *actual {
                    result.record["fields"]["description"] = actual.clone();
                    result.record["field_sources"]["description"] = json!("Emby");
                }
            }
            let server = match server_id {
                Some(server) => server,
                None => emby.automatic_server_id(&collector.client).await?,
            };
            collector.upsert_emby_item(&server, &event.id, &fresh.data["item"], None).await?;
            if event.backfill {
                return Ok(Outcome::Enriched);
            }
        } else if event.item_type == "Episode" {
            let item = &crate::output::emby_source(&result.record).ok_or("缺少 Emby 文件关联。")?["item"];
            if !needs_title(item) {
                result.record["fields"]["title"] = item["Name"].clone();
                result.record["field_sources"]["title"] = json!("Emby");
            }
        }
        let item =
            &crate::output::emby_source(&result.record).ok_or("缺少 Emby 文件关联。")?["item"];
        let missing_title =
            needs_title(item) && usable(&result.record["fields"]["title"], item).is_none();
        let awaiting_title = missing_title
            && event_age(&event.path)
                .await
                .map_err(|error| format!("无法读取入库事件时间：{error}"))?
                < RETRY_WINDOW;
        let written = write_automatic(
            &collector.client,
            &emby,
            &paths,
            &result.record,
            awaiting_title,
        )
        .await?;
        if !central && (!awaiting_title || written > 0) {
            // The legacy route loads local sidecars into Emby.
            emby.refresh_sidecars(&collector.client, &event.id).await?;
        }
        if written > 0 {
            let saved = collector.save_lookup(result).await;
            if let Some(error) = saved.storage_error {
                eprintln!("Automatic scrape {} snapshot: {error}", event.id);
            }
        }
        if awaiting_title {
            return Ok(Outcome::AwaitingTitle);
        }
        if missing_title {
            eprintln!("Automatic scrape Episode {} retained its episode number after seven days without a sourced title.", event.id);
        }
        Ok(Outcome::Written(written))
    }
}

async fn sync_mapped_episode(
    collector: &Collector,
    emby: &Emby,
    server: &str,
    item: &Value,
    bangumi_id: Option<u32>,
    central: bool,
) -> Result<Outcome, String> {
    let item_id = item["Id"].as_str().ok_or("Emby 条目缺少 ID。")?;
    let people = if central && missing_people(item) {
        collector.verified_episode_people(emby, item).await
    } else {
        Vec::new()
    };
    let _write = collector.automatic_write_lock(item_id).await;
    let fresh;
    let item = if !has_provider_id(item, "Bangumi")
        && let Some(id) = bangumi_id
    {
        if emby
            .fill_episode_bangumi_id(&collector.client, item, id)
            .await?
        {
            eprintln!("Automatic scrape Episode {item_id} filled Bangumi ID {id} from PostgreSQL.");
        }
        fresh = emby
            .automatic_item(&collector.client, item_id, "Episode")
            .await?
            .ok_or("Emby 条目在写入期间已消失。")?;
        &fresh.data["item"]
    } else {
        item
    };
    let refreshed;
    let item = if emby
        .fill_automatic_people(&collector.client, item, &people)
        .await?
    {
        refreshed = emby
            .automatic_item(&collector.client, item_id, "Episode")
            .await?
            .ok_or("Emby 条目在写入期间已消失。")?;
        &refreshed.data["item"]
    } else {
        item
    };
    collector
        .upsert_emby_item(server, item_id, item, None)
        .await?;
    Ok(Outcome::Skipped)
}

fn emby_airdate(value: &Value, timezone: &TimeZone) -> Result<Option<Date>, String> {
    if value.is_null() || value.as_str().is_some_and(|date| date.trim().is_empty()) {
        return Ok(None);
    }
    let parsed = value.as_str().map(str::trim).and_then(|date| {
        if date.len() == 10 {
            date.parse::<Date>().ok()
        } else if let Ok(timestamp) = date.parse::<Timestamp>() {
            Some(timezone.to_datetime(timestamp).date())
        } else if date
            .get(10..)
            .is_some_and(|time| !time.contains(['+', '-', 'Z', 'z']))
        {
            // A timestamp without an offset already represents local wall time.
            date.parse::<DateTime>().ok().map(|local| local.date())
        } else {
            None
        }
    });
    parsed
        .map(Some)
        .ok_or_else(|| "Emby 首播日格式无效，无法核对分集。".into())
}

#[cfg(test)]
#[test]
fn emby_airdate_uses_the_configured_calendar_day() {
    let shanghai = TimeZone::get("Asia/Shanghai").unwrap();
    let new_york = TimeZone::get("America/New_York").unwrap();
    for (input, zone, day) in [
        ("2021-01-11T16:00:00.0000000Z", &shanghai, "2021-01-12"),
        ("2026-11-01", &shanghai, "2026-11-01"),
        ("2026-11-01T01:00:00+09:00", &shanghai, "2026-11-01"),
        ("2026-11-01T00:00:00", &shanghai, "2026-11-01"),
        ("2026-07-01T04:30:00Z", &new_york, "2026-07-01"),
        ("2026-01-01T04:30:00Z", &new_york, "2025-12-31"),
    ] {
        assert_eq!(emby_airdate(&json!(input), zone), Ok(Some(day.parse().unwrap())));
    }
    assert_eq!(emby_airdate(&Value::Null, &shanghai), Ok(None));
    assert!(emby_airdate(&json!("2021-01-11T16:00:00+99:00"), &shanghai).is_err());
    assert!(emby_airdate(&json!("2021-02-30"), &shanghai).is_err());
}

async fn event_age(path: &Path) -> std::io::Result<Duration> {
    Ok(tokio::fs::metadata(path)
        .await?
        .modified()?
        .elapsed()
        .unwrap_or_default())
}

fn complete_episode(item: &Value) -> bool {
    !needs_title(item)
        && item["Overview"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        && item["ImageTags"]["Primary"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_map_is_scoped_to_series_and_rejects_invalid_subjects() {
        let map = SeasonMap::parse(r#"{"102":{"2":[120,140]},"103":{"2":[160]}}"#).unwrap();
        assert_eq!(
            map.subjects(&json!({"Type":"Season","SeriesId":"102","IndexNumber":2})),
            &[120, 140]
        );
        assert_eq!(
            map.subjects(
                &json!({"Type":"Episode","SeriesId":"103","ParentIndexNumber":2,"IndexNumber":1})
            ),
            &[160]
        );
        assert!(
            map.subjects(
                &json!({"Type":"Episode","SeriesId":"102","ParentIndexNumber":1,"IndexNumber":2})
            )
            .is_empty()
        );
        for invalid in [
            r#"{"102":{"0":[120]}}"#,
            r#"{"102":{"2":[]}}"#,
            r#"{"102":{"2":[0]}}"#,
            r#"{"102":{"2":[120,120]}}"#,
        ] {
            assert!(SeasonMap::parse(invalid).is_err());
        }
    }

    #[test]
    fn existing_episode_requires_a_title_an_overview_and_a_primary_image() {
        let mut item = json!({"Type":"Episode","Name":"A real title","IndexNumber":7,
            "Overview":"Keep this","ImageTags":{"Primary":"existing-tag"}});
        assert!(complete_episode(&item));
        item["Name"] = json!("第 7 集");
        assert!(!complete_episode(&item));
        item["Name"] = json!("A real title");
        item["Overview"] = json!(" ");
        assert!(!complete_episode(&item));
    }
}
