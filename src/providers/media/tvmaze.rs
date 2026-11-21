use super::*;
use crate::providers::{field_map, text};
use scraper::Html;
use serde_json::json;

const NAME: &str = "TVmaze";

fn url(path: &str) -> Url {
    Url::parse(&format!("https://api.tvmaze.com/{path}")).unwrap()
}

pub(super) async fn search(client: &Client, query: &str) -> Result<Vec<Candidate>, Failure> {
    let mut endpoint = url("search/shows");
    endpoint.query_pairs_mut().append_pair("q", query);
    let result = get_json(client, &endpoint).await?;
    let hits = result.as_array().ok_or(Failure::format_changed())?;
    Ok(hits
        .iter()
        .filter_map(|hit| {
            let show = &hit["show"];
            let id = show["id"].as_u64()?;
            Some(Candidate {
                provider: MediaProvider::Tvmaze,
                id: id.to_string(),
                title: show["name"].as_str()?.to_owned(),
                match_titles: vec![show["name"].as_str()?.to_owned()],
                year: show["premiered"].as_str().and_then(year),
                original_languages: languages(show),
                cover_url: show["image"]["medium"].as_str().map(str::to_owned),
                tvmaze_id: Some(id),
                tmdb_id: None,
                bangumi_id: None,
            })
        })
        .collect())
}

pub(super) async fn details(client: &Client, id: u64) -> Source {
    let page = Url::parse(&format!("https://www.tvmaze.com/shows/{id}")).unwrap();
    let mut endpoint = url(&format!("shows/{id}"));
    for section in ["seasons", "cast", "crew", "images", "akas"] {
        endpoint.query_pairs_mut().append_pair("embed[]", section);
    }
    let mut episodes_url = url(&format!("shows/{id}/episodes"));
    episodes_url.query_pairs_mut().append_pair("specials", "1");
    let (details, episodes) =
        tokio::join!(get_json(client, &endpoint), get_json(client, &episodes_url));
    match details {
        Ok(mut data) if data["id"].as_u64() == Some(id) => {
            let complete = match episodes {
                Ok(episodes) if episodes.is_array() => {
                    data["_embedded"]["episodeswithspecials"] = episodes;
                    true
                }
                _ => false,
            };
            let mut source = Source::found(NAME, &page, fields(&data), data);
            if !complete {
                source.status = "partial";
                source.note = Some("已取得剧集资料，分集列表暂不可用。".into());
            }
            source
        }
        Ok(_) => Source::failure(NAME, &page, Failure::format_changed()),
        Err(error) => Source::failure(NAME, &page, error),
    }
}

fn fields(data: &Value) -> crate::collect::Fields {
    let embedded = &data["_embedded"];
    let cast: Vec<_> = embedded["cast"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|credit| credit["person"]["name"].as_str())
        .collect();
    let episodes: Vec<_> = embedded["episodeswithspecials"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|episode| {
            json!({
                "id": episode["id"],
                "season": episode["season"],
                "number": episode["number"],
                "title": episode["name"],
                "date": episode["airdate"],
                "runtime_minutes": episode["runtime"],
                "url": episode["url"],
            })
        })
        .collect();
    let summary = data["summary"].as_str().map(|html| {
        let document = Html::parse_fragment(html);
        text(document.root_element())
    });
    let channel = data["network"]["name"]
        .as_str()
        .or_else(|| data["webChannel"]["name"].as_str());
    field_map([
        ("title", data["name"].clone()),
        ("release_date", data["premiered"].clone()),
        ("end_date", data["ended"].clone()),
        ("original_languages", json!(languages(data))),
        ("runtime_minutes", data["averageRuntime"].clone()),
        (
            "rating",
            data["rating"]["average"]
                .as_f64()
                .map(|value| json!({"value":value,"scale":10}))
                .unwrap_or_default(),
        ),
        ("cast", json!(cast)),
        ("genres", data["genres"].clone()),
        ("description", json!(summary)),
        ("network", json!(channel)),
        ("status", data["status"].clone()),
        (
            "season_count",
            json!(embedded["seasons"].as_array().map(Vec::len)),
        ),
        (
            "episode_count",
            json!(embedded["episodeswithspecials"].as_array().map(Vec::len)),
        ),
        ("episodes", json!(episodes)),
        ("imdb_id", data["externals"]["imdb"].clone()),
        ("cover_url", data["image"]["original"].clone()),
        ("official_url", data["officialSite"].clone()),
    ])
}

fn languages(show: &Value) -> Vec<String> {
    let code = match show["language"].as_str().unwrap_or_default() {
        "English" => "en",
        "Chinese" => "zh",
        "Mandarin" => "cmn",
        "Cantonese" => "yue",
        "Japanese" => "ja",
        "Korean" => "ko",
        "French" => "fr",
        "German" => "de",
        "Spanish" => "es",
        "Italian" => "it",
        "Russian" => "ru",
        "Portuguese" => "pt",
        "Thai" => "th",
        "Hindi" => "hi",
        "Turkish" => "tr",
        "Dutch" => "nl",
        "Arabic" => "ar",
        "Danish" => "da",
        "Norwegian" => "no",
        "Swedish" => "sv",
        "Finnish" => "fi",
        "Icelandic" => "is",
        "Polish" => "pl",
        "Czech" => "cs",
        "Hebrew" => "he",
        "Greek" => "el",
        "Hungarian" => "hu",
        "Indonesian" => "id",
        "Vietnamese" => "vi",
        "Ukrainian" => "uk",
        "Persian" => "fa",
        "Bengali" => "bn",
        "Tamil" => "ta",
        "Telugu" => "te",
        "Malayalam" => "ml",
        "Malay" => "ms",
        "Urdu" => "ur",
        "Tagalog" => "tl",
        _ => return Vec::new(),
    };
    vec![code.to_owned()]
}
