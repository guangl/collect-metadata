use reqwest::{Client, Url};
use serde_json::{Value, json};

#[cfg(feature = "web")]
use crate::providers::media::episode_title;
use crate::{
    collect::Source,
    providers::{Failure, field_map},
};

use super::super::emby;
use super::list;
use super::{NAME, Tmdb, id, image, languages, rating};

pub(super) struct Episode {
    pub(super) series: u32,
    pub(super) season: u64,
    pub(super) number: u64,
    pub(super) id: Option<u64>,
}

impl Tmdb {
    pub(crate) async fn emby_episode(
        &self,
        client: &Client,
        emby: &emby::Emby,
        item: &Value,
        language: &str,
    ) -> Option<Source> {
        self.emby_episode_with_series(client, emby, item, language, None)
            .await
    }

    pub(crate) async fn emby_episode_with_series(
        &self,
        client: &Client,
        emby: &emby::Emby,
        item: &Value,
        language: &str,
        series_override: Option<u32>,
    ) -> Option<Source> {
        let number = item["IndexNumber"].as_u64();
        if number
            .zip(item["IndexNumberEnd"].as_u64())
            .is_some_and(|(start, end)| end > start)
        {
            return None;
        }
        let ids = &item["ProviderIds"];
        let expected_id = ids["Tmdb"].as_str().and_then(id).map(u64::from);
        let episode = if let Some(series) = series_override {
            match self.find_episode(client, ids, language).await {
                Ok(episode) if episode.series == series => episode,
                Ok(_) => return None,
                Err(_) => Episode {
                    series,
                    season: item["ParentIndexNumber"].as_u64()?,
                    number: number.filter(|number| *number > 0)?,
                    id: expected_id,
                },
            }
        } else {
            match self.find_episode(client, ids, language).await {
                Ok(episode) => episode,
                Err(error) => {
                    let fallback = match (item["ParentIndexNumber"].as_u64(), number) {
                        (Some(season), Some(number)) if number > 0 => emby
                            .series_provider_id(client, item, "Tmdb")
                            .await
                            .as_deref()
                            .and_then(id)
                            .map(|series| Episode {
                                series,
                                season,
                                number,
                                id: expected_id,
                            }),
                        _ => None,
                    };
                    let Some(episode) = fallback else {
                        let page = Url::parse("https://www.themoviedb.org/").unwrap();
                        return Some(Source::failure(NAME, &page, error));
                    };
                    episode
                }
            }
        };
        Some(
            self.episode_source(client, item, language, episode, series_override.is_some())
                .await,
        )
    }

