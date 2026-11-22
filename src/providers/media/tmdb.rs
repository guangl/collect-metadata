use futures_util::{StreamExt, stream};
use reqwest::header::HeaderValue;
use serde_json::json;
use tokio::sync::Semaphore;

use super::*;
use crate::{
    collect::Fields,
    providers::{field_map, request_with_auth},
};

const NAME: &str = "TMDB";
const COMMON: &str = "alternative_titles,external_ids,images,keywords,recommendations,similar,translations,videos,watch/providers";

pub(crate) struct Tmdb {
    api_root: Url,
    credential: Credential,
    requests: Semaphore,
}

enum Credential {
    ApiKey(String),
    Bearer(HeaderValue),
}

struct Episode {
    series: u32,
    season: u64,
    number: u64,
    id: Option<u64>,
}

impl Tmdb {
    pub(crate) fn new(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.is_empty()
            || token.len() > 8192
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return None;
        }
        let credential = if token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            Credential::ApiKey(token.to_owned())
        } else {
            let mut authorization = HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
            authorization.set_sensitive(true);
            Credential::Bearer(authorization)
        };
        Some(Self {
            api_root: Url::parse("https://api.themoviedb.org/3/").unwrap(),
            credential,
            requests: Semaphore::new(4),
        })
    }

    pub(crate) async fn validate(&self, client: &Client) -> Result<(), String> {
        let endpoint = self.api_root.join("authentication").unwrap();
        match self.get(client, &endpoint).await {
            Ok(data) if data["success"] == true => Ok(()),
            Ok(_) => Err("TMDB 未确认此凭据有效。".into()),
            Err(error) => Err(format!("TMDB 连接测试失败。{}", error.description())),
        }
    }

    fn endpoint(&self, path: &str, language: &str) -> Url {
        let mut url = self.api_root.join(path).unwrap();
        url.query_pairs_mut().append_pair("language", language);
        url
    }

    async fn get(&self, client: &Client, endpoint: &Url) -> Result<Value, Failure> {
        let _request = self
            .requests
            .acquire()
            .await
            .expect("TMDB semaphore is open");
        let body = match &self.credential {
            Credential::ApiKey(key) => {
                let mut endpoint = endpoint.clone();
                endpoint.query_pairs_mut().append_pair("api_key", key);
                request_with_auth(client, &endpoint, "application/json", None).await?
            }
            Credential::Bearer(authorization) => {
                request_with_auth(client, endpoint, "application/json", Some(authorization)).await?
            }
        };
        serde_json::from_str(&body).map_err(|_| Failure::format_changed())
    }

    pub(super) async fn search(
        &self,
        client: &Client,
        kind: MediaKind,
        query: &str,
        language: &str,
    ) -> Result<Vec<Candidate>, Failure> {
        let mut endpoint = self.endpoint(&format!("search/{}", namespace(kind)), language);
        endpoint
            .query_pairs_mut()
            .append_pair("query", query)
            .append_pair("include_adult", "false");
        let result = self.get(client, &endpoint).await?;
        let hits = result["results"]
            .as_array()
            .ok_or_else(Failure::format_changed)?;
        Ok(hits
            .iter()
            .filter(|hit| hit["adult"].as_bool() != Some(true))
            .filter_map(|hit| candidate(kind, hit))
            .collect())
    }

    #[cfg(feature = "web")]
    pub(crate) async fn automatic_candidates(
        &self, client: &Client, kind: MediaKind, query: &str, language: &str,
    ) -> Vec<Candidate> {
        self.search(client, kind, query, language).await.unwrap_or_default()
    }

    pub(super) async fn search_id(
        &self,
        client: &Client,
        kind: MediaKind,
        id: u32,
        language: &str,
    ) -> Result<Vec<Candidate>, Failure> {
        let endpoint = self.endpoint(&format!("{}/{id}", namespace(kind)), language);
        match self.get(client, &endpoint).await {
            Ok(data) if data["id"].as_u64() == Some(u64::from(id)) => {
                Ok(candidate(kind, &data).into_iter().collect())
            }
            Ok(_) => Err(Failure::format_changed()),
            Err(Failure::NotFound) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

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
    pub(crate) async fn emby_season(
        &self,
        client: &Client,
        series: u32,
        number: u64,
        language: &str,
    ) -> Result<Source, String> {
        let path = format!("tv/{series}/season/{number}");
        let data = self.get(client, &self.endpoint(&path, language)).await
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
            ("tmdb_id", json!(data["id"].as_u64().map(|id| id.to_string()))),
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

    pub(super) async fn details(
        &self,
        client: &Client,
        kind: MediaKind,
        id: u32,
        language: &str,
    ) -> Source {
        self.work_details(client, kind, id, language, true).await
    }

    pub(crate) async fn work_details(
        &self,
        client: &Client,
        kind: MediaKind,
        id: u32,
        language: &str,
        include_episodes: bool,
    ) -> Source {
        let namespace = namespace(kind);
        let page = Url::parse(&format!("https://www.themoviedb.org/{namespace}/{id}")).unwrap();
        let mut details = self.endpoint(&format!("{namespace}/{id}"), language);
        let extra = if kind == MediaKind::Movie {
            "credits,release_dates"
        } else {
            "aggregate_credits,content_ratings"
        };
        let components = if include_episodes {
            format!("{COMMON},{extra}")
        } else {
            format!("external_ids,images,keywords,translations,{extra}")
        };
        details
            .query_pairs_mut()
            .append_pair("append_to_response", &components)
            .append_pair(
                "include_image_language",
                &format!("{},ja,en,ko,null", language.split('-').next().unwrap_or(language)),
            );
        let mut data = match self.get(client, &details).await {
            Ok(data)
                if data["id"].as_u64() == Some(u64::from(id)) && title(kind, &data).is_some() =>
            {
                data
            }
            Ok(_) => return Source::failure(NAME, &page, Failure::format_changed()),
            Err(error) => return Source::failure(NAME, &page, error),
        };

        if include_episodes && language.split('-').next() != Some("en")
            && !list(&data["videos"]["results"]).any(|video| video["type"] == "Trailer")
        {
            let endpoint = self.endpoint(&format!("{namespace}/{id}/videos"), "en-US");
            if let Ok(videos) = self.get(client, &endpoint).await
                && videos["id"].as_u64() == Some(u64::from(id))
                && videos["results"].is_array()
            {
                data["videos_fallback_en"] = videos;
            }
        }

        let mut stage_note = None;
        if include_episodes && kind == MediaKind::Series {
            let season_numbers = data["seasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|season| season["season_number"].as_u64())
                .collect::<Vec<_>>();
            let mut attempts = stream::iter(season_numbers)
                .map(|number| async move {
                    let url = self.endpoint(&format!("tv/{id}/season/{number}"), language);
                    let details = self.get(client, &url).await.ok().filter(|data| {
                        data["season_number"].as_u64() == Some(number)
                            && data["episodes"].is_array()
                    });
                    (number, details)
                })
                .buffer_unordered(4)
                .collect::<Vec<_>>()
                .await;
            attempts.sort_unstable_by_key(|(number, _)| *number);
            let missing = attempts
                .iter()
                .filter(|(_, details)| details.is_none())
                .count();
            data["season_details"] = Value::Array(
                attempts
                    .into_iter()
                    .filter_map(|(_, details)| details)
                    .collect(),
            );
            if missing > 0 {
                stage_note = Some(format!("已取得剧集资料，有 {missing} 季的分集暂不可用。"));
            }
        }
        let mut source = Source::found(NAME, &page, fields(kind, id, &data, language), data);
        if let Some(note) = stage_note {
            source.status = "partial";
            source.note = Some(note);
        }
        source
    }
}

pub(super) fn id(value: &str) -> Option<u32> {
    value
        .parse()
        .ok()
        .filter(|id| (1..=i32::MAX as u32).contains(id))
}

fn namespace(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Series => "tv",
        MediaKind::Av => unreachable!("TMDB only provides movie and series metadata"),
    }
}

fn title(kind: MediaKind, data: &Value) -> Option<&str> {
    data[if kind == MediaKind::Series {
        "name"
    } else {
        "title"
    }]
    .as_str()
    .filter(|title| !title.trim().is_empty())
    .or_else(|| original_title(kind, data))
}

fn original_title(kind: MediaKind, data: &Value) -> Option<&str> {
    data[if kind == MediaKind::Series {
        "original_name"
    } else {
        "original_title"
    }]
    .as_str()
    .filter(|title| !title.trim().is_empty())
}

fn release_date(kind: MediaKind, data: &Value) -> &Value {
    &data[if kind == MediaKind::Series {
        "first_air_date"
    } else {
        "release_date"
    }]
}

fn languages(data: &Value) -> Vec<&str> {
    data["original_language"]
        .as_str()
        .filter(|language| {
            matches!(language.len(), 2 | 3)
                && language.bytes().all(|byte| byte.is_ascii_lowercase())
        })
        .into_iter()
        .collect()
}

fn rating(data: &Value) -> Value {
    match (data["vote_average"].as_f64(), data["vote_count"].as_u64()) {
        (Some(value), Some(count)) if count > 0 => {
            json!({"value": value, "scale": 10, "count": count})
        }
        _ => Value::Null,
    }
}

fn image(path: &Value, size: &str) -> Option<String> {
    let path = path.as_str()?.strip_prefix('/')?;
    if path.is_empty()
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(format!("https://image.tmdb.org/t/p/{size}/{path}"))
}

fn artwork(data: &Value, key: &str, locale: &str, landscape: bool, size: &str) -> Option<String> {
    let preferred = locale.split('-').next().unwrap_or(locale);
    let original = data["original_language"].as_str().unwrap_or("");
    let logo = key == "logos";
    list(&data["images"][key])
        .filter_map(|entry| {
            let language = entry["iso_639_1"].as_str();
            let path = entry["file_path"].as_str()?;
            let width = entry["width"].as_u64()?;
            if logo {
                if !path.ends_with(".png") || width < 280 { return None; }
            } else if !(1.7..1.85).contains(&entry["aspect_ratio"].as_f64()?)
                || (landscape && (language.is_none() || width < 900))
                || (!landscape && (language.is_some() || width < 1280))
            {
                return None;
            }
            let preference = match language {
                Some(lang) if lang == preferred => 4,
                Some(lang) if lang == original => 3,
                Some("ja") => 2,
                Some("en") => 1,
                Some("ko") | None => 0,
                _ => return None,
            };
            let votes = entry["vote_average"].as_f64().unwrap_or_default();
            Some((preference, votes, width, image(&entry["file_path"], size)?))
        })
        .max_by(|a,b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)))
        .map(|(_,_,_,url)| url)
}

