use std::collections::HashSet;

use reqwest::header;
use serde_json::json;
use unicode_normalization::UnicodeNormalization;

use super::*;
use crate::providers::{field_map, request_text};

const NAME: &str = "Bangumi";
const PAGE_SIZE: usize = 200;

pub(crate) fn id(value: &str) -> Option<u32> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().filter(|&value| value > 0)
}

pub(crate) fn id_query(query: &str) -> Option<&str> {
    let (prefix, id) = query.split_once([':', '：'])?;
    prefix
        .trim()
        .eq_ignore_ascii_case("bangumi")
        .then_some(id.trim())
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn infobox<'a>(subject: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    list(&subject["infobox"])
        .filter(move |entry| entry["key"] == key)
        .flat_map(|entry| {
            let value = &entry["value"];
            text(value)
                .into_iter()
                .chain(list(value).filter_map(|entry| text(&entry["v"])))
        })
}

fn matches_kind(subject: &Value, kind: MediaKind) -> bool {
    match (subject["type"].as_u64(), subject["platform"].as_str()) {
        (Some(2 | 6), Some("剧场版" | "电影" | "Movie")) => kind == MediaKind::Movie,
        (Some(2), Some("TV" | "OVA" | "OAD" | "WEB"))
        | (Some(6), Some("日剧" | "欧美剧" | "华语剧" | "电视剧" | "综艺")) => {
            kind == MediaKind::Series
        }
        _ => false,
    }
}

fn image(value: &Value, sizes: &[&str]) -> Option<String> {
    sizes.iter().find_map(|&size| {
        let url = Url::parse(text(&value[size])?).ok()?;
        (url.scheme() == "https"
            && url.host_str() == Some("lain.bgm.tv")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && (url.path().starts_with("/pic/") || url.path().starts_with("/r/")))
        .then(|| url.to_string())
    })
}

fn supports_kind(subject: &Value, kind: MediaKind) -> bool {
    matches_kind(subject, kind)
        || (kind != MediaKind::Av
            && matches!(subject["type"].as_u64(), Some(2 | 6))
            && subject["platform"] == "其他")
}

fn candidate(subject: &Value) -> Option<Candidate> {
    let id = u32::try_from(subject["id"].as_u64()?)
        .ok()
        .filter(|&id| id > 0)?;
    let title = text(&subject["name_cn"]).or_else(|| text(&subject["name"]))?;
    let match_titles = ["name_cn", "name"]
        .into_iter()
        .filter_map(|key| text(&subject[key]))
        .chain(infobox(subject, "别名"))
        .map(str::to_owned)
        .collect();
    Some(Candidate {
        provider: MediaProvider::Bangumi,
        id: id.to_string(),
        title: title.to_owned(),
        match_titles,
        year: text(&subject["date"]).and_then(year),
        original_languages: original_languages(subject),
        cover_url: image(&subject["images"], &["small", "common", "large"]),
        tvmaze_id: None,
        tmdb_id: None,
        bangumi_id: Some(id),
    })
}

pub(crate) struct Bangumi {
    api_root: Url,
    authorization: Option<header::HeaderValue>,
    cache: EntityCache,
}

impl Default for Bangumi {
    fn default() -> Self {
        Self {
            api_root: Url::parse("https://api.bgm.tv/v0/").unwrap(),
            authorization: None,
            cache: EntityCache::default(),
        }
    }
}