    async fn episode_source(
        &self,
        client: &Client,
        item: &Value,
        language: &str,
        episode: Episode,
        automatic: bool,
    ) -> Source {
        let ids = &item["ProviderIds"];
        let expected_id = ids["Tmdb"].as_str().and_then(super::id).map(u64::from);
        let Episode {
            series,
            season,
            number,
            id,
        } = episode;
        let path = format!("tv/{series}/season/{season}/episode/{number}");
        let page = Url::parse(&format!("https://www.themoviedb.org/{path}")).unwrap();
        let mut endpoint = self.endpoint(&path, language);
        let extras = if automatic {
            "external_ids,translations,credits"
        } else {
            "external_ids,translations"
        };
        endpoint
            .query_pairs_mut()
            .append_pair("append_to_response", extras);
        let (details, show) = tokio::join!(
            self.get(client, &endpoint),
            self.episode_series_details(client, series, language)
        );
        let details = match details {
            Ok(data)
                if data["season_number"].as_u64() == Some(season)
                    && data["episode_number"].as_u64() == Some(number)
                    && data["id"].as_u64().is_some_and(|actual| {
                        actual > 0
                            && id.is_none_or(|id| id == actual)
                            && expected_id.is_none_or(|id| id == actual)
                    })
                    && ![("Imdb", "imdb_id"), ("Tvdb", "tvdb_id")].into_iter().any(
                        |(key, field)| {
                            let external = &data["external_ids"][field];
                            let actual = external
                                .as_str()
                                .map(str::to_owned)
                                .or_else(|| external.as_u64().map(|value| value.to_string()));
                            ids[key]
                                .as_str()
                                .filter(|id| !id.is_empty())
                                .zip(actual.as_deref())
                                .is_some_and(|(expected, actual)| expected != actual)
                        },
                    ) =>
            {
                data
            }
            Ok(_) => return Source::failure(NAME, &page, Failure::NotFound),
            Err(error) => return Source::failure(NAME, &page, error),
        };
        let show = show.and_then(|data| {
            (data["id"].as_u64() == Some(u64::from(series)))
                .then_some(data)
                .ok_or_else(Failure::format_changed)
        });
        let (series_data, note) = match show {
            Ok(data) => (data, None),
            Err(error) => (
                Value::Null,
                Some(format!(
                    "无法读取所属剧集的原始语言。{}",
                    error.description()
                )),
            ),
        };
        let mut fields = field_map([
            ("release_date", details["air_date"].clone()),
            ("rating", rating(&details)),
            ("original_languages", json!(languages(&series_data))),
            ("imdb_id", details["external_ids"]["imdb_id"].clone()),
            (
                "tvdb_id",
                json!(
                    details["external_ids"]["tvdb_id"]
                        .as_u64()
                        .map(|id| id.to_string())
                ),
            ),
            (
                "title_original",
                original_episode_name(&details, &series_data),
            ),
        ]);
        if automatic {
            fields.extend(field_map([
                ("title", details["name"].clone()),
                ("description", details["overview"].clone()),
                ("series", series_data["name"].clone()),
                ("still_url", json!(image(&details["still_path"], "w500"))),
            ]));
        }
        let mut source = Source::found(
            NAME,
            &page,
            fields,
            json!({
                "episode": details,
                "series": series_data,
            }),
        );
        if note.is_some() {
            source.status = "partial";
            source.note = note;
        }
        source
    }

    async fn episode_series_details(
        &self,
        client: &Client,
        series: u32,
        language: &str,
    ) -> Result<Value, Failure> {
        let key = format!("{series}:{language}");
        if let Some(entry) = self.series_details.get(&key) {
            return Ok((*entry).clone());
        }
        let data = self
            .get(client, &self.endpoint(&format!("tv/{series}"), language))
            .await?;
        if data["id"].as_u64() == Some(u64::from(series)) {
            self.series_details
                .insert(&key, std::sync::Arc::new(data.clone()));
        }
        Ok(data)
    }

    #[cfg(feature = "web")]
    pub(crate) async fn verified_episode_credits(
        &self,
        client: &Client,
        item: &Value,
        series: u32,
        language: &str,
    ) -> Option<Value> {
        let expected = emby::provider_id(item, "Tmdb").and_then(id)?;
        let season = item["ParentIndexNumber"].as_u64()?;
        let number = item["IndexNumber"].as_u64().filter(|number| *number > 0)?;
        if item["IndexNumberEnd"]
            .as_u64()
            .is_some_and(|end| end > number)
        {
            return None;
        }
        let path = format!("tv/{series}/season/{season}/episode/{number}/credits");
        self.get(client, &self.endpoint(&path, language))
            .await
            .ok()
            .filter(|credits| credits["id"].as_u64() == Some(u64::from(expected)))
    }

    #[cfg(feature = "web")]
    pub(crate) async fn automatic_episode(
        &self,
        client: &Client,
        emby: &emby::Emby,
        item: &Value,
        language: &str,
        series: u32,
    ) -> Option<Source> {
        let source = self
            .emby_episode_with_series(client, emby, item, language, Some(series))
            .await?;
        Some(prepare_automatic_source(source, item, language))
    }

