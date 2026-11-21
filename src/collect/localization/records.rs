use serde_json::{Value, json};

use super::{LanguageSettings, Texts, Translations, language_base, normalize_locale, text};
use crate::{Collector, LookupResponse};

fn copy_text(fields: &mut Texts, key: &str, value: &Value) {
    if let Some(value) = text(value) {
        fields.insert(key.to_owned(), value.to_owned());
    }
}

pub(crate) fn emby_texts(document: &Value) -> Texts {
    let mut texts = Texts::new();
    copy_text(&mut texts, "title", &document["Name"]);
    copy_text(&mut texts, "description", &document["Overview"]);
    texts
}

pub(crate) fn synced_cache(previous: &Value, texts: &Texts) -> Translations {
    let old = emby_texts(previous);
    let mut cached: Translations =
        serde_json::from_value(previous["_collect_metadata"]["localized_metadata"].clone())
            .unwrap_or_default();
    for fields in cached.values_mut() {
        fields.retain(|key, _| {
            texts
                .get(key)
                .is_some_and(|value| old.get(key) == Some(value))
        });
    }
    cached
}

fn catalog_text<'a>(
    translations: &'a [Value],
    key: &str,
    preferred: impl Fn(&Value) -> bool,
) -> Option<&'a str> {
    translations
        .iter()
        .filter(|entry| preferred(entry))
        .find_map(|entry| text(&entry["data"][key]))
}

