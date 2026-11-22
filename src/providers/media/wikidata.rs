use std::collections::{BTreeSet, HashSet};

use futures_util::{StreamExt, stream};

use super::*;
use crate::providers::field_map;
use serde_json::json;

const NAME: &str = "Wikidata";
const FILM_CLASSES: &[&str] = &[
    "Q11424",
    "Q20650540",
    "Q202866",
    "Q93204",
    "Q506240",
    "Q24862",
    "Q24869",
];
const SERIES_CLASSES: &[&str] = &["Q5398426", "Q1259759", "Q581714", "Q526877", "Q63952888"];

fn classes(kind: MediaKind) -> &'static [&'static str] {
    match kind {
        MediaKind::Movie => FILM_CLASSES,
        MediaKind::Series => SERIES_CLASSES,
        MediaKind::Av => &[],
    }
}

pub(super) fn valid_id(id: &str) -> bool {
    id.strip_prefix('Q')
        .and_then(|digits| digits.parse::<u64>().ok())
        .is_some_and(|id| id > 0)
        && id[1..].bytes().all(|byte| byte.is_ascii_digit())
}

async fn api(client: &Client, args: &[(&str, &str)]) -> Result<Value, Failure> {
    let mut url = Url::parse("https://www.wikidata.org/w/api.php").unwrap();
    url.query_pairs_mut()
        .append_pair("format", "json")
        .extend_pairs(args.iter().copied());
    let data = get_json(client, &url).await?;
    if data.get("error").is_some() {
        return Err(Failure::Unavailable("Wikidata 暂时无法完成请求。".into()));
    }
    Ok(data)
}

async fn entities(
    client: &Client,
    ids: &[&str],
    props: &str,
    languages: Option<&str>,
) -> Result<Value, Failure> {
    if ids.is_empty() {
        return Ok(json!({}));
    }
    let ids = ids.join("|");
    let mut args = vec![
        ("action", "wbgetentities"),
        ("ids", ids.as_str()),
        ("props", props),
    ];
    if let Some(languages) = languages {
        args.push(("languages", languages));
    }
    let mut response = api(client, &args).await?;
    if response["entities"].is_object() {
        Ok(response["entities"].take())
    } else {
        Err(Failure::format_changed())
    }
}

