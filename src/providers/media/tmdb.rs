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

    pub(super) async fn details(
        &self,
        client: &Client,
        kind: MediaKind,
        id: u32,
        language: &str,
    ) -> Source {
        let namespace = namespace(kind);
        let page = Url::parse(&format!("https://www.themoviedb.org/{namespace}/{id}")).unwrap();
        let mut details = self.endpoint(&format!("{namespace}/{id}"), language);
        let extra = if kind == MediaKind::Movie {
            "credits,release_dates"
        } else {
            "aggregate_credits,content_ratings"
        };
        details
            .query_pairs_mut()
            .append_pair("append_to_response", &format!("{COMMON},{extra}"))
            .append_pair(
                "include_image_language",
                &format!("{},en,null", language.split('-').next().unwrap_or(language)),
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

        let mut stage_note = None;
        if kind == MediaKind::Series {
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
        let mut source = Source::found(NAME, &page, fields(kind, id, &data), data);
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

fn fields(kind: MediaKind, id: u32, data: &Value) -> Fields {
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
    let trailer = list(&data["videos"]["results"]).find_map(|video| {
        let key = video["key"].as_str()?;
        let safe_key = !key.is_empty()
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        (safe_key && video["type"] == "Trailer" && video["site"] == "YouTube")
            .then(|| format!("https://www.youtube.com/watch?v={key}"))
    });
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
        (
            "rating",
            if data["vote_count"].as_u64().is_some_and(|count| count > 0) {
                json!({"value": data["vote_average"], "scale": 10, "count": data["vote_count"]})
            } else {
                Value::Null
            },
        ),
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
            json!(image(&data["backdrop_path"], "w1280")),
        ),
        ("trailer_url", json!(trailer)),
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
    fn maps_tmdb_movies_and_series_without_confusing_localized_and_original_fields() {
        let movie = json!({
            "id": 496243, "title": "寄生虫", "original_title": "기생충", "original_language": "ko",
            "release_date": "2019-05-30", "poster_path": "/test.jpg",
            "credits": {"crew": [{"job": "Director", "name": "Bong Joon Ho"}]},
            "external_ids": {"imdb_id": "tt6751668"}, "vote_average": 8.5, "vote_count": 10
        });
        let hit = candidate(MediaKind::Movie, &movie).unwrap();
        assert_eq!(hit.match_titles, ["寄生虫", "기생충"]);
        assert_eq!(hit.original_languages, ["ko"]);
        assert_eq!(hit.year, Some(2019));
        let movie_fields = fields(MediaKind::Movie, 496243, &movie);
        assert_eq!(movie_fields["imdb_id"], "tt6751668");
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
        let show_fields = fields(MediaKind::Series, 70523, &show);
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
                assert_eq!(headers[AUTHORIZATION], "Bearer fake-test-token");
                let query = uri.query().unwrap_or_default();
                let language = if uri.path() == "/3/search/movie" { "ja" } else { "zh-CN" };
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
                    "/3/tv/70523" => {
                        assert!(query.contains("aggregate_credits"));
                        json!({"id":70523,"name":"暗黑","original_language":"de","seasons":[{"season_number":0},{"season_number":1}]})
                    }
                    "/3/tv/70523/season/0" => json!({"season_number":0,"episodes":[{"id":1,"name":"Special","episode_number":1,"season_number":0}]}),
                    _ => return (StatusCode::NOT_FOUND, Json(Value::Null)),
                };
                (StatusCode::OK, Json(body))
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut tmdb = Tmdb::new("fake-test-token").unwrap();
        tmdb.api_root =
            Url::parse(&format!("http://{}/3/", listener.local_addr().unwrap())).unwrap();
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
        let series = tmdb
            .details(&client, MediaKind::Series, 70523, "zh-CN")
            .await;
        assert_eq!(series.status, "partial");
        assert_eq!(series.fields["episodes"][0]["season"], 0);
        server.abort();
    }
}