impl Bangumi {
    pub(crate) fn authenticated(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.is_empty()
            || token.len() > 8192
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return None;
        }
        let mut value = header::HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
        value.set_sensitive(true);
        Some(Self {
            authorization: Some(value),
            ..Self::default()
        })
    }

    pub(crate) async fn validate(&self, client: &Client) -> Result<(), String> {
        let user = self
            .api(client, "me", None)
            .await
            .map_err(|error| format!("Bangumi 连接测试失败。{}", error.description()))?;
        if !user["id"].as_u64().is_some_and(|id| id > 0) {
            return Err("Bangumi 未返回有效的用户资料。".into());
        }
        Ok(())
    }

    async fn api(
        &self,
        client: &Client,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Failure> {
        let url = self.api_root.join(path).expect("static Bangumi API path");
        let text = request_text(&url, |url| {
            let mut request = match body {
                Some(body) => client.post(url).json(body),
                None => client.get(url),
            }
            .header(header::ACCEPT, "application/json");
            if let Some(value) = &self.authorization {
                request = request.header(header::AUTHORIZATION, value.clone());
            }
            request
        })
        .await?;
        serde_json::from_str(&text).map_err(|_| Failure::format_changed())
    }

    pub(crate) async fn search(
        &self,
        client: &Client,
        kind: MediaKind,
        query: &str,
    ) -> Result<Vec<Candidate>, Failure> {
        let body = json!({ "keyword": query, "filter": { "type": [2, 6] } });
        let result = self
            .api(client, "search/subjects?limit=12", Some(&body))
            .await?;
        let hits = result["data"]
            .as_array()
            .ok_or_else(Failure::format_changed)?;
        Ok(hits
            .iter()
            .filter(|hit| matches_kind(hit, kind))
            .filter_map(candidate)
            .collect())
    }

    pub(super) async fn search_id(
        &self,
        client: &Client,
        kind: MediaKind,
        id: u32,
    ) -> Result<Vec<Candidate>, Failure> {
        let subject = self.subject(client, id).await?;
        if !supports_kind(&subject, kind) {
            return Err(wrong_kind());
        }
        candidate(&subject)
            .map(|candidate| vec![candidate])
            .ok_or_else(Failure::format_changed)
    }

    async fn subject(&self, client: &Client, id: u32) -> Result<Arc<Value>, Failure> {
        let key = format!("Bangumi-{id}");
        if let Some(subject) = self.cache.get(&key) {
            return Ok(subject);
        }
        let data = self.api(client, &format!("subjects/{id}"), None).await?;
        if data["id"].as_u64() != Some(u64::from(id)) {
            return Err(Failure::format_changed());
        }
        let data = Arc::new(data);
        self.cache.insert(&key, Arc::clone(&data));
        Ok(data)
    }

    async fn people(&self, client: &Client, id: u32, section: &str) -> Result<Value, Failure> {
        let people = self
            .api(client, &format!("subjects/{id}/{section}"), None)
            .await?;
        people
            .is_array()
            .then_some(people)
            .ok_or_else(Failure::format_changed)
    }

    async fn episodes(&self, client: &Client, id: u32, kind: MediaKind) -> (Vec<Value>, bool) {
        let mut episodes = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(9);
        for _ in 0..10 {
            let offset = episodes.len();
            let mut path = format!("episodes?subject_id={id}&limit={PAGE_SIZE}&offset={offset}");
            if kind == MediaKind::Movie {
                path.push_str("&type=0");
            }
            let Ok(Ok(mut data)) =
                tokio::time::timeout_at(deadline, self.api(client, &path, None)).await
            else {
                break;
            };
            let Some(total) = data["total"].as_u64() else {
                break;
            };
            if data["offset"].as_u64() != Some(offset as u64) {
                break;
            }
            let Some(page) = data["data"].as_array_mut() else {
                break;
            };
            let empty = page.is_empty();
            episodes.append(page);
            if episodes.len() as u64 >= total {
                return (episodes, true);
            }
            if empty {
                break;
            }
        }
        (episodes, false)
    }

    pub(crate) async fn emby_episode_overview(
        &self,
        client: &Client,
        emby: &emby::Emby,
        item: &Value,
    ) -> Option<Source> {
        let series_id = emby.series_provider_id(client, item, "Bangumi").await?;
        let (episodes, complete) = self.episodes(client, id(&series_id)?, MediaKind::Series).await;
        if !complete {
            return None;
        }
        let episode = matching_episode(&episodes, item)?;
        let description = text(&episode["desc"]).filter(|text| chinese_overview(text))?;
        let episode_id = episode["id"].as_u64().filter(|id| *id > 0)?;
        let page = Url::parse(&format!("https://bgm.tv/ep/{episode_id}")).ok()?;
        Some(Source::found(
            NAME,
            &page,
            field_map([("description", json!(description))]),
            json!({ "episode": episode, "overview_language": "zh" }),
        ))
    }

    pub(crate) async fn details(&self, client: &Client, kind: MediaKind, id: u32) -> Source {
        let page = Url::parse(&format!("https://bgm.tv/subject/{id}")).unwrap();
        let subject = match self.subject(client, id).await {
            Ok(subject) if supports_kind(&subject, kind) => subject,
            Ok(_) => return Source::failure(NAME, &page, wrong_kind()),
            Err(error) => return Source::failure(NAME, &page, error),
        };
        let (persons, characters, (episodes, complete)) = tokio::join!(
            self.people(client, id, "persons"),
            self.people(client, id, "characters"),
            self.episodes(client, id, kind),
        );
        let mut missing = Vec::new();
        if persons.is_err() {
            missing.push("幕后人员");
        }
        if characters.is_err() {
            missing.push("角色与配音");
        }
        if !complete {
            missing.push("分集");
        }
        let data = json!({
            "subject": subject.as_ref(),
            "persons": persons.unwrap_or_default(),
            "characters": characters.unwrap_or_default(),
            "episodes": episodes,
        });
        let mut source = Source::found(NAME, &page, fields(&data, kind, id), data);
        if !missing.is_empty() {
            source.status = "partial";
            source.note = Some(format!(
                "已取得作品资料，部分{}资料未能读取。",
                missing.join("、")
            ));
        }
        source
    }
}

