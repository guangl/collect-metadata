#[cfg(feature = "web")]
mod automatic;

use std::collections::BTreeMap;

use crate::{MediaKind, providers::media};

use super::{Collector, EmbyEpisodeOverview, LookupResponse, Source, make_lookup};

impl Collector {
    pub(crate) async fn gather_emby(
        &self,
        emby: &media::emby::Emby,
        kind: MediaKind,
        query: &str,
        (id, mut home): (String, Source),
        candidate: Option<&crate::CandidateChoice>,
        prefer_emby: bool,
    ) -> Result<LookupResponse, String> {
        let targets = self.languages.targets(None);
        let is_episode = query.starts_with(media::emby::EPISODE_QUERY_PREFIX);
        let sources = if let Some(candidate) = candidate {
            let mut external = if is_episode {
                let tmdb = self.tmdb.provider().ok_or("请先配置 TMDB API 凭据。")?;
                let series = candidate
                    .id
                    .parse::<u32>()
                    .map_err(|_| "剧集 TMDB ID 无效。")?;
                let episode = tmdb
                    .emby_episode_with_series(
                        &self.client,
                        emby,
                        &home.data["item"],
                        &targets[0],
                        Some(series),
                    )
                    .await
                    .ok_or("此分集缺少单集季集号。")?;
                vec![episode]
            } else {
                self.media_sources(kind, candidate.provider, &candidate.id, &targets[0])
                    .await?
            };
            if external.iter().all(|source| source.fields.is_empty()) {
                return Err(external
                    .into_iter()
                    .find_map(|source| source.note)
                    .unwrap_or_else(|| "来源未返回可用资料。".into()));
            }
            // Keep Emby episode IDs and file associations while applying known titles and stills.
            if kind == MediaKind::Series && !is_episode {
                merge_emby_episodes(&mut home, &mut external, prefer_emby);
            }
            if prefer_emby {
                external.insert(0, home);
            } else {
                external.push(home);
            }
            external
        } else {
            let mut sources = vec![home];
            if !is_episode
                && kind != MediaKind::Av
                && let Some(bangumi_id) = sources[0]
                    .fields
                    .get("bangumi_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(media::bangumi::id)
            {
                let enrichment = self
                    .bangumi
                    .provider()
                    .details(&self.client, kind, bangumi_id)
                    .await;
                if !enrichment.fields.is_empty() {
                    sources.push(enrichment);
                }
            }
            if is_episode {
                let episode = &sources[0].data["item"];
                let needs_chinese = targets.iter().any(|target| target == "zh-CN")
                    && !episode["Overview"]
                        .as_str()
                        .is_some_and(media::bangumi::chinese_overview);
                let (tmdb, bangumi) = tokio::join!(
                    async {
                        self.tmdb
                            .provider()?
                            .emby_episode(&self.client, emby, episode, &targets[0])
                            .await
                    },
                    async {
                        if needs_chinese {
                            self.bangumi
                                .provider()
                                .emby_episode_overview(&self.client, emby, episode)
                                .await
                        } else {
                            None
                        }
                    },
                );
                sources.extend(tmdb);
                sources.extend(bangumi);
            }
            sources
        };
        let mut lookup = make_lookup(kind, query, Some(&format!("Emby-{id}")), sources)?;
        if is_episode {
            lookup.record["scope"] = serde_json::json!("episode");
        }
        if prefer_emby {
            lookup.record["fill_missing"] = serde_json::json!(true);
        }
        Ok(self.localize_lookup(lookup, targets).await)
    }

    pub async fn emby_episode_overview(&self, id: &str) -> Result<EmbyEpisodeOverview, String> {
        let _permit = self.permit()?;
        let emby = self
            .emby_settings()
            .and_then(|settings| settings.connection())
            .ok_or("请先配置 Emby 服务。")?;
        let query = format!("{}{id}", media::emby::EPISODE_QUERY_PREFIX);
        let home = emby
            .find(&self.client, MediaKind::Series, &query, None)
            .await
            .ok_or("无法读取此 Emby 分集。")?;
        let record = self
            .gather_emby(&emby, MediaKind::Series, &query, home, None, false)
            .await?
            .record;
        Ok(EmbyEpisodeOverview {
            overview: record["fields"]["description"].as_str().map(str::to_owned),
            source: record["field_sources"]["description"]
                .as_str()
                .map(str::to_owned),
        })
    }
}

fn merge_emby_episodes(home: &mut Source, external: &mut [Source], prefer_emby: bool) {
    let mut by_number = BTreeMap::new();
    for source in external {
        if let Some(episodes) = source.fields.remove("episodes") {
            for episode in episodes.as_array().into_iter().flatten() {
                if let Some(number) = episode["season"].as_u64().zip(episode["number"].as_u64()) {
                    by_number.entry(number).or_insert_with(|| episode.clone());
                }
            }
        }
        source.fields.remove("season_count");
    }
    if let Some(episodes) = home
        .fields
        .get_mut("episodes")
        .and_then(serde_json::Value::as_array_mut)
    {
        for episode in episodes {
            let number = episode["season"].as_u64().zip(episode["number"].as_u64());
            if let Some(source) = number.and_then(|number| by_number.get(&number)) {
                for key in [
                    "title",
                    "description",
                    "date",
                    "runtime_minutes",
                    "still_url",
                    "url",
                ] {
                    if prefer_emby
                        && !episode[key].is_null()
                        && episode[key]
                            .as_str()
                            .is_none_or(|text| !text.trim().is_empty())
                    {
                        continue;
                    }
                    if let Some(value) = source.get(key).filter(|value| {
                        !value.is_null()
                            && value.as_str().is_none_or(|text| !text.trim().is_empty())
                    }) {
                        episode[key] = value.clone();
                    }
                }
            }
        }
    }
}
