use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use unicode_normalization::UnicodeNormalization;

use super::{CandidateChoice, Claim, Outcome};
use crate::{
    Collector, MediaKind, MediaProvider, SearchResult, diagnostics,
    output::{PreparedWriteback, WritebackJob, WritebackState, emby_source, prepare_writeback},
    providers::media::emby::{EPISODE_QUERY_PREFIX, Emby},
    storage::Store,
};

impl Collector {
    pub async fn run_batch_worker(self: Arc<Self>) {
        if self.storage_configured() {
            diagnostics::info("batch_worker_started", json!({"concurrency": 3}));
            tokio::join!(self.batch_loop(), self.batch_loop(), self.batch_loop());
        }
    }

    async fn batch_loop(&self) {
        let mut queue_unavailable = false;
        let mut last_error = None;
        let owner = self.output_settings().map(|settings| settings.owner());
        loop {
            let mut log_queue_error = |event, error: Option<&sqlx::Error>| {
                queue_unavailable = true;
                if last_error.is_none_or(|last: Instant| last.elapsed() >= Duration::from_secs(30))
                {
                    last_error = Some(Instant::now());
                    if let Some(error) = error {
                        diagnostics::postgres_error(event, error);
                    } else {
                        diagnostics::warn(event, json!({"retry_in_seconds": 2}));
                    }
                }
                Duration::from_secs(2)
            };
            let delay = match self.require_store().await {
                Ok(store) => match store
                    .claim_with_ai(
                        self.tmdb_configured(),
                        owner.as_deref(),
                        self.ai_configured(),
                    )
                    .await
                {
                    Ok(claim) => {
                        if queue_unavailable {
                            diagnostics::info("batch_queue_recovered", json!({}));
                            queue_unavailable = false;
                            last_error = None;
                        }
                        if let Some(claim) = claim {
                            diagnostics::with_batch_row(
                                claim.batch,
                                claim.id,
                                claim.attempt,
                                self.run_claim(store, &claim),
                            )
                            .await;
                            continue;
                        }
                        Duration::from_millis(600)
                    }
                    Err(error) => log_queue_error("batch_claim_failed", Some(&error)),
                },
                Err(_) => log_queue_error("batch_store_unavailable", None),
            };
            tokio::time::sleep(delay).await;
        }
    }

    async fn run_claim(&self, store: &Store, claim: &Claim) {
        let finished = if let Some((job, state)) = &claim.writeback {
            let work = self.prepare_claim_writeback(store, claim, job, state);
            match run_with_lease(store, claim, work).await {
                Some(output) => store.finish_writeback(claim, job, state, output).await,
                None => Ok(false),
            }
        } else {
            let work = self.process_claim(Some(store), claim);
            match run_with_lease(store, claim, work).await {
                Some(outcome) => store.finish(claim, outcome).await,
                None => Ok(false),
            }
        };
        match finished {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => diagnostics::postgres_error("batch_commit_failed", &error),
        }
        if let Err(error) = store.release_revoked(claim).await {
            diagnostics::postgres_error("batch_release_failed", &error);
        }
    }