#[cfg(all(test, feature = "web"))]
#[tokio::test]
async fn validates_the_token_using_the_authenticated_user_endpoint() {
    use axum::{Json, Router, http::HeaderMap, routing::get};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/v0/me",
        get(|headers: HeaderMap| async move {
            assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture-token");
            Json(json!({"id": 123, "username": "fixture"}))
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut provider = Bangumi::authenticated(" fixture-token ").unwrap();
    provider.api_root = Url::parse(&format!("http://{root}/v0/")).unwrap();
    provider.validate(&Client::new()).await.unwrap();
    server.abort();
}

pub(crate) fn chinese_overview(value: &str) -> bool {
    whatlang::detect(value).is_some_and(|info| info.is_reliable() && info.lang() == whatlang::Lang::Cmn)
}

fn matching_episode<'a>(episodes: &'a [Value], item: &Value) -> Option<&'a Value> {
    let number = item["IndexNumber"].as_u64().filter(|&number| number > 0)?;
    let season = item["ParentIndexNumber"].as_u64().filter(|&season| season > 0)?;
    if item["IndexNumberEnd"].as_u64().is_some_and(|end| end > number) {
        return None;
    }
    let premiere = text(&item["PremiereDate"]).and_then(|date| date.get(..10));
    let titles = [&item["Name"], &item["OriginalTitle"]].into_iter()
        .filter_map(text)
        .map(episode_title)
        .filter(|title| !title.is_empty())
        .collect::<Vec<_>>();
    let mut matches = episodes.iter().filter(|episode| {
        if episode["type"] != 0
            || episode_fields(episode).and_then(|fields| fields["number"].as_u64()) != Some(number)
        {
            return false;
        }
        match (premiere, text(&episode["airdate"])) {
            (Some(premiere), Some(airdate)) => premiere == airdate,
            _ => season == 1 && [&episode["name_cn"], &episode["name"]]
                .into_iter().filter_map(text).any(|name| titles.contains(&episode_title(name))),
        }
    });
    let matched = matches.next()?;
    matches.next().is_none().then_some(matched)
}

#[cfg(test)]
#[test]
fn matches_regular_episodes_by_number_and_date_or_first_season_title() {
    let episodes = vec![
        json!({"id":101,"type":0,"ep":9,"airdate":"2026-11-28","name_cn":"菲亚前往萨斯兰特"}),
        json!({"id":202,"type":1,"sort":9,"airdate":"2026-11-28","name_cn":"菲亚前往萨斯兰特"}),
    ];
    let mut emby = json!({
        "IndexNumber":9,"ParentIndexNumber":1,"PremiereDate":"2026-11-28T00:00:00Z",
        "Name":"菲亚，前往萨斯兰特"
    });
    assert_eq!(matching_episode(&episodes, &emby).unwrap()["id"], 101);
    emby["PremiereDate"] = json!("2026-11-27");
    assert!(matching_episode(&episodes, &emby).is_none());
    emby["PremiereDate"] = Value::Null;
    assert_eq!(matching_episode(&episodes, &emby).unwrap()["id"], 101);
    emby["IndexNumberEnd"] = json!(10);
    assert!(matching_episode(&episodes, &emby).is_none());
}

