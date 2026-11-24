use serde_json::{Value, json};

use crate::{MediaKind, MediaProvider, providers::media};

use super::super::{Collector, LookupResponse, Source, make_lookup};

impl Collector {
    pub(crate) async fn verified_episode_people(
        &self,
        emby: &media::emby::Emby,
        item: &Value,
    ) -> Vec<Value> {
        let Some(tmdb) = self.tmdb.provider() else {
            return Vec::new();
        };
        if !media::emby::has_provider_id(item, "Tmdb") {
            return Vec::new();
        }
        let Some(series) = emby
            .series_provider_id(&self.client, item, "Tmdb")
            .await
            .and_then(|id| id.parse::<u32>().ok().filter(|&id| id > 0))
        else {
            return Vec::new();
        };
        let targets = self.languages.targets(None);
        tmdb.verified_episode_credits(&self.client, item, series, &targets[0])
            .await
            .map_or_else(Vec::new, |credits| media::emby::tmdb_people(&credits))
    }

    pub(crate) async fn automatic_tmdb_search(
        &self,
        kind: MediaKind,
        title: &str,
    ) -> Vec<media::Candidate> {
        let Some(tmdb) = self.tmdb.provider() else {
            return Vec::new();
        };
        let targets = self.languages.targets(None);
        tmdb.automatic_candidates(&self.client, kind, title, &targets[0])
            .await
    }

    pub(crate) async fn automatic_backup(
        &self,
        kind: MediaKind,
        item: &Value,
        primary: MediaProvider,
    ) -> Result<crate::CandidateChoice, String> {
        let (provider, key) = match primary {
            MediaProvider::Tmdb => (MediaProvider::Bangumi, "Bangumi"),
            _ => (MediaProvider::Tmdb, "Tmdb"),
        };
        if let Some(id) = item["ProviderIds"][key]
            .as_str()
            .and_then(media::bangumi::id)
        {
            return Ok(crate::CandidateChoice {
                provider,
                id: id.to_string(),
            });
        }
        let year = item["ProductionYear"]
            .as_u64()
            .and_then(|y| u16::try_from(y).ok())
            .or_else(|| item["PremiereDate"].as_str()?.get(..4)?.parse().ok())
            .ok_or("无年份，无法安全匹配备用来源。")?;
        let mut matched = Vec::new();
        for name in ["OriginalTitle", "Name"]
            .into_iter()
            .filter_map(|key| item[key].as_str())
            .filter(|s| !s.trim().is_empty())
        {
            let list = match provider {
                MediaProvider::Bangumi => self
                    .bangumi
                    .provider()
                    .search(&self.client, kind, name)
                    .await
                    .map_err(|error| format!("Bangumi：{}", error.description()))?,
                _ => self.automatic_tmdb_search(kind, name).await,
            };
            if let Some(id) = automatic_exact(&list, name, year).and_then(|c| {
                if provider == MediaProvider::Bangumi {
                    c.bangumi_id
                } else {
                    c.tmdb_id
                }
            }) && !matched.contains(&id)
            {
                matched.push(id);
            }
        }
        match matched.as_slice() {
            [id] => Ok(crate::CandidateChoice {
                provider,
                id: id.to_string(),
            }),
            _ => Err(format!("{key} 没有唯一的同名同年份条目，未写入。")),
        }
    }