    async fn prepare_claim_writeback(
        &self,
        store: &Store,
        claim: &Claim,
        job: &WritebackJob,
        state: &WritebackState,
    ) -> Option<PreparedWriteback> {
        let result = match store.batch_row_result(claim.batch, claim.id).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                diagnostics::error("batch_writeback_snapshot_missing", json!({}));
                return None;
            }
            Err(_) => {
                diagnostics::warn("batch_writeback_snapshot_read_failed", json!({}));
                return None;
            }
        };
        let Some(settings) = self.output_settings() else {
            diagnostics::error("batch_writeback_settings_missing", json!({}));
            return None;
        };
        let paths = settings.media_paths();
        let emby = self.emby_connection().ok();
        Some(
            prepare_writeback(
                &self.client,
                emby.as_deref(),
                &paths,
                job,
                &result.record,
                state,
            )
            .await,
        )
    }

    pub(super) async fn process_claim(
        &self,
        store: Option<&Store>,
        claim: &Claim,
    ) -> Option<Outcome> {
        let _permit = self.batch_permit().await;
        if claim.query.is_empty() {
            return Some(Outcome::failed("缺少作品名称或番号。".into()));
        }
        let from_folder = claim.folder_root.is_some();
        let (query, year) = if from_folder || claim.use_ai {
            super::folder::split_search_year(&claim.query)
        } else {
            (claim.query.as_str(), None)
        };
        let import = query.starts_with("emby:") || query.starts_with(EPISODE_QUERY_PREFIX);
        let emby = self.emby_settings().and_then(|settings| {
            if import {
                settings.connection()
            } else {
                settings.active()
            }
        });
        let key = match_key(claim, import, emby.as_deref());
        let remembered = if claim.kind != MediaKind::Av
            && claim.candidate.is_none()
            && let (Some(store), Some(key)) = (store, key.as_deref())
        {
            match store.remembered_match(claim.kind, key).await {
                Ok(value) => value,
                Err(error) => {
                    diagnostics::postgres_error("batch_match_read_failed", &error);
                    return Some(Outcome::failed(
                        "无法读取已保存的作品匹配，请检查 PostgreSQL 连接后重试。".into(),
                    ));
                }
            }
        } else {
            None
        };
        let prefer_emby = claim.fill_missing && claim.export_revision == 0;
        let mut candidate = claim.candidate.clone().or(remembered);
        let mut ai_matched = false;
        let home = if (from_folder || import)
            && let Some(emby) = emby.as_deref()
        {
            emby.find(&self.client, claim.kind, query, year).await
        } else {
            None
        };
        let result = if let Some((id, home)) = home {
            let item = &home.data["item"];
            let is_public_work = matches!(item["Type"].as_str(), Some("Movie" | "Series"))
                && !self.emby_path_excluded(&item["Path"]);
            let needs_metadata = claim.use_ai
                || !item["ImageTags"]["Primary"].is_string()
                || item["Overview"]
                    .as_str()
                    .is_none_or(|overview| overview.trim().is_empty());
            if candidate.is_none() && prefer_emby && is_public_work && needs_metadata {
                let title = item["OriginalTitle"]
                    .as_str()
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| item["Name"].as_str())
                    .unwrap_or(query);
                let year = item["ProductionYear"]
                    .as_u64()
                    .and_then(|year| year.try_into().ok());
                let provider_id = item["ProviderIds"]["Tmdb"].as_str().filter(|id| {
                    self.tmdb_configured() && id.parse::<u32>().is_ok_and(|id| id > 0)
                });
                let search_query =
                    provider_id.map_or_else(|| title.to_owned(), |id| format!("tmdb:{id}"));
                candidate = match self
                    .search_match(claim.kind, &search_query, year, claim.use_ai)
                    .await
                {
                    Ok((found, from_ai)) => {
                        ai_matched = from_ai;
                        Some(found)
                    }
                    Err(outcome) => return Some(*outcome),
                };
                if let Some(store) = store
                    && !mark_collecting(store, claim, candidate.as_ref()?).await
                {
                    return None;
                }
            }
            self.gather_emby(
                emby.as_deref()?,
                claim.kind,
                query,
                (id, home),
                candidate.as_ref(),
                prefer_emby,
            )
            .await
        } else if import {
            return Some(Outcome::failed(
                "无法读取该 Emby 条目，请检查媒体库与连接设置。".into(),
            ));
        } else if claim.kind == MediaKind::Av {
            self.gather_av(&claim.query).await
        } else {
            let found = if let Some(candidate) = &candidate {
                candidate.clone()
            } else {
                let (found, from_ai) = match self
                    .search_match(claim.kind, query, year, claim.use_ai)
                    .await
                {
                    Ok(found) => found,
                    Err(outcome) => return Some(*outcome),
                };
                if let Some(store) = store
                    && !mark_collecting(store, claim, &found).await
                {
                    return None;
                }
                ai_matched = from_ai;
                found
            };
            self.gather_media(claim.kind, found.provider, &found.id)
                .await
        };
        let result = result.map(|mut result| {
            if ai_matched {
                result.record["match_method"] = serde_json::json!("ai");
            }
            result
        });

        let mut outcome = match result {
            Ok(result) => Outcome::collected(result),
            Err(error) => Outcome::failed(error),
        };
        if let Some(result) = outcome.result.as_ref().filter(|result| result.has_fields()) {
            if claim.remember_match {
                outcome.remembered_match = key.zip(candidate);
            }
            if (store.is_none() || emby_source(&result.record).is_none())
                && let Some(plan) = claim
                    .export_plan
                    .as_ref()
                    .filter(|plan| plan.directory.is_some())
            {
                outcome.output = Some(
                    crate::output::prepare(
                        &self.client,
                        emby.as_deref(),
                        plan,
                        claim.batch,
                        (claim.id, claim.export_revision),
                        result,
                        &claim.source_files,
                    )
                    .await,
                );
            }
        }
        Some(outcome)
    }

    pub(super) async fn search_match(
        &self,
        kind: MediaKind,
        query: &str,
        year: Option<u16>,
        use_ai: bool,
    ) -> Result<(CandidateChoice, bool), Box<Outcome>> {
        let mut search = self
            .gather_candidates(kind, query)
            .await
            .map_err(|error| Box::new(Outcome::failed(error)))?;
        if let Some(candidate) = unique_candidate(query, year, &search) {
            return Ok((candidate, false));
        }
        if use_ai
            && crate::providers::media::tmdb_id_query(query).is_none()
            && !query
                .strip_prefix(['Q', 'q'])
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        {
            let mut suggestion = self
                .suggest_ai_match(kind, query, year, &search.candidates)
                .await;
            if let Ok(first) = &suggestion
                && first.index.is_none()
                && let Some(title) = first
                    .search_title
                    .as_deref()
                    .filter(|title| normalize(title) != normalize(query))
                && let Ok(revised) = self.gather_candidates(kind, title).await
                && !revised.candidates.is_empty()
            {
                search = revised;
                suggestion = self
                    .suggest_ai_match(kind, query, year, &search.candidates)
                    .await;
            }
            match suggestion {
                Ok(suggestion) => {
                    if let Some(index) = suggestion.index {
                        let candidate = &search.candidates[index];
                        if suggestion.confident
                            && year.is_none_or(|year| candidate.year == Some(year))
                        {
                            return Ok((
                                CandidateChoice {
                                    provider: candidate.provider,
                                    id: candidate.id.clone(),
                                },
                                true,
                            ));
                        }
                        search.candidates.swap(0, index);
                        search
                            .notes
                            .push("首项为 AI 建议，请核对片名和年份。".into());
                    }
                }
                Err(error) => search.notes.push(error),
            }
        }
        if search.candidates.is_empty() {
            return Err(Box::new(if search.notes.is_empty() {
                Outcome::new("not_found", Some("没有找到候选作品。".into()))
            } else {
                Outcome::failed(search.notes.join(" "))
            }));
        }
        for candidate in &mut search.candidates {
            candidate.match_titles.clear();
        }
        let mut outcome = Outcome::new(
            "choice",
            (!search.notes.is_empty()).then(|| search.notes.join(" ")),
        );
        outcome.candidates = serde_json::to_value(search.candidates).ok();
        Err(Box::new(outcome))
    }
}