fn episode_title(value: &str) -> String {
    value.nfkc().filter(|value| value.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn wrong_kind() -> Failure {
    Failure::Unavailable("条目类型与所选类别不一致。".into())
}

fn episode_fields(episode: &Value) -> Option<Value> {
    let id = episode["id"].as_u64()?;
    let special = match episode["type"].as_u64()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let title_cn = text(&episode["name_cn"]);
    let title = title_cn
        .or_else(|| text(&episode["name"]))
        .unwrap_or(if special {
            "特别篇"
        } else {
            "未命名分集"
        });
    let number = if special {
        &episode["sort"]
    } else {
        episode
            .get("ep")
            .filter(|ep| ep.as_f64().is_some_and(|ep| ep > 0.0))
            .unwrap_or(&episode["sort"])
    };
    let number = number.as_f64().filter(|&number| number > 0.0);
    let number = match number {
        Some(number) if number.fract() == 0.0 && number <= f64::from(u32::MAX) => {
            json!(number as u32)
        }
        _ => json!(number),
    };
    Some(json!({
        "id": id,
        "season": special.then_some(0),
        "number": number,
        "title": title,
        "title_cn": title_cn,
        "title_original": text(&episode["name"]),
        "description": text(&episode["desc"]),
        "date": text(&episode["airdate"]),
        "runtime_minutes": episode_minutes(episode),
        "url": format!("https://bgm.tv/ep/{id}"),
    }))
}

fn episode_minutes(episode: &Value) -> Option<u64> {
    let seconds = episode["duration_seconds"].as_u64()?.checked_add(30)?;
    let minutes = seconds / 60;
    (minutes > 0).then_some(minutes)
}

pub(crate) fn is_cast(relation: &str) -> bool {
    matches!(relation, "主演" | "演员" | "配角")
}

fn language(value: &str) -> Option<&'static str> {
    Some(match value.trim() {
        "日语" | "日語" | "日本語" => "ja",
        "英语" | "英語" => "en",
        "汉语" | "漢語" | "中文" => "zh",
        "汉语普通话" | "普通话" | "普通話" | "国语" | "國語" => "cmn",
        "粤语" | "粵語" | "广东话" | "廣東話" => "yue",
        "韩语" | "韓語" | "朝鲜语" | "朝鮮語" => "ko",
        "法语" | "法語" => "fr",
        "德语" | "德語" => "de",
        "俄语" | "俄語" => "ru",
        "西班牙语" | "西班牙語" => "es",
        "意大利语" | "義大利語" => "it",
        "泰语" | "泰語" => "th",
        _ => return None,
    })
}

fn original_languages(subject: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    infobox(subject, "语言")
        .flat_map(|value| value.split(['、', '/', '／', '，', ',']))
        .filter_map(language)
        .filter(|code| seen.insert(*code))
        .map(str::to_owned)
        .collect()
}

fn fields(data: &Value, kind: MediaKind, id: u32) -> crate::collect::Fields {
    let subject = &data["subject"];
    let animation = subject["type"] == 2;
    let distinct = |names: Vec<&str>| {
        let mut seen = HashSet::new();
        json!(
            names
                .into_iter()
                .filter(|name| seen.insert(*name))
                .collect::<Vec<_>>()
        )
    };
    let staff = |roles: &[&str]| {
        distinct(
            list(&data["persons"])
                .filter(|person| {
                    text(&person["relation"]).is_some_and(|role| roles.contains(&role))
                })
                .filter_map(|person| text(&person["name"]))
                .collect(),
        )
    };
    let cast = list(&data["persons"])
        .filter(|person| !animation && text(&person["relation"]).is_some_and(is_cast));
    let actors = distinct(
        list(&data["characters"])
            .flat_map(|character| list(&character["actors"]))
            .chain(cast)
            .filter_map(|person| text(&person["name"]))
            .collect(),
    );
    let title_original = text(&subject["name"]);
    let title = text(&subject["name_cn"]).or(title_original);
    let episodes = if kind == MediaKind::Series {
        list(&data["episodes"])
            .filter_map(episode_fields)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let duration_key = if kind == MediaKind::Movie {
        "片长"
    } else {
        "单集片长"
    };
    let runtime = infobox(subject, duration_key)
        .find_map(|value| {
            value
                .trim_end_matches("分钟")
                .trim_end_matches("分")
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|&n| n > 0)
        })
        .or_else(|| {
            (kind == MediaKind::Movie)
                .then(|| list(&data["episodes"]).find_map(episode_minutes))
                .flatten()
        });
    let tags = list(&subject["tags"])
        .filter_map(|tag| text(&tag["name"]))
        .take(12)
        .collect::<Vec<_>>();
    let rating = subject["rating"]["score"]
        .as_f64()
        .filter(|&score| score > 0.0)
        .map(|score| json!({"value": score, "scale": 10, "count": subject["rating"]["total"]}));

    field_map([
        ("bangumi_id", json!(id.to_string())),
        ("title", json!(title)),
        ("title_original", json!(title_original)),
        ("original_languages", json!(original_languages(subject))),
        ("description", json!(text(&subject["summary"]))),
        ("release_date", json!(text(&subject["date"]))),
        ("runtime_minutes", json!(runtime)),
        (
            "episode_count",
            json!(
                subject["eps"]
                    .as_u64()
                    .filter(|&count| count > 0 && kind == MediaKind::Series)
            ),
        ),
        ("episodes", json!(episodes)),
        ("rating", json!(rating)),
        ("tags", json!(tags)),
        ("network", json!(infobox(subject, "播放电视台").next())),
        (
            "official_url",
            json!(infobox(subject, "官方网站").find(|url| {
                Url::parse(url).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                })
            })),
        ),
        ("directors", staff(&["导演", "监督"])),
        ("writers", staff(&["脚本", "编剧", "系列构成"])),
        ("producers", staff(&["制片人", "执行制片人"])),
        ("studio", staff(&["动画制作", "制作", "承制方/摄制"])),
        (if animation { "voice_actors" } else { "cast" }, actors),
        (
            "cover_url",
            json!(image(&subject["images"], &["large", "medium", "common"])),
        ),
    ])
}