fn fill_missing_tmdb_text(record: &mut Value) {
    let is_series = record["media_type"] == "series";
    if let Some(source) = record["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|source| source["provider"] == "TMDB")
    {
        let data = &source["data"];
        let Some(translations) = data["translations"]["translations"].as_array() else {
            return;
        };
        let mut fallback = Texts::new();
        for (field, key) in [
            ("title", if is_series { "name" } else { "title" }),
            ("description", "overview"),
            ("tagline", "tagline"),
        ] {
            if text(&record["fields"][field]).is_some() {
                continue;
            }
            if let Some(value) = catalog_text(translations, key, |entry| {
                entry["iso_639_1"] == data["original_language"]
            })
            .or_else(|| catalog_text(translations, key, |entry| entry["iso_639_1"] == "en"))
            .or_else(|| catalog_text(translations, key, |_| true))
            {
                fallback.insert(field.to_owned(), value.to_owned());
            }
        }
        for (field, value) in fallback {
            record["fields"][&field] = json!(value);
            record["field_sources"][&field] = json!("TMDB");
        }
    }
}

fn official_catalog(record: &Value, targets: &[String]) -> (Translations, Translations) {
    let mut official = Translations::new();
    let mut origins = Translations::new();
    let mut insert = |target: &str, field: &str, value: &str, origin: &str| {
        let fields = official.entry(target.to_owned()).or_default();
        if !fields.contains_key(field) {
            fields.insert(field.to_owned(), value.to_owned());
            origins
                .entry(target.to_owned())
                .or_default()
                .insert(field.to_owned(), origin.to_owned());
        }
    };
    for source in record["sources"].as_array().into_iter().flatten() {
        if source["provider"] == "TMDB" {
            let translations = source["data"]["translations"]["translations"].as_array();
            for target in targets {
                let mut entries = translations
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        let lang = text(&entry["iso_639_1"])?;
                        let region = text(&entry["iso_3166_1"]);
                        let tag =
                            normalize_locale(&region.map_or_else(
                                || lang.to_owned(),
                                |region| format!("{lang}-{region}"),
                            ))?;
                        let base = language_base(target);
                        let rank = if &tag == target {
                            0
                        } else if base != "zh" && language_base(&tag) == base {
                            1
                        } else {
                            return None;
                        };
                        Some((rank, &entry["data"]))
                    })
                    .collect::<Vec<_>>();
                entries.sort_by_key(|(rank, _)| *rank);
                for (field, key) in [
                    (
                        "title",
                        if record["media_type"] == "series" {
                            "name"
                        } else {
                            "title"
                        },
                    ),
                    ("description", "overview"),
                    ("tagline", "tagline"),
                ] {
                    if let Some(value) = entries.iter().find_map(|(_, data)| text(&data[key])) {
                        insert(target, field, value, "TMDB");
                    }
                }
            }
        }
    }
    if let Some(bangumi) = record["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|source| source["provider"] == "Bangumi")
    {
        let subject = &bangumi["data"]["subject"];
        let fields = &record["fields"];
        for target in targets {
            let title_key = if target == "zh-CN" {
                "name_cn"
            } else if fields["original_languages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|language| language == language_base(target) && language != "zh")
            {
                "name"
            } else {
                continue;
            };
            if record["field_sources"]["title"] != "Emby"
                && let Some(title) = text(&subject[title_key])
            {
                insert(target, "title", title, "Bangumi");
            }
            if record["field_sources"]["episodes"] == "Bangumi" {
                let key = if title_key == "name_cn" {
                    "title_cn"
                } else {
                    "title_original"
                };
                for (index, episode) in fields["episodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(title) = text(&episode[key]) {
                        insert(target, &format!("episodes/{index}/title"), title, "Bangumi");
                    }
                }
            }
        }
    }
    if let Some(wiki) = record["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|source| source["provider"] == "Wikidata")
    {
        let entity = &wiki["data"]["entity"];
        for target in targets {
            let locale = target.to_ascii_lowercase();
            let base = language_base(&locale);
            let codes: &[&str] = match locale.as_str() {
                "zh-cn" => &["zh-cn", "zh-hans"],
                "zh-tw" => &["zh-tw", "zh-hant"],
                "zh-hk" => &["zh-hk", "zh-hant"],
                _ => &[&locale, base],
            };
            for (field, key) in [("title", "labels"), ("description", "descriptions")] {
                if let Some(value) = codes
                    .iter()
                    .find_map(|code| text(&entity[key][code]["value"]))
                {
                    insert(target, field, value, "Wikidata");
                }
            }
        }
    }
    let fields = &record["fields"];
    if record["media_type"] == "av" {
        for target in targets {
            if let Some(key) = match language_base(target) {
                "ja" => Some("title_ja"),
                "en" => Some("title_en"),
                _ => None,
            } && let Some(value) = text(&fields[key])
            {
                let source = text(&record["field_sources"][key]).unwrap_or("原始资料");
                insert(target, "title", value, source);
            }
        }
    }
    (official, origins)
}

impl Collector {
    pub(in crate::collect) async fn localize_lookup(
        &self,
        mut response: LookupResponse,
        targets: Vec<String>,
    ) -> LookupResponse {
        if !response.has_fields() {
            return response;
        }
        fill_missing_tmdb_text(&mut response.record);
        let fields = &response.record["fields"];
        let mut texts = Texts::new();
        for key in ["title", "description", "tagline"] {
            copy_text(&mut texts, key, &fields[key]);
        }
        if !texts.contains_key("title") {
            for key in ["title_ja", "title_original", "title_en", "title_site"] {
                if let Some(title) = text(&fields[key]) {
                    texts.insert("title".into(), title.to_owned());
                    break;
                }
            }
        }
        for (index, episode) in fields["episodes"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for field in ["title", "description"] {
                copy_text(
                    &mut texts,
                    &format!("episodes/{index}/{field}"),
                    &episode[field],
                );
            }
        }
        let (official, origins) = official_catalog(&response.record, &targets);
        let variants = self
            .languages
            .localize(
                &self.client,
                &targets,
                &texts,
                &official,
                &Translations::new(),
            )
            .await;
        if let Some(fields) = variants.get(&targets[0]) {
            for (key, value) in fields {
                if key.starts_with("episodes/") {
                    if let Some(field) = response.record.pointer_mut(&format!("/fields/{key}")) {
                        *field = json!(value);
                    }
                } else {
                    if let Some(source) =
                        origins.get(&targets[0]).and_then(|fields| fields.get(key))
                    {
                        response.record["field_sources"][key] = json!(source);
                    } else if texts.get(key) != Some(value) {
                        response.record["field_sources"][key] = json!("元数据翻译");
                    }
                    response.record["fields"][key] = json!(value);
                }
            }
        }
        response.record["metadata_languages"] = json!(targets);
        response.record["localized_metadata"] = json!(variants);
        response
    }
}

pub(crate) async fn sync_document(
    settings: &LanguageSettings,
    client: &reqwest::Client,
    document: &Value,
    previous: &Value,
    preference: Option<&str>,
) -> Value {
    let targets = settings.targets(preference);
    let texts = emby_texts(document);
    let mut variants = settings
        .localize(
            client,
            &targets,
            &texts,
            &Translations::new(),
            &synced_cache(previous, &texts),
        )
        .await;
    let previous_metadata = &previous["_collect_metadata"];
    let supplemental_overview = if text(&document["Overview"]).is_none() {
        text(&previous_metadata["overview_source"]).zip(text(
            &previous_metadata["localized_metadata"]["zh-CN"]["description"],
        ))
    } else {
        None
    };
    if let Some((_, overview)) = supplemental_overview {
        variants
            .entry("zh-CN".into())
            .or_default()
            .insert("description".into(), overview.into());
    }
    let mut document = document.clone();
    document["_collect_metadata"] =
        json!({"metadata_languages": targets, "localized_metadata": variants});
    if let Some((source, _)) = supplemental_overview {
        document["_collect_metadata"]["overview_source"] = json!(source);
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_blank_catalog_translation_and_preserves_exact_locale() {
        let record = json!({"media_type":"movie", "sources":[{"provider":"TMDB","data":{"translations":{"translations":[
            {"iso_639_1":"zh","iso_3166_1":"TW","data":{"title":"臺灣片名","overview":"臺灣簡介"}},
            {"iso_639_1":"zh","iso_3166_1":"CN","data":{"title":"大陆片名","overview":" "}},
            {"iso_639_1":"en","iso_3166_1":"US","data":{"title":"English Title","overview":"English Overview"}}
        ]}}}]});
        let (official, origins) =
            official_catalog(&record, &["zh-CN".into(), "zh-TW".into(), "en-GB".into()]);
        assert_eq!(origins["en-GB"]["title"], "TMDB");
        assert_eq!(official["zh-CN"]["title"], "大陆片名");
        assert!(!official["zh-CN"].contains_key("description"));
        assert_eq!(official["zh-TW"]["description"], "臺灣簡介");
        assert_eq!(official["en-GB"]["title"], "English Title");
    }

    #[tokio::test]
    async fn sync_prefers_emby_and_keeps_raw_text() {
        let settings = LanguageSettings {
            path: None,
            languages: std::sync::RwLock::new(vec!["zh-CN".into(), "ja".into()]),
            translator: None,
        };
        let client = reqwest::Client::new();
        let raw = json!({"Id":"12","Name":"原始标题","Overview":"原始简介"});
        let synced = sync_document(&settings, &client, &raw, &Value::Null, Some("zh-hant")).await;
        assert_eq!(
            synced["_collect_metadata"]["metadata_languages"],
            json!(["zh-TW"])
        );
        let fallback = sync_document(&settings, &client, &raw, &Value::Null, None).await;
        assert_eq!(
            fallback["_collect_metadata"]["metadata_languages"],
            json!(["zh-CN", "ja"])
        );
        assert_eq!(synced["Name"], raw["Name"]);
        assert_eq!(synced["Overview"], raw["Overview"]);
    }

    #[tokio::test]
    async fn sync_retains_sourced_overview_until_emby_provides_one() {
        let settings = LanguageSettings {
            path: None,
            languages: std::sync::RwLock::new(vec!["zh-CN".into()]),
            translator: None,
        };
        let client = reqwest::Client::new();
        let previous = json!({"Id":"12", "_collect_metadata":{
            "overview_source":"https://example.com/season/1",
            "localized_metadata":{"zh-CN":{"description":"补充简介"}}
        }});
        let raw = json!({"Id":"12","Name":"第一季"});
        let synced = sync_document(&settings, &client, &raw, &previous, None).await;
        assert_eq!(
            synced["_collect_metadata"]["overview_source"],
            previous["_collect_metadata"]["overview_source"]
        );
        assert_eq!(
            synced["_collect_metadata"]["localized_metadata"]["zh-CN"]["description"],
            "补充简介"
        );
        assert!(synced.get("Overview").is_none());

        let updated = json!({"Id":"12","Name":"第一季","Overview":"Emby 的新简介"});
        let synced = sync_document(&settings, &client, &updated, &previous, None).await;
        assert_eq!(synced["Overview"], updated["Overview"]);
        assert!(synced["_collect_metadata"].get("overview_source").is_none());
        assert!(
            synced["_collect_metadata"]["localized_metadata"]
                .get("zh-CN")
                .is_none()
        );
    }

    #[test]
    fn sync_reuses_only_unchanged_inputs() {
        let previous = json!({"Name":"Old","Overview":"Same", "_collect_metadata":{"localized_metadata":{
            "ja":{"title":"古い","description":"同じ"}
        }}});
        let fields = synced_cache(
            &previous,
            &emby_texts(&json!({"Name":"New","Overview":"Same"})),
        );
        assert!(!fields["ja"].contains_key("title"));
        assert_eq!(fields["ja"]["description"], "同じ");
    }
}