    #[cfg(feature = "web")]
    pub(crate) async fn matched_group_episode(
        &self,
        client: &Client,
        item: &Value,
        language: &str,
        series: u32,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<Option<Source>, String> {
        let Some((episode, airdate)) = self.grouped_episode(client, item, series, timezone).await?
        else {
            return Ok(None);
        };
        let source = self
            .episode_source(client, item, language, episode, true)
            .await;
        if source.data["episode"].is_null() {
            return if source.status == "not_found" {
                Ok(None)
            } else {
                Err(source
                    .note
                    .unwrap_or_else(|| "TMDB 分组分集读取失败。".into()))
            };
        }
        if source.data["episode"]["air_date"]
            .as_str()
            .and_then(|date| date.parse::<jiff::civil::Date>().ok())
            != Some(airdate)
        {
            return Ok(None);
        }
        Ok(Some(prepare_automatic_source(source, item, language)))
    }

    #[cfg(feature = "web")]
    pub(crate) fn uses_episode_group(&self, item: &Value, series: u32) -> bool {
        item["ParentIndexNumber"]
            .as_u64()
            .is_some_and(|season| self.has_group(series, season))
    }

    #[cfg(feature = "web")]
    pub(crate) async fn matched_automatic_episode(
        &self,
        client: &Client,
        emby: &emby::Emby,
        item: &Value,
        language: &str,
        series: u32,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<Option<Source>, String> {
        if self.uses_episode_group(item, series) {
            return self
                .matched_group_episode(client, item, language, series, timezone)
                .await;
        }
        let Some(source) = self
            .automatic_episode(client, emby, item, language, series)
            .await
        else {
            return Ok(None);
        };
        let episode = &source.data["episode"];
        if episode.is_null() {
            // The automatic match reports identity conflicts as NotFound.
            return if source.status == "not_found" {
                Ok(None)
            } else {
                Err(source.note.unwrap_or_else(|| "TMDB 分集读取失败。".into()))
            };
        }
        Ok(verified_episode_identity(item, episode, timezone).then_some(source))
    }

    #[cfg(feature = "web")]
    pub(crate) async fn emby_season(
        &self,
        client: &Client,
        emby: &emby::Emby,
        series: u32,
        item: &Value,
        language: &str,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<Source, String> {
        let number = item["IndexNumber"].as_u64().ok_or("Emby 条目缺少季号。")?;
        let path = format!("tv/{series}/season/{number}");
        let mut endpoint = self.endpoint(&path, language);
        endpoint
            .query_pairs_mut()
            .append_pair("append_to_response", "external_ids,aggregate_credits");
        let data = self
            .get(client, &endpoint)
            .await
            .map_err(|error| format!("TMDB：{}", error.description()))?;
        if !season_matches(&data, item, timezone)
            && (!compatible_season(&data, item)
                || !season_matches_episodes(
                    &data,
                    &emby.season_episodes(client, item).await?,
                    timezone,
                ))
        {
            return Err("TMDB 季号、季 ID 或首播日与 Emby 冲突，需要核对分季映射。".into());
        }
        let page = Url::parse(&format!("https://www.themoviedb.org/{path}")).unwrap();
        let fields = field_map([
            ("title", data["name"].clone()),
            ("description", data["overview"].clone()),
            ("poster_url", json!(image(&data["poster_path"], "w500"))),
            ("release_date", data["air_date"].clone()),
            ("rating", rating(&data)),
            (
                "tvdb_id",
                json!(
                    data["external_ids"]["tvdb_id"]
                        .as_u64()
                        .map(|id| id.to_string())
                ),
            ),
            ("season_number", json!(number)),
            (
                "tmdb_id",
                json!(data["id"].as_u64().map(|id| id.to_string())),
            ),
        ]);
        Ok(Source::found(NAME, &page, fields, data))
    }

    async fn find_episode(
        &self,
        client: &Client,
        ids: &Value,
        language: &str,
    ) -> Result<Episode, Failure> {
        let imdb = ids["Imdb"].as_str().filter(|value| {
            value.strip_prefix("tt").is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
        });
        let tvdb = ids["Tvdb"].as_str().filter(|value| id(value).is_some());
        let mut failure = Failure::NotFound;
        for (source, value) in [("imdb_id", imdb), ("tvdb_id", tvdb)] {
            let Some(value) = value else { continue };
            let mut endpoint = self.endpoint(&format!("find/{value}"), language);
            endpoint
                .query_pairs_mut()
                .append_pair("external_source", source);
            match self.get(client, &endpoint).await {
                Ok(data) => {
                    if let Some([item]) = data["tv_episode_results"].as_array().map(Vec::as_slice)
                        && let Some(series) = item["show_id"]
                            .as_u64()
                            .and_then(|id| u32::try_from(id).ok())
                            .filter(|id| *id > 0)
                        && let Some(season) = item["season_number"].as_u64()
                        && let Some(number) =
                            item["episode_number"].as_u64().filter(|number| *number > 0)
                        && let Some(id) = item["id"].as_u64().filter(|id| *id > 0)
                    {
                        return Ok(Episode {
                            series,
                            season,
                            number,
                            id: Some(id),
                        });
                    }
                }
                Err(Failure::NotFound) => {}
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}

#[cfg(feature = "web")]
fn prepare_automatic_source(mut source: Source, item: &Value, language: &str) -> Source {
    let episode = &source.data["episode"];
    if let Some(id) = episode["id"].as_u64() {
        source.fields.insert("tmdb_id", json!(id.to_string()));
    }
    if let Some(title) = automatic_episode_title(episode, &source.data["series"], item, language) {
        source.fields.insert("title", json!(title));
    } else {
        source.fields.remove("title");
    }
    source
}

fn original_episode_name(episode: &Value, series: &Value) -> Value {
    let language = series["original_language"].as_str();
    list(&episode["translations"]["translations"])
        .find_map(|entry| {
            (language.is_some() && entry["iso_639_1"].as_str() == language)
                .then(|| {
                    entry["data"]["name"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                })
                .flatten()
        })
        .map_or(Value::Null, |name| json!(name))
}

#[cfg(feature = "web")]
fn verified_episode_identity(item: &Value, episode: &Value, timezone: &jiff::tz::TimeZone) -> bool {
    let exact_id = [
        ("Tmdb", episode["id"].as_u64().map(|id| id.to_string())),
        (
            "Tvdb",
            episode["external_ids"]["tvdb_id"]
                .as_u64()
                .map(|id| id.to_string()),
        ),
        (
            "Imdb",
            episode["external_ids"]["imdb_id"]
                .as_str()
                .map(str::to_owned),
        ),
    ]
    .into_iter()
    .any(|(provider, source)| {
        source
            .as_deref()
            .is_some_and(|id| emby::provider_id(item, provider) == Some(id))
    });
    if exact_id {
        return true;
    }
    let local = [&item["Name"], &item["OriginalTitle"]]
        .into_iter()
        .filter_map(|title| episode_title::usable(title, item))
        .map(emby::normalize)
        .collect::<Vec<_>>();
    let matched_title = std::iter::once(&episode["name"])
        .chain(list(&episode["translations"]["translations"]).map(|entry| &entry["data"]["name"]))
        .filter_map(|title| episode_title::usable(title, item))
        .any(|title| local.contains(&emby::normalize(title)));
    let dates =
        local_premiere_date(item, timezone).zip(episode_title::calendar_date(&episode["air_date"]));
    matched_title && dates.is_none_or(|(local, source)| (local - source).whole_days().abs() <= 1)
        || dates.is_some_and(|(local, source)| local == source)
}

#[cfg(feature = "web")]
fn local_premiere_date(item: &Value, timezone: &jiff::tz::TimeZone) -> Option<time::Date> {
    let local_date = item["PremiereDate"]
        .as_str()
        .and_then(|date| date.parse::<jiff::Timestamp>().ok())
        .map(|timestamp| json!(timezone.to_datetime(timestamp).date().to_string()));
    episode_title::calendar_date(local_date.as_ref().unwrap_or(&item["PremiereDate"]))
}

#[cfg(feature = "web")]
fn season_ids(data: &Value, item: &Value) -> (Option<bool>, Option<bool>) {
    let tmdb = emby::provider_id(item, "Tmdb")
        .and_then(id)
        .map(|expected| data["id"].as_u64() == Some(u64::from(expected)));
    let tvdb = emby::provider_id(item, "Tvdb")
        .and_then(id)
        .zip(data["external_ids"]["tvdb_id"].as_u64())
        .map(|(expected, actual)| u64::from(expected) == actual);
    (tmdb, tvdb)
}

#[cfg(feature = "web")]
fn compatible_season(data: &Value, item: &Value) -> bool {
    let (tmdb, tvdb) = season_ids(data, item);
    data["season_number"].as_u64() == item["IndexNumber"].as_u64()
        && data["id"].as_u64().is_some_and(|id| id > 0)
        && data["name"].as_str().is_some()
        && tmdb != Some(false)
        && tvdb != Some(false)
}

#[cfg(feature = "web")]
fn season_matches(data: &Value, item: &Value, timezone: &jiff::tz::TimeZone) -> bool {
    let (tmdb, tvdb) = season_ids(data, item);
    compatible_season(data, item)
        && (tmdb == Some(true)
            || tvdb == Some(true)
            || local_premiere_date(item, timezone)
                .zip(episode_title::calendar_date(&data["air_date"]))
                .is_none_or(|(local, source)| (local - source).whole_days().abs() <= 1))
}

#[cfg(feature = "web")]
fn season_matches_episodes(data: &Value, local: &[Value], timezone: &jiff::tz::TimeZone) -> bool {
    let mut verified = std::collections::HashSet::new();
    for item in local {
        let Some(number) = item["IndexNumber"].as_u64().filter(|&number| number > 0) else {
            return false;
        };
        if item["IndexNumberEnd"]
            .as_u64()
            .is_some_and(|end| end > number)
        {
            return false;
        }
        let Some(id) = emby::provider_id(item, "Tmdb").and_then(id) else {
            return false;
        };
        let mut hits = list(&data["episodes"]).filter(|episode| {
            episode["episode_number"] == number && episode["season_number"] == data["season_number"]
        });
        let Some(episode) = hits.next().filter(|_| hits.next().is_none()) else {
            return false;
        };
        if episode["id"] != id {
            return false;
        }
        if let Some((local_date, source_date)) = local_premiere_date(item, timezone)
            .zip(episode_title::calendar_date(&episode["air_date"]))
        {
            if (local_date - source_date).whole_days().abs() > 1 {
                return false;
            }
            verified.insert(number);
        }
    }
    verified.len() >= 3
}

#[cfg(all(test, feature = "web"))]
#[test]
fn season_episode_evidence_requires_distinct_matching_ids_and_dates() {
    let zone = jiff::tz::TimeZone::get("Asia/Shanghai").unwrap();
    let source = json!({"season_number":2,"episodes":[
        {"season_number":2,"episode_number":1,"id":31,"air_date":"2026-09-04"},
        {"season_number":2,"episode_number":2,"id":32,"air_date":"2026-09-11"},
        {"season_number":2,"episode_number":3,"id":33,"air_date":"2026-09-18"}
    ]});
    let mut local = vec![
        json!({"IndexNumber":1,"ProviderIds":{"Tmdb":"31"},"PremiereDate":"2026-09-04"}),
        json!({"IndexNumber":2,"ProviderIds":{"Tmdb":"32"},"PremiereDate":"2026-09-11"}),
        json!({"IndexNumber":3,"ProviderIds":{"Tmdb":"33"},"PremiereDate":"2026-09-18"}),
    ];
    assert!(season_matches_episodes(&source, &local, &zone));
    assert!(!season_matches_episodes(&source, &local[..2], &zone));
    local[2]["ProviderIds"]["Tmdb"] = json!("34");
    assert!(!season_matches_episodes(&source, &local, &zone));
}

#[cfg(all(test, feature = "web"))]
#[test]
fn backfill_requires_evidence_beyond_episode_number() {
    let zone = jiff::tz::TimeZone::get("Asia/Shanghai").unwrap();
    let source = json!({"id":55,"name":"惊喜","air_date":"2022-10-30","episode_number":3});
    let mut item = json!({"Type":"Episode","Name":"第 3 集","IndexNumber":3});
    assert!(!verified_episode_identity(&item, &source, &zone));
    item["PremiereDate"] = json!("2022-10-29T16:00:00Z");
    assert!(verified_episode_identity(&item, &source, &zone));
    item["PremiereDate"] = json!("2022-11-05T16:00:00Z");
    assert!(!verified_episode_identity(&item, &source, &zone));
}

#[cfg(all(test, feature = "web"))]
#[test]
fn automatic_season_requires_consistent_local_evidence() {
    let zone = jiff::tz::TimeZone::get("Asia/Shanghai").unwrap();
    let source = json!({"season_number":2,"name":"Season 2","id":123,"air_date":"2026-09-27",
                        "external_ids":{"tvdb_id":27}});
    let mut item =
        json!({"IndexNumber":2,"ProviderIds":{"Tmdb":"123"},"PremiereDate":"2026-09-26T16:00:00Z"});
    assert!(season_matches(&source, &item, &zone));
    item["ProviderIds"]["Tmdb"] = json!("124");
    assert!(!season_matches(&source, &item, &zone));
    item["ProviderIds"]["Tmdb"] = Value::Null;
    item["PremiereDate"] = json!("2026-08-27");
    assert!(!season_matches(&source, &item, &zone));
    item["ProviderIds"]["Tvdb"] = json!("27");
    assert!(season_matches(&source, &item, &zone));
    item["ProviderIds"]["Tvdb"] = json!("28");
    assert!(!season_matches(&source, &item, &zone));
    item["ProviderIds"]["Tvdb"] = Value::Null;
    item["PremiereDate"] = json!("2026-09-25T16:00:00Z");
    assert!(season_matches(&source, &item, &zone));
}

#[cfg(feature = "web")]
fn automatic_episode_title<'a>(
    episode: &'a Value,
    series: &Value,
    item: &Value,
    language: &str,
) -> Option<&'a str> {
    let (lang, region) = language.split_once('-').unwrap_or((language, ""));
    let translations = &episode["translations"]["translations"];
    let matching = |language: &str, region: Option<&str>| {
        list(translations)
            .filter(|entry| {
                entry["iso_639_1"] == language && region.is_none_or(|r| entry["iso_3166_1"] == r)
            })
            .find_map(|entry| episode_title::usable(&entry["data"]["name"], item))
    };
    matching(lang, Some(region))
        .or_else(|| matching(lang, None))
        .or_else(|| episode_title::usable(&episode["name"], item))
        .or_else(|| {
            series["original_language"]
                .as_str()
                .and_then(|lang| matching(lang, None))
        })
        .or_else(|| matching("en", None))
}

#[cfg(all(test, feature = "web"))]
#[test]
fn automatic_episode_title_ignores_numbered_translations() {
    let mut episode = json!({"name":"第 7 集", "translations":{"translations":[
        {"iso_639_1":"zh", "iso_3166_1":"CN", "data":{"name":"第7集"}},
        {"iso_639_1":"zh", "iso_3166_1":"TW", "data":{"name":"重逢"}},
        {"iso_639_1":"ja", "iso_3166_1":"JP", "data":{"name":"再会"}}
    ]}});
    let series = json!({"original_language":"ja"});
    let item = json!({"IndexNumber":7});
    assert_eq!(
        automatic_episode_title(&episode, &series, &item, "zh-CN"),
        Some("重逢")
    );
    episode["translations"]["translations"][1]["data"]["name"] = json!("第７話");
    assert_eq!(
        automatic_episode_title(&episode, &series, &item, "zh-CN"),
        Some("再会")
    );
    episode["translations"] = Value::Null;
    assert_eq!(
        automatic_episode_title(&episode, &series, &item, "zh-CN"),
        None
    );
}