fn candidate(kind: MediaKind, data: &Value) -> Option<Candidate> {
    let id = data["id"].as_u64().and_then(|id| i32::try_from(id).ok())?;
    if id <= 0 {
        return None;
    }
    let title = title(kind, data)?;
    let mut match_titles = vec![title.to_owned()];
    if let Some(original) = original_title(kind, data).filter(|original| *original != title) {
        match_titles.push(original.to_owned());
    }
    Some(Candidate {
        provider: MediaProvider::Tmdb,
        id: id.to_string(),
        title: title.to_owned(),
        match_titles,
        year: release_date(kind, data).as_str().and_then(year),
        original_languages: languages(data).into_iter().map(str::to_owned).collect(),
        cover_url: image(&data["poster_path"], "w342"),
        tvmaze_id: None,
        tmdb_id: Some(id as u32),
        bangumi_id: None,
    })
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn names(value: &Value) -> Vec<&str> {
    list(value)
        .filter_map(|item| item["name"].as_str())
        .collect()
}

fn fields(kind: MediaKind, id: u32, data: &Value, language: &str) -> Fields {
    let series = kind == MediaKind::Series;
    let credits = &data[if series {
        "aggregate_credits"
    } else {
        "credits"
    }];
    let crew = |job: &str| {
        list(&credits["crew"])
            .filter(|credit| {
                credit["job"] == job || list(&credit["jobs"]).any(|part| part["job"] == job)
            })
            .filter_map(|credit| credit["name"].as_str())
            .collect::<Vec<_>>()
    };
    let episodes = list(&data["season_details"]).flat_map(|season| list(&season["episodes"]))
        .map(|episode| json!({
            "id": episode["id"],
            "season": episode["season_number"],
            "number": episode["episode_number"],
            "title": episode["name"],
            "date": episode["air_date"],
            "runtime_minutes": episode["runtime"],
            "description": episode["overview"],
            "still_url": image(&episode["still_path"], "w500"),
            "url": match (episode["season_number"].as_u64(), episode["episode_number"].as_u64()) {
                (Some(season), Some(number)) => Some(format!("https://www.themoviedb.org/tv/{id}/season/{season}/episode/{number}")),
                _ => None,
            },
        }))
        .collect::<Vec<_>>();
    let mut videos = list(&data["videos"]["results"])
        .chain(list(&data["videos_fallback_en"]["results"]))
        .filter_map(|video| {
            let key = video["key"].as_str()?;
            if key.is_empty()
                || key.len() > 64
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            let url = match video["site"].as_str()? {
                "YouTube" => format!("https://www.youtube.com/watch?v={key}"),
                "Vimeo" if key.bytes().all(|byte| byte.is_ascii_digit()) => {
                    format!("https://vimeo.com/{key}")
                }
                _ => return None,
            };
            Some(json!({
                "type": video["type"], "name": video["name"], "url": url,
                "language": video["iso_639_1"], "official": video["official"],
            }))
        })
        .collect::<Vec<_>>();
    videos.sort_by_key(|video| video["official"] != true);
    let trailer = videos
        .iter()
        .find(|video| video["type"] == "Trailer")
        .map(|video| video["url"].clone())
        .unwrap_or_default();
    let original_description = list(&data["translations"]["translations"])
        .filter(|entry| {
            entry["iso_639_1"].is_string() && entry["iso_639_1"] == data["original_language"]
        })
        .filter_map(|entry| entry["data"]["overview"].as_str())
        .find(|value| !value.trim().is_empty())
        .or_else(|| {
            (language.split('-').next()? == data["original_language"].as_str()?)
                .then(|| data["overview"].as_str())
                .flatten()
        });
    let region = language
        .rsplit_once('-')
        .map(|(_, region)| region)
        .unwrap_or("US");
    let official_rating = if series {
        list(&data["content_ratings"]["results"])
            .find(|entry| entry["iso_3166_1"] == region)
            .and_then(|entry| entry["rating"].as_str())
    } else {
        list(&data["release_dates"]["results"])
            .filter(|entry| entry["iso_3166_1"] == region)
            .flat_map(|entry| list(&entry["release_dates"]))
            .filter_map(|entry| entry["certification"].as_str())
            .find(|value| !value.trim().is_empty())
    };
    let countries = names(&data["production_countries"]);
    field_map([
        ("title", json!(title(kind, data))),
        ("title_original", json!(original_title(kind, data))),
        ("tmdb_id", json!(id.to_string())),
        ("release_date", release_date(kind, data).clone()),
        (
            "end_date",
            if series && data["in_production"] == false {
                data["last_air_date"].clone()
            } else {
                Value::Null
            },
        ),
        ("original_languages", json!(languages(data))),
        ("description", data["overview"].clone()),
        ("description_original", json!(original_description)),
        ("official_rating", json!(official_rating)),
        ("tagline", data["tagline"].clone()),
        (
            "runtime_minutes",
            if series {
                list(&data["episode_run_time"])
                    .next()
                    .cloned()
                    .unwrap_or_default()
            } else {
                data["runtime"].clone()
            },
        ),
        ("genres", json!(names(&data["genres"]))),
        (
            "tags",
            json!(names(
                &data["keywords"][if series { "results" } else { "keywords" }]
            )),
        ),
        ("collection", data["belongs_to_collection"]["name"].clone()),
        (
            "countries",
            if countries.is_empty() {
                data["origin_country"].clone()
            } else {
                json!(countries)
            },
        ),
        ("studio", json!(names(&data["production_companies"]))),
        ("cast", json!(names(&credits["cast"]))),
        ("directors", json!(crew("Director"))),
        ("writers", json!(crew("Writer"))),
        ("producers", json!(crew("Producer"))),
        ("network", json!(names(&data["networks"]))),
        ("rating", rating(data)),
        ("status", data["status"].clone()),
        ("season_count", data["number_of_seasons"].clone()),
        ("episode_count", data["number_of_episodes"].clone()),
        ("episodes", json!(episodes)),
        (
            "imdb_id",
            json!(
                data["external_ids"]["imdb_id"]
                    .as_str()
                    .or_else(|| data["imdb_id"].as_str())
            ),
        ),
        ("cover_url", json!(image(&data["poster_path"], "w500"))),
        (
            "backdrop_url",
            json!(artwork(data, "backdrops", language, false, "w1280")
                .or_else(|| image(&data["backdrop_path"], "w1280"))),
        ),
        ("logo_url", json!(artwork(data, "logos", language, false, "w500"))),
        ("landscape_url", json!(artwork(data, "backdrops", language, true, "w1280"))),
        ("trailer_url", trailer),
        ("videos", json!(videos)),
        ("official_url", data["homepage"].clone()),
        (
            "watch_providers_source",
            data["watch/providers"]["results"]
                .as_object()
                .filter(|providers| !providers.is_empty())
                .map(|_| json!("JustWatch"))
                .unwrap_or_default(),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_localized_landscape_from_clean_background_and_keeps_png_logos() {
        let data = json!({"original_language":"ja","images":{
            "backdrops":[
                {"file_path":"/background.jpg","width":1920,"aspect_ratio":1.78,"iso_639_1":null},
                {"file_path":"/landscape.jpg","width":1920,"aspect_ratio":1.78,"iso_639_1":"ja"}],
            "logos":[{"file_path":"/text.svg","width":800,"iso_639_1":"zh"},
                {"file_path":"/text.png","width":800,"iso_639_1":"ja"}]
        }});
        let result = fields(MediaKind::Series, 100, &data, "zh-CN");
        assert_eq!(result["backdrop_url"], "https://image.tmdb.org/t/p/w1280/background.jpg");
        assert_eq!(result["landscape_url"], "https://image.tmdb.org/t/p/w1280/landscape.jpg");
        assert_eq!(result["logo_url"], "https://image.tmdb.org/t/p/w500/text.png");
    }

    #[test]
    fn maps_tmdb_movies_and_series_without_confusing_localized_and_original_fields() {
        let movie = json!({
            "id": 496243, "title": "寄生虫", "original_title": "기생충", "original_language": "ko",
            "release_date": "2019-05-30", "poster_path": "/test.jpg",
            "credits": {"crew": [{"job": "Director", "name": "Bong Joon Ho"}]},
            "external_ids": {"imdb_id": "tt6751668"}, "vote_average": 8.5, "vote_count": 10,
            "translations": {"translations": [{"iso_639_1":"ko","data":{"overview":"원문"}}]},
            "release_dates": {"results": [{"iso_3166_1":"CN","release_dates":[{"certification":"R"}]}]},
            "videos":{"results":[
                {"type":"Trailer","key":"unofficial","site":"YouTube","official":false},
                {"type":"Trailer","key":"official","site":"YouTube","official":true},
                {"type":"Behind the Scenes","key":"12345","site":"Vimeo"},
                {"type":"Trailer","key":"x?y","site":"YouTube"}
            ]}
        });
        let hit = candidate(MediaKind::Movie, &movie).unwrap();
        assert_eq!(hit.match_titles, ["寄生虫", "기생충"]);
        assert_eq!(hit.original_languages, ["ko"]);
        assert_eq!(hit.year, Some(2019));
        let movie_fields = fields(MediaKind::Movie, 496243, &movie, "zh-CN");
        assert_eq!(movie_fields["imdb_id"], "tt6751668");
        assert_eq!(movie_fields["description_original"], "원문");
        assert_eq!(movie_fields["official_rating"], "R");
        assert_eq!(
            movie_fields["trailer_url"],
            "https://www.youtube.com/watch?v=official"
        );
        assert_eq!(movie_fields["videos"].as_array().unwrap().len(), 3);
        assert_eq!(movie_fields["directors"][0], "Bong Joon Ho");
        assert_eq!(
            movie_fields["cover_url"],
            "https://image.tmdb.org/t/p/w500/test.jpg"
        );

        let show = json!({
            "name": "暗黑", "original_language": "de", "number_of_seasons": 3,
            "first_air_date": "2017-12-01", "in_production": false, "last_air_date": "2020-06-27",
            "season_details": [{"season_number": 0, "episodes": [{"season_number":0,"episode_number":1,"name":"Special"}]}]
        });
        let show_fields = fields(MediaKind::Series, 70523, &show, "zh-CN");
        assert_eq!(show_fields["original_languages"][0], "de");
        assert_eq!(show_fields["end_date"], "2020-06-27");
        assert_eq!(show_fields["episodes"][0]["season"], 0);
        assert_eq!(
            show_fields["episodes"][0]["url"],
            "https://www.themoviedb.org/tv/70523/season/0/episode/1"
        );
    }

    #[cfg(feature = "web")]
    #[tokio::test]
    async fn searches_and_collects_over_http_using_the_token() {
        use axum::{Json, Router, http::StatusCode, routing::get};
        use reqwest::header::{AUTHORIZATION, HeaderMap};

        let app = Router::new().fallback(get(
            |headers: HeaderMap, uri: axum::http::Uri| async move {
                if uri.path() == "/emby/Items" {
                    assert_eq!(headers["x-emby-token"], "fake-emby-token");
                    assert!(uri.query().unwrap_or_default().contains("Ids=70"));
                    return (StatusCode::OK, Json(json!({"Items": [
                        {"Id":"70","Type":"Series","ProviderIds":{"Tmdb":"70523"}}
                    ]})));
                }
                assert_eq!(headers[AUTHORIZATION], "Bearer fake-test-token");
                let query = uri.query().unwrap_or_default();
                let language = match uri.path() {
                    "/3/search/movie" => "ja",
                    path if path.ends_with("/videos") => "en-US",
                    _ => "zh-CN",
                };
                assert!(query.contains(&format!("language={language}")));
                assert!(!query.contains("fake-test-token"));
                let body = match uri.path() {
                    "/3/search/movie" => {
                        assert!(query.contains("query=Parasite+2019"));
                        json!({"results":[{"id":496243,"title":"寄生虫","original_language":"ko"}]})
                    }
                    "/3/movie/496243" => {
                        assert!(query.contains("release_dates"));
                        json!({"id":496243,"title":"寄生虫","original_language":"ko","translations":{"translations":[]}})
                    }
                    "/3/movie/496243/videos" => json!({"id":496243,"results":[{"key":"trailer","type":"Trailer","site":"YouTube"}]}),
                    "/3/tv/70523" => {
                        assert!(query == "language=zh-CN" || query.contains("aggregate_credits"));
                        json!({"id":70523,"name":"暗黑","original_language":"de","seasons":[{"season_number":0},{"season_number":1}]})
                    }
                    "/3/tv/70523/season/0" => json!({"season_number":0,"episodes":[{"id":1,"name":"Special","episode_number":1,"season_number":0}]}),
                    "/3/find/tt123" => {
                        assert!(query.contains("external_source=imdb_id"));
                        json!({"tv_episode_results":[{"id":1,"show_id":70523,"season_number":0,"episode_number":1}]})
                    }
                    "/3/tv/70523/season/0/episode/1" => {
                        assert!(query.contains("append_to_response=external_ids"));
                        json!({"id":1,"season_number":0,"episode_number":1,"air_date":"2017-12-01",
                            "vote_average":8.1,"vote_count":17,"external_ids":{"imdb_id":"tt123"}})
                    }
                    _ => return (StatusCode::NOT_FOUND, Json(Value::Null)),
                };
                (StatusCode::OK, Json(body))
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let mut tmdb = Tmdb::new("fake-test-token").unwrap();
        tmdb.api_root = Url::parse(&format!("{address}/3/")).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        let found = tmdb
            .search(&client, MediaKind::Movie, "Parasite 2019", "ja")
            .await
            .unwrap_or_else(|error| panic!("{}", error.description()));
        assert_eq!(found[0].id, "496243");
        let movie = tmdb
            .details(&client, MediaKind::Movie, 496243, "zh-CN")
            .await;
        assert_eq!(movie.status, "ok");
        assert!(movie.data["translations"].is_object());
        assert_eq!(
            movie.fields["trailer_url"],
            "https://www.youtube.com/watch?v=trailer"
        );
        let series = tmdb
            .details(&client, MediaKind::Series, 70523, "zh-CN")
            .await;
        assert_eq!(series.status, "partial");
        assert_eq!(series.fields["episodes"][0]["season"], 0);

        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("emby.json");
        std::fs::write(
            &config,
            json!({
                "enabled":true, "server":address, "api_key":"fake-emby-token",
            })
            .to_string(),
        )
        .unwrap();
        let emby = emby::EmbySettings::load(config)
            .unwrap()
            .connection()
            .unwrap();
        let mut item = json!({"SeriesId":"70","ParentIndexNumber":0,"IndexNumber":1});
        for ids in [json!({"Imdb":"tt123"}), json!({"Tmdb":"1"})] {
            item["ProviderIds"] = ids;
            let episode = tmdb
                .emby_episode(&client, &emby, &item, "zh-CN")
                .await
                .unwrap();
            assert_eq!(episode.status, "ok");
            assert_eq!(episode.fields["release_date"], "2017-12-01");
            assert_eq!(
                episode.fields["rating"],
                json!({"value":8.1,"scale":10,"count":17})
            );
            assert_eq!(episode.fields["original_languages"], json!(["de"]));
        }
        item["ProviderIds"] = json!({"Tmdb":"2"});
        let mismatch = tmdb
            .emby_episode(&client, &emby, &item, "zh-CN")
            .await
            .unwrap();
        assert_eq!(mismatch.status, "not_found");
        assert!(mismatch.fields.is_empty());
        item["IndexNumberEnd"] = json!(2);
        assert!(
            tmdb.emby_episode(&client, &emby, &item, "zh-CN")
                .await
                .is_none()
        );
        server.abort();
    }
}