    pub(crate) async fn gather_automatic_work(
        &self,
        kind: MediaKind,
        id: String,
        query: &str,
        home: Source,
        candidate: &crate::CandidateChoice,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let language = &targets[0];
        let tmdb = self.tmdb.provider();
        let mut external = if candidate.provider == MediaProvider::Tmdb {
            let source = tmdb
                .as_deref()
                .ok_or("请先配置 TMDB API 凭据。")?
                .work_details(
                    &self.client,
                    kind,
                    candidate.id.parse().map_err(|_| "TMDB ID 无效。")?,
                    language,
                    false,
                )
                .await;
            vec![source]
        } else {
            self.media_sources(kind, candidate.provider, &candidate.id, language)
                .await?
        };
        if external.iter().all(|source| source.fields.is_empty()) {
            return Err(external
                .into_iter()
                .find_map(|source| source.note)
                .unwrap_or_else(|| "公开来源未返回可用资料。".into()));
        }
        let item = &home.data["item"];
        let name = external
            .iter()
            .find_map(|source| source.fields.get("title_original")?.as_str())
            .or_else(|| {
                item["OriginalTitle"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
            })
            .or_else(|| item["Name"].as_str().filter(|name| !name.trim().is_empty()));
        let year = external
            .iter()
            .find_map(|source| {
                source
                    .fields
                    .get("release_date")?
                    .as_str()?
                    .get(..4)?
                    .parse::<u16>()
                    .ok()
            })
            .or_else(|| item["ProductionYear"].as_u64()?.try_into().ok());
        if let Some((name, year)) = name.zip(year) {
            let name = name.to_owned();
            let has = |key: &str, sources: &[Source]| {
                home.fields.contains_key(key)
                    || sources.iter().any(|source| source.fields.contains_key(key))
            };
            if !has("tmdb_id", &external)
                && let Some(tmdb) = tmdb.as_deref()
            {
                let list = tmdb
                    .automatic_candidates(&self.client, kind, &name, language)
                    .await;
                if let Some(id) = automatic_exact(&list, &name, year).and_then(|hit| hit.tmdb_id) {
                    external.push(
                        tmdb.work_details(&self.client, kind, id, language, false)
                            .await,
                    );
                }
            }
            if !has("bangumi_id", &external) {
                let bangumi = self.bangumi.provider();
                let list = bangumi
                    .search(&self.client, kind, &name)
                    .await
                    .map_err(|error| format!("Bangumi：{}", error.description()))?;
                if let Some(id) = automatic_exact(&list, &name, year).and_then(|hit| hit.bangumi_id)
                {
                    let source = bangumi.details(&self.client, kind, id).await;
                    if source.fields.is_empty() {
                        return Err(source
                            .note
                            .unwrap_or_else(|| "Bangumi 未返回作品资料。".into()));
                    }
                    external.push(source);
                }
            }
        }
        let mut sources = vec![home];
        sources.extend(external);
        let mut result = make_lookup(kind, query, Some(&format!("Emby-{id}")), sources)?;
        result.record["fill_missing"] = json!(true);
        Ok(self.localize_lookup(result, targets).await)
    }

    pub(crate) async fn existing_automatic_episode(
        &self,
        emby: &media::emby::Emby,
        home: Source,
        query: &str,
        id: &str,
        enrich: bool,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<LookupResponse, String> {
        let mut sources = vec![home];
        if enrich {
            let item = &sources[0].data["item"];
            let targets = self.languages.targets(None);
            let language = &targets[0];
            let series = match item["SeriesId"].as_str() {
                Some(id) => Some(emby.item(&self.client, id, "Series", "ProviderIds").await?),
                None => None,
            };
            let tmdb_id = series
                .as_ref()
                .and_then(|series| media::emby::provider_id(series, "Tmdb"))
                .and_then(|id| id.parse().ok());
            let tmdb = async {
                let Some(provider) = self.tmdb.provider() else {
                    return Ok(None);
                };
                let Some(series) = tmdb_id else {
                    return Ok(None);
                };
                provider
                    .matched_automatic_episode(&self.client, emby, item, language, series, timezone)
                    .await
            };
            let provider = self.bangumi.provider();
            let (tmdb, bangumi) = tokio::join!(
                tmdb,
                provider.existing_episode(&self.client, item, language)
            );
            let mut external = Vec::new();
            let mut failure = None;
            for result in [tmdb, bangumi] {
                match result {
                    Ok(Some(source)) => external.push(source),
                    Ok(None) => {}
                    Err(error) => failure = Some(error),
                }
            }
            if !media::emby::has_provider_id(item, "Bangumi")
                && item["ParentIndexNumber"] == 1
                && let Some(subject) = series
                    .as_ref()
                    .and_then(|series| media::emby::provider_id(series, "Bangumi"))
                    .and_then(media::bangumi::id)
            {
                let tmdb = external.first().filter(|source| source.provider == "TMDB");
                if let Some(source) = provider
                    .linked_episode(&self.client, subject, item, tmdb, language)
                    .await
                {
                    external.push(source);
                }
            }
            if external.is_empty()
                && let Some(error) = failure
            {
                return Err(error);
            }
            sources.extend(external);
        }
        let mut result = make_lookup(
            MediaKind::Series,
            query,
            Some(&format!("Emby-{id}")),
            sources,
        )?;
        result.record["scope"] = json!("episode");
        result.record["fill_missing"] = json!(true);
        Ok(result)
    }

    pub(crate) async fn gather_mapped_child(
        &self,
        emby: &media::emby::Emby,
        home: Source,
        query: &str,
        id: &str,
        subjects: &[u32],
        archived: &[Value],
        airdate: media::bangumi::MappedAirdate,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let item = &home.data["item"];
        let bangumi = self.bangumi.provider();
        let (scope, sources) = if item["Type"] == "Season" {
            let number = item["IndexNumber"].as_u64().ok_or("Emby 条目缺少季号。")?;
            let season = bangumi
                .mapped_season(&self.client, subjects, number)
                .await?;
            ("season", vec![home, season])
        } else {
            let bangumi = bangumi
                .mapped_episode(&self.client, subjects, archived, item, airdate, &targets[0])
                .await;
            let mut external = Vec::new();
            let tmdb = async {
                let Some(provider) = self.tmdb.provider() else {
                    return Ok(None);
                };
                let Some(series_id) = item["SeriesId"].as_str() else {
                    return Ok(None);
                };
                let series = emby
                    .item(&self.client, series_id, "Series", "ProviderIds")
                    .await?;
                let Some(series) =
                    media::emby::provider_id(&series, "Tmdb").and_then(|id| id.parse().ok())
                else {
                    return Ok(None);
                };
                provider
                    .matched_group_episode(&self.client, item, &targets[0], series, timezone)
                    .await
            }
            .await;
            match tmdb {
                Ok(Some(source)) => external.push(source),
                Ok(None) => {}
                Err(error) if bangumi.is_err() => return Err(error),
                Err(error) => eprintln!("TMDB 分组采集暂未完成：{error}"),
            }
            match bangumi {
                Ok(source) => external.push(source),
                Err(error) if external.is_empty() => return Err(error),
                Err(error) => eprintln!("Bangumi 季集匹配暂未完成：{error}"),
            }
            ("episode", automatic_episode_sources(home, external))
        };
        let mut result = make_lookup(
            MediaKind::Series,
            query,
            Some(&format!("Emby-{id}")),
            sources,
        )?;
        result.record["scope"] = json!(scope);
        result.record["fill_missing"] = json!(true);
        Ok(self.localize_lookup(result, targets).await)
    }

    pub(crate) async fn gather_automatic_child(
        &self,
        emby: &media::emby::Emby,
        home: Source,
        query: &str,
        candidate: &crate::CandidateChoice,
        series: &serde_json::Value,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<LookupResponse, String> {
        let item = &home.data["item"];
        let season = item["Type"] == "Season";
        let id = item["Id"].as_str().ok_or("Emby 条目缺少 ID。")?.to_owned();
        if !season
            && item["IndexNumberEnd"]
                .as_u64()
                .zip(item["IndexNumber"].as_u64())
                .is_some_and(|(end, first)| end > first)
        {
            return Err("多集文件需要确认季集匹配。".into());
        }
        if !season
            && matches!(
                candidate.provider,
                MediaProvider::Tmdb | MediaProvider::Bangumi
            )
        {
            return self
                .gather_automatic_episode(emby, home, query, &id, candidate, series, timezone)
                .await;
        }

        let targets = self.languages.targets(None);
        let external = if season {
            let primary = async {
                if candidate.provider != MediaProvider::Tmdb {
                    return Err("季资料需要剧集的 TMDB ID。".into());
                }
                let tmdb = self.tmdb.provider().ok_or("请先配置 TMDB API 凭据。")?;
                let series = candidate.id.parse().map_err(|_| "剧集 TMDB ID 无效。")?;
                tmdb.emby_season(&self.client, emby, series, item, &targets[0], timezone)
                    .await
            }
            .await;
            match primary {
                Ok(source) => source,
                Err(primary) => {
                    let related = async {
                        let subject = media::emby::provider_id(series, "Bangumi")
                            .or_else(|| {
                                (candidate.provider == MediaProvider::Bangumi)
                                    .then_some(candidate.id.as_str())
                            })
                            .and_then(media::bangumi::id)
                            .ok_or("剧集缺少已确认的 Bangumi ID。")?;
                        let number = item["IndexNumber"]
                            .as_u64()
                            .filter(|&number| number > 0)
                            .ok_or("Emby 条目缺少普通季号。")?;
                        let episodes = emby.season_episodes(&self.client, item).await?;
                        self.bangumi
                            .provider()
                            .related_season(&self.client, subject, number, &episodes, timezone)
                            .await
                    }
                    .await;
                    related.map_err(|error| format!("{primary}；{error}"))?
                }
            }
        } else {
            let detail = self
                .gather_media(MediaKind::Series, candidate.provider, &candidate.id)
                .await?;
            let season = item["ParentIndexNumber"]
                .as_u64()
                .ok_or("Emby 条目缺少季号。")?;
            let number = item["IndexNumber"].as_u64().ok_or("Emby 条目缺少集号。")?;
            let records = detail.record["fields"]["episodes"]
                .as_array()
                .ok_or("来源暂无分集资料。")?;
            let mut exact = records.iter().filter(|episode| {
                episode["number"].as_u64() == Some(number)
                    && episode["season"].as_u64() == Some(season)
            });
            let episode = exact
                .next()
                .filter(|_| exact.next().is_none())
                .ok_or("来源找不到唯一的季集。")?;
            let source = detail.record["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|source| source["status"] == "ok" || source["status"] == "partial")
                .ok_or("分集来源不可用。")?;
            let provider = match candidate.provider {
                MediaProvider::Tvmaze => "TVmaze",
                MediaProvider::Wikidata => "Wikidata",
                MediaProvider::Tmdb | MediaProvider::Bangumi => unreachable!(),
            };
            let url = source["url"]
                .as_str()
                .and_then(|value| reqwest::Url::parse(value).ok())
                .ok_or("分集来源地址无效。")?;
            let fields = crate::providers::field_map([
                ("title", episode["title"].clone()),
                ("description", episode["description"].clone()),
                ("release_date", episode["date"].clone()),
                ("still_url", episode["still_url"].clone()),
                ("series", detail.record["fields"]["title"].clone()),
            ]);
            Source::found(provider, &url, fields, Value::Null)
        };
        if external.fields.is_empty() {
            return Err("来源没有该季集的可用资料。".into());
        }
        let sources = if season {
            vec![home, external]
        } else {
            automatic_episode_sources(home, vec![external])
        };
        let mut result = make_lookup(
            MediaKind::Series,
            query,
            Some(&format!("Emby-{id}")),
            sources,
        )?;
        result.record["scope"] = json!(if season { "season" } else { "episode" });
        result.record["fill_missing"] = json!(true);
        Ok(self.localize_lookup(result, targets).await)
    }

    async fn gather_automatic_episode(
        &self,
        emby: &media::emby::Emby,
        home: Source,
        query: &str,
        id: &str,
        candidate: &crate::CandidateChoice,
        series: &serde_json::Value,
        timezone: &jiff::tz::TimeZone,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let language = &targets[0];
        let item = &home.data["item"];
        let (tmdb, bangumi) = if candidate.provider == MediaProvider::Tmdb {
            let id = candidate.id.parse().map_err(|_| "剧集 TMDB ID 无效。")?;
            let tmdb = match self.tmdb.provider() {
                Some(provider) if media::Tmdb::uses_episode_group(item, id) => {
                    provider
                        .matched_group_episode(&self.client, item, language, id, timezone)
                        .await?
                }
                Some(provider) => {
                    provider
                        .automatic_episode(&self.client, emby, item, language, id)
                        .await
                }
                None => None,
            };
            let bangumi = series["ProviderIds"]["Bangumi"]
                .as_str()
                .and_then(media::bangumi::id);
            (tmdb, bangumi)
        } else {
            (None, media::bangumi::id(&candidate.id))
        };
        let mut external = tmdb.into_iter().collect::<Vec<_>>();
        if item["ParentIndexNumber"] == 1
            && !external
                .iter()
                .any(|source| source.fields.contains_key("title"))
            && let Some(subject) = bangumi
        {
            let mut match_item = item.clone();
            if item["PremiereDate"].is_null()
                && let Some(date) = external.first().and_then(|s| s.fields.get("release_date"))
            {
                match_item["PremiereDate"] = date.clone();
            }
            if let Some(source) = self
                .bangumi
                .provider()
                .automatic_episode(&self.client, subject, &match_item, language)
                .await
            {
                external.push(source);
            }
        }
        if !media::episode_title::needs_title(item)
            && external.iter().all(|source| source.fields.is_empty())
        {
            return Err(external
                .into_iter()
                .find_map(|source| source.note)
                .unwrap_or_else(|| "分集来源暂无可用资料。".into()));
        }
        let sources = automatic_episode_sources(home, external);
        let mut result = make_lookup(
            MediaKind::Series,
            query,
            Some(&format!("Emby-{id}")),
            sources,
        )?;
        result.record["scope"] = json!("episode");
        result.record["fill_missing"] = json!(true);
        Ok(self.localize_lookup(result, targets).await)
    }
}

fn automatic_exact<'a>(
    candidates: &'a [media::Candidate],
    name: &str,
    year: u16,
) -> Option<&'a media::Candidate> {
    let name = media::emby::normalize(name);
    let mut exact = candidates.iter().filter(|candidate| {
        candidate.year == Some(year)
            && std::iter::once(&candidate.title)
                .chain(&candidate.match_titles)
                .any(|title| media::emby::normalize(title) == name)
    });
    exact.next().filter(|_| exact.next().is_none())
}

fn automatic_episode_sources(mut home: Source, mut external: Vec<Source>) -> Vec<Source> {
    use media::episode_title::{needs_title, usable};
    let item = &home.data["item"];
    for source in &mut external {
        if source
            .fields
            .get("title")
            .and_then(|title| usable(title, item))
            .is_none()
        {
            source.fields.remove("title");
        }
    }
    if needs_title(item) {
        if external
            .iter()
            .any(|source| source.fields.contains_key("title"))
        {
            home.fields.remove("title");
        } else if let Some(title) = usable(&item["ForcedSortName"], item) {
            home.fields.insert("title", json!(title));
        } else if !home.fields.contains_key("title") {
            // An old original-language title can belong to a different season.
            home.fields.remove("title_original");
        }
    }
    external.insert(0, home);
    external
}

#[cfg(test)]
#[test]
fn automatic_episode_sources_replace_only_placeholder_display_names() {
    use crate::collect::Fields;

    let item = json!({"Type":"Episode", "Name":"第 7 集", "ForcedSortName":"排序中的集名", "IndexNumber":7});
    let page = reqwest::Url::parse("https://www.themoviedb.org/").unwrap();
    let home = |item: &serde_json::Value| {
        Source::found(
            "Emby",
            &page,
            media::emby::episode_fields(item),
            json!({"item":item}),
        )
    };
    let tmdb = |name| {
        Source::found(
            "TMDB",
            &page,
            Fields::from([("title", json!(name))]),
            json!(null),
        )
    };
    let merged = |item: &serde_json::Value, sources| {
        make_lookup(
            MediaKind::Series,
            "emby-episode:7",
            None,
            automatic_episode_sources(home(item), sources),
        )
        .unwrap()
        .record
    };
    let preferred = merged(&item, vec![tmdb("重逢")]);
    assert_eq!(preferred["fields"]["title"], "重逢");
    assert_eq!(preferred["field_sources"]["title"], "TMDB");
    assert_eq!(
        merged(&item, vec![tmdb("Episode 7")])["fields"]["title"],
        "排序中的集名"
    );
    let mut item = item;
    item["Name"] = json!("人工维护的标题");
    assert_eq!(
        merged(&item, vec![tmdb("重逢")])["fields"]["title"],
        "人工维护的标题"
    );
}
impl Collector {
    pub(crate) async fn automatic_write_lock(&self, id: &str) -> tokio::sync::MutexGuard<'_, ()> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hash = DefaultHasher::new();
        id.hash(&mut hash);
        self.automatic_writes[hash.finish() as usize % self.automatic_writes.len()]
            .lock()
            .await
    }

    pub async fn repair_anime_metadata(
        &self,
        original: &serde_json::Value,
        field: &str,
        text: &str,
    ) -> Result<bool, String> {
        let id = original["Id"]
            .as_str()
            .filter(|id| crate::providers::media::emby::is_item_id(id))
            .ok_or("Emby 条目 ID 无效。")?;
        let _write = self.automatic_write_lock(id).await;
        let url = std::env::var("EMBY_AUTO_SCRAPE_URL").ok();
        let emby = self
            .emby_settings()
            .ok_or("Emby 设置不可用。")?
            .automatic_connection(url.as_deref())?;
        emby.repair_anime_metadata(&self.client, original, field, text)
            .await
    }
}