async fn mark_collecting(store: &Store, claim: &Claim, candidate: &CandidateChoice) -> bool {
    match store.collecting(claim, candidate).await {
        Ok(owned) => owned,
        Err(error) => {
            diagnostics::postgres_error("batch_collecting_failed", &error);
            false
        }
    }
}

async fn run_with_lease<T>(
    store: &Store,
    claim: &Claim,
    work: impl Future<Output = Option<T>>,
) -> Option<T> {
    tokio::pin!(work);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(4));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            outcome = &mut work => return outcome,
            _ = heartbeat.tick() => match store.renew(claim).await {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    diagnostics::postgres_error("batch_lease_renew_failed", &error);
                    return None;
                }
            },
        }
    }
}

fn match_key(claim: &Claim, import: bool, emby: Option<&Emby>) -> Option<String> {
    if import {
        return Some(format!("emby:{}:{}", emby?.server_address(), claim.query));
    }
    let query = normalize(&claim.query);
    Some(match &claim.folder_root {
        Some(root) => format!("folder:{}:{root}:{query}", root.len()),
        None => format!("manual:{query}"),
    })
}

pub(super) fn normalize(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .trim()
        .trim_start_matches('《')
        .trim_end_matches('》')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn unique_candidate(
    query: &str,
    year: Option<u16>,
    search: &SearchResult,
) -> Option<CandidateChoice> {
    let query = normalize(query);
    let to_choice = |candidate: &crate::providers::media::Candidate| CandidateChoice {
        provider: candidate.provider,
        id: candidate.id.clone(),
    };
    if crate::providers::media::tmdb_id_query(&query).is_some() {
        return search
            .candidates
            .iter()
            .find(|candidate| candidate.provider == MediaProvider::Tmdb)
            .map(to_choice);
    }
    if let Some(id) = crate::providers::media::bangumi::id_query(&query) {
        let id = crate::providers::media::bangumi::id(id)?;
        return search
            .candidates
            .iter()
            .find(|candidate| {
                candidate.provider == MediaProvider::Bangumi && candidate.bangumi_id == Some(id)
            })
            .map(to_choice);
    }
    if let Some(id) = query.strip_prefix('q')
        && !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return search
            .candidates
            .iter()
            .find(|candidate| {
                candidate.provider == MediaProvider::Wikidata
                    && candidate.id.eq_ignore_ascii_case(&query)
            })
            .map(to_choice);
    }
    if year.is_none() && !search.notes.is_empty() {
        return None;
    }
    let mut exact = search.candidates.iter().filter(|candidate| {
        year.is_none_or(|year| candidate.year == Some(year))
            && std::iter::once(&candidate.title)
                .chain(&candidate.match_titles)
                .any(|title| normalize(title) == query)
    });
    let candidate = exact.next()?;
    exact.next().is_none().then(|| to_choice(candidate))
}

#[cfg(test)]
#[path = "../tests/worker.rs"]
mod tests;