pub(super) async fn search(
    client: &Client,
    cache: &EntityCache,
    kind: MediaKind,
    query: &str,
) -> Result<SearchResult, Failure> {
    let search_hits = if valid_id(query) {
        vec![query.to_owned()]
    } else {
        let filter = classes(kind)
            .iter()
            .map(|id| format!("P31={id}"))
            .collect::<Vec<_>>()
            .join("|");
        let title = query.replace(['"', '\\'], " ");
        let search = format!("\"{}\" haswbstatement:{filter}", title.trim());
        let data = api(
            client,
            &[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", &search),
                ("srnamespace", "0"),
                ("srprop", ""),
                ("srlimit", "12"),
            ],
        )
        .await?;
        data["query"]["search"]
            .as_array()
            .ok_or(Failure::format_changed())?
            .iter()
            .filter_map(|hit| hit["title"].as_str())
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let had_candidates = !search_hits.is_empty();
    let mut entries = stream::iter(search_hits.into_iter().enumerate())
        .map(|(rank, id)| async move {
            let milliseconds = if rank < 2 { 5000 } else { 2500 };
            let data = tokio::time::timeout(
                Duration::from_millis(milliseconds),
                entity(client, cache, &id),
            )
            .await
            .ok()
            .and_then(Result::ok);
            (rank, id, data)
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    entries.sort_unstable_by_key(|(rank, _, _)| *rank);
    if had_candidates && entries.iter().all(|(_, _, entry)| entry.is_none()) {
        return Err(Failure::Unavailable(
            "暂时无法读取匹配的作品详情，可稍后重试。".into(),
        ));
    }
    let language_ids = entries
        .iter()
        .filter_map(|(_, _, entry)| entry.as_ref())
        .flat_map(|entry| item_ids(entry, "P364"))
        .filter(|id| known_language(id).is_none())
        .collect::<BTreeSet<_>>();
    let ids = language_ids.into_iter().take(50).collect::<Vec<_>>();
    // Missing ancillary language-code lookups must not discard the main search results.
    let related = tokio::time::timeout(
        Duration::from_millis(1500),
        entities(client, &ids, "claims", None),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();

    let incomplete = entries.iter().any(|(_, _, entry)| entry.is_none());
    let candidates = entries
        .into_iter()
        .filter_map(|(_, id, entry)| {
            let entity = entry.as_ref()?;
            if !matches_kind(entity, kind) {
                return None;
            }
            Some(Candidate {
                provider: MediaProvider::Wikidata,
                id,
                title: title(entity)?.to_owned(),
                match_titles: match_titles(entity),
                year: release_date(entity).and_then(year),
                original_languages: languages(entity, &related),
                cover_url: cover(entity, "320"),
                tvmaze_id: tvmaze_id(entity),
                tmdb_id: tmdb_id(entity, kind),
                bangumi_id: bangumi_id(entity),
            })
        })
        .collect();
    let notes = if incomplete {
        vec!["Wikidata：部分候选暂不可用。".into()]
    } else {
        Vec::new()
    };
    Ok(SearchResult { candidates, notes })
}

pub(super) async fn entity(
    client: &Client,
    cache: &EntityCache,
    id: &str,
) -> Result<Arc<Value>, Failure> {
    if let Some(entity) = cache.get(id) {
        return Ok(entity);
    }
    let url = Url::parse(&format!(
        "https://www.wikidata.org/wiki/Special:EntityData/{id}.json"
    ))
    .unwrap();
    let mut data = get_json(client, &url).await?;
    let entity = data["entities"][id].take();
    if entity.is_null() || entity.get("missing").is_some() {
        return Err(Failure::NotFound);
    }
    let entity = Arc::new(entity);
    cache.insert(id, Arc::clone(&entity));
    Ok(entity)
}

pub(super) fn matches_kind(entity: &Value, kind: MediaKind) -> bool {
    item_ids(entity, "P31").any(|id| classes(kind).contains(&id))
}

pub(super) fn tvmaze_id(entity: &Value) -> Option<u64> {
    first_string(entity, "P8600")?.parse().ok()
}

pub(super) fn bangumi_id(entity: &Value) -> Option<u32> {
    let mut ids = claim_values(entity, "P5732")
        .filter_map(Value::as_str)
        .filter_map(bangumi::id);
    let id = ids.next()?;
    ids.all(|other| other == id).then_some(id)
}

fn bangumi_claim(id: u32) -> String {
    format!("haswbstatement:P5732={id}")
}

pub(super) fn bangumi_search_url(id: u32) -> Url {
    let mut url = Url::parse("https://www.wikidata.org/wiki/Special:Search").unwrap();
    url.query_pairs_mut()
        .append_pair("search", &bangumi_claim(id));
    url
}

pub(super) async fn by_bangumi_id(
    client: &Client,
    cache: &EntityCache,
    kind: MediaKind,
    id: u32,
) -> Result<Option<(String, Arc<Value>)>, Failure> {
    let data = api(
        client,
        &[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", &bangumi_claim(id)),
            ("srnamespace", "0"),
            ("srprop", ""),
            ("srlimit", "2"),
        ],
    )
    .await?;
    let hits = data["query"]["search"]
        .as_array()
        .ok_or_else(Failure::format_changed)?;
    let [hit] = hits.as_slice() else {
        return Ok(None);
    };
    let Some(qid) = hit["title"].as_str().filter(|id| valid_id(id)) else {
        return Err(Failure::format_changed());
    };
    let entry = entity(client, cache, qid).await?;
    Ok(
        (bangumi_id(&entry) == Some(id) && matches_kind(&entry, kind))
            .then(|| (qid.to_owned(), entry)),
    )
}

pub(super) fn tmdb_id(entity: &Value, kind: MediaKind) -> Option<u32> {
    let property = match kind {
        MediaKind::Movie => "P4947",
        MediaKind::Series => "P4983",
        MediaKind::Av => return None,
    };
    super::tmdb::id(first_string(entity, property)?)
}

pub(super) async fn details(client: &Client, id: &str, entity: Arc<Value>) -> Source {
    let page = Url::parse(&format!("https://www.wikidata.org/wiki/{id}")).unwrap();
    // Keep references to every linked item in the raw record even if its label is missing.
    let mut seen = HashSet::new();
    let ids = ["P364", "P495", "P136", "P57", "P161", "P58", "P162", "P272"]
        .into_iter()
        .flat_map(|property| item_ids(&entity, property))
        .filter(|id| seen.insert(*id))
        .take(50)
        .collect::<Vec<_>>();
    let (language_ids, label_ids): (Vec<_>, Vec<_>) = ids
        .into_iter()
        .partition(|id| item_ids(&entity, "P364").any(|language| language == *id));
    let language_ids = language_ids
        .into_iter()
        .filter(|id| known_language(id).is_none())
        .collect::<Vec<_>>();
    let label_timeout = Duration::from_secs(4);
    let (language_items, labels) = tokio::join!(
        tokio::time::timeout(
            label_timeout,
            entities(
                client,
                &language_ids,
                "claims|labels",
                Some("zh-hans|zh|en|mul")
            )
        ),
        tokio::time::timeout(
            label_timeout,
            entities(client, &label_ids, "labels", Some("zh-hans|zh|en|mul"))
        )
    );
    let language_items = language_items.ok().and_then(Result::ok);
    let labels = labels.ok().and_then(Result::ok);
    let partial = language_items.is_none() || labels.is_none();
    let language_items = language_items.unwrap_or_default();
    let labels = labels.unwrap_or_default();

    let names = |key: &str| {
        json!(
            item_ids(&entity, key)
                .filter_map(|id| label(&labels[id], "labels"))
                .collect::<Vec<_>>()
        )
    };
    let originals = languages(&entity, &language_items);
    let original_title = claim_values(&entity, "P1476")
        .find(|value| {
            value["language"].as_str().is_some_and(|lang| {
                originals.iter().any(|code| {
                    code == lang
                        || (lang.starts_with("zh") && matches!(code.as_str(), "zh" | "cmn" | "yue"))
                })
            })
        })
        .and_then(|value| value["text"].as_str());
    let fields = field_map([
        ("title", json!(title(&entity))),
        ("title_original", json!(original_title)),
        ("release_date", json!(release_date(&entity))),
        (
            "end_date",
            json!(claim_values(&entity, "P582").find_map(date)),
        ),
        (
            "runtime_minutes",
            json!(claim_values(&entity, "P2047").find_map(runtime)),
        ),
        ("original_languages", json!(originals)),
        ("description", json!(label(&entity, "descriptions"))),
        ("cast", names("P161")),
        ("directors", names("P57")),
        ("writers", names("P58")),
        ("producers", names("P162")),
        ("studio", names("P272")),
        ("countries", names("P495")),
        ("genres", names("P136")),
        (
            "season_count",
            json!(claim_values(&entity, "P2437").find_map(quantity)),
        ),
        (
            "episode_count",
            json!(claim_values(&entity, "P1113").find_map(quantity)),
        ),
        ("imdb_id", json!(first_string(&entity, "P345"))),
        (
            "bangumi_id",
            json!(bangumi_id(&entity).map(|id| id.to_string())),
        ),
        ("official_url", json!(first_string(&entity, "P856"))),
        ("cover_url", json!(cover(&entity, "640"))),
    ]);
    let mut source = Source::found(
        NAME,
        &page,
        fields,
        json!({
            "entity": entity.as_ref(),
            "related_labels": labels,
            "language_entities": language_items,
        }),
    );
    if partial {
        source.status = "partial";
        source.note = Some("已取得作品条目，部分关联名称暂不可用。".into());
    }
    source
}

fn claim_values<'a>(entry: &'a Value, property: &str) -> impl Iterator<Item = &'a Value> {
    entry["claims"][property]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|claim| claim["rank"] != "deprecated")
        .filter_map(|claim| claim["mainsnak"]["datavalue"].get("value"))
}

fn item_ids<'a>(entry: &'a Value, property: &str) -> impl Iterator<Item = &'a str> {
    claim_values(entry, property)
        .filter_map(|value| value["id"].as_str())
        .filter(|id| valid_id(id))
}

