use reqwest::{Client, Url};
use serde_json::{Value, json};

#[cfg(feature = "web")]
use crate::providers::media::episode_title;
use crate::{
    collect::Source,
    providers::{Failure, field_map},
};

use super::super::emby;
#[cfg(feature = "web")]
use super::list;
use super::{NAME, Tmdb, id, image, languages, rating};

struct Episode {
    series: u32,
    season: u64,
    number: u64,
    id: Option<u64>,
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
        let empty_ids = Value::Null;
        let ids = if series_override.is_some() {
            &empty_ids
        } else {
            &item["ProviderIds"]
        };
        let expected_id = ids["Tmdb"].as_str().and_then(id).map(u64::from);
        let episode = if let Some(series) = series_override {
            Episode {
                series,
                season: item["ParentIndexNumber"].as_u64()?,
                number: number.filter(|number| *number > 0)?,
                id: None,
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
        let Episode {
            series,
            season,
            number,
            id,
        } = episode;
        let path = format!("tv/{series}/season/{season}/episode/{number}");
        let page = Url::parse(&format!("https://www.themoviedb.org/{path}")).unwrap();
        let mut endpoint = self.endpoint(&path, language);
        endpoint
            .query_pairs_mut()
            .append_pair("append_to_response", "external_ids,translations");
        let series_endpoint = self.endpoint(&format!("tv/{series}"), language);
        let (details, show) = tokio::join!(
            self.get(client, &endpoint),
            self.get(client, &series_endpoint)
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
            Ok(_) => return Some(Source::failure(NAME, &page, Failure::NotFound)),
            Err(error) => return Some(Source::failure(NAME, &page, error)),
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
        ]);
        if series_override.is_some() {
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
        Some(source)
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
        let mut source = self
            .emby_episode_with_series(client, emby, item, language, Some(series))
            .await?;
        let episode = &source.data["episode"];
        if let Some(id) = episode["id"].as_u64() {
            source.fields.insert("tmdb_id", json!(id.to_string()));
        }
        if let Some(title) =
            automatic_episode_title(episode, &source.data["series"], item, language)
        {
            source.fields.insert("title", json!(title));
        } else {
            source.fields.remove("title");
        }
        Some(source)
    }

    #[cfg(feature = "web")]
    pub(crate) async fn emby_season(
        &self,
        client: &Client,
        series: u32,
        number: u64,
        language: &str,
    ) -> Result<Source, String> {
        let path = format!("tv/{series}/season/{number}");
        let data = self
            .get(client, &self.endpoint(&path, language))
            .await
            .map_err(|error| format!("TMDB：{}", error.description()))?;
        if data["season_number"].as_u64() != Some(number) || data["name"].as_str().is_none() {
            return Err("TMDB 返回的季号不匹配。".into());
        }
        let page = Url::parse(&format!("https://www.themoviedb.org/{path}")).unwrap();
        let fields = field_map([
            ("title", data["name"].clone()),
            ("description", data["overview"].clone()),
            ("poster_url", json!(image(&data["poster_path"], "w500"))),
            ("release_date", data["air_date"].clone()),
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