fn first_string<'a>(entity: &'a Value, property: &str) -> Option<&'a str> {
    claim_values(entity, property).find_map(Value::as_str)
}

fn label<'a>(entity: &'a Value, key: &str) -> Option<&'a str> {
    let localized = &entity[key];
    ["zh-hans", "zh", "en", "mul"]
        .iter()
        .find_map(|language| localized[language]["value"].as_str())
        .or_else(|| {
            localized
                .as_object()?
                .values()
                .find_map(|entry| entry["value"].as_str())
        })
}

fn title(entity: &Value) -> Option<&str> {
    label(entity, "labels")
        .or_else(|| claim_values(entity, "P1476").find_map(|value| value["text"].as_str()))
}

fn match_titles(entity: &Value) -> Vec<String> {
    entity["labels"]
        .as_object()
        .into_iter()
        .flat_map(|labels| labels.values())
        .chain(
            entity["aliases"]
                .as_object()
                .into_iter()
                .flat_map(|aliases| aliases.values())
                .filter_map(Value::as_array)
                .flatten(),
        )
        .filter_map(|label| label["value"].as_str())
        .chain(claim_values(entity, "P1476").filter_map(|title| title["text"].as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn date(value: &Value) -> Option<&str> {
    let time = value["time"].as_str()?.strip_prefix('+')?;
    match value["precision"].as_u64()? {
        11.. => time.get(..10),
        10 => time.get(..7),
        9 => time.get(..4),
        _ => None,
    }
}

fn release_date(entity: &Value) -> Option<&str> {
    claim_values(entity, "P580")
        .find_map(date)
        .or_else(|| claim_values(entity, "P577").filter_map(date).min())
}

fn quantity(value: &Value) -> Option<f64> {
    value["amount"].as_str()?.parse().ok()
}

fn runtime(value: &Value) -> Option<f64> {
    let amount = quantity(value)?;
    match value["unit"].as_str()?.rsplit('/').next()? {
        "Q7727" => Some(amount),
        "Q11574" => Some(amount / 60.0),
        "Q25235" => Some(amount * 60.0),
        _ => None,
    }
}

fn cover(entity: &Value, width: &str) -> Option<String> {
    let name = first_string(entity, "P18")?;
    let mut url = Url::parse("https://commons.wikimedia.org/wiki/Special:FilePath/").unwrap();
    url.path_segments_mut().ok()?.pop_if_empty().push(name);
    url.query_pairs_mut().append_pair("width", width);
    Some(url.into())
}

fn known_language(id: &str) -> Option<&'static str> {
    Some(match id {
        "Q1860" | "Q7976" => "en",
        "Q188" => "de",
        "Q150" => "fr",
        "Q1321" => "es",
        "Q652" => "it",
        "Q7737" => "ru",
        "Q5146" => "pt",
        "Q5287" => "ja",
        "Q9176" => "ko",
        "Q9192" | "Q24841726" => "cmn",
        "Q9186" => "yue",
        "Q7850" => "zh",
        "Q21686005" => "zxx",
        _ => return None,
    })
}

fn languages(entity: &Value, related: &Value) -> Vec<String> {
    item_ids(entity, "P364")
        .filter_map(|id| {
            known_language(id).or_else(|| {
                first_string(&related[id], "P218").or_else(|| first_string(&related[id], "P220"))
            })
        })
        .filter(|code| {
            matches!(code.len(), 2 | 3)
                && code.bytes().all(|byte| byte.is_ascii_lowercase())
                && *code != "und"
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detail_can_reuse_the_entity_fetched_during_search() {
        let client = Client::new();
        let cache = EntityCache::default();
        let entity_data = Arc::new(json!({"id": "Q155653", "claims": {}}));
        cache.insert("Q155653", Arc::clone(&entity_data));
        let entry = entity(&client, &cache, "Q155653").await.ok().unwrap();
        assert!(Arc::ptr_eq(&entity_data, &entry));
    }

    #[test]
    fn rejects_ambiguous_bangumi_links() {
        let statement = |id: &str, rank: &str| {
            json!({"rank": rank, "mainsnak": {"datavalue": {"value": id}}})
        };
        let mut entity = json!({"claims": {"P5732": [
            statement("253", "normal"),
            statement("253", "preferred"),
            statement("160209", "deprecated")
        ]}});
        assert_eq!(bangumi_id(&entity), Some(253));
        entity["claims"]["P5732"][2]["rank"] = json!("normal");
        assert_eq!(bangumi_id(&entity), None);
    }

    #[test]
    fn preserves_multiple_original_languages_and_ignores_deprecated_statements() {
        let language = |id: &str, rank: &str| {
            json!({
                "rank": rank, "mainsnak": {"datavalue": {"value": {"id": id}}}
            })
        };
        let entity = json!({"claims": {"P364": [
            language("Q1860","normal"), language("Q9186","normal"), language("Q9176","deprecated")
        ]}});
        assert_eq!(languages(&entity, &Value::Null), ["en", "yue"]);
    }
}
