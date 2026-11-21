use std::{sync::Arc, time::Duration};

use unicode_normalization::UnicodeNormalization;

use super::{CandidateChoice, Claim, Outcome};
use crate::{
    Collector, MediaKind, MediaProvider, SearchResult,
    output::{PreparedWriteback, emby_source, prepare_writeback},
    storage::Store,
};

enum Work {
    Collect(Outcome),
    Writeback(PreparedWriteback),
}

impl Collector {
    pub async fn run_batch_worker(self: Arc<Self>) {
        tokio::join!(self.batch_loop(), self.batch_loop(), self.batch_loop());
    }

    async fn batch_loop(&self) {
        if !self.storage_configured() {
            return;
        }
        let owner = self.output_settings().map(|settings| settings.owner());
        loop {
            let delay = if let Ok(store) = self.require_store().await {
                match store.claim(self.tmdb_configured(), owner.as_deref()).await {
                    Ok(Some(claim)) => {
                        self.run_claim(store, &claim).await;
                        continue;
                    }
                    Ok(None) => Duration::from_millis(600),
                    Err(_) => Duration::from_secs(2),
                }
            } else {
                Duration::from_secs(2)
            };
            tokio::time::sleep(delay).await;
        }
    }

    async fn run_claim(&self, store: &Store, claim: &Claim) {
        let outcome = {
            let work = async {
                if let Some((job, state)) = &claim.writeback {
                    let result = store.batch_row_result(claim.batch, claim.id).await.ok()??;
                    let paths = self.output_settings()?.media_paths();
                    let emby = self.emby_connection().ok();
                    Some(Work::Writeback(
                        prepare_writeback(
                            &self.client,
                            emby.as_deref(),
                            &paths,
                            job,
                            &result.record,
                            state,
                        )
                        .await,
                    ))
                } else {
                    self.process_claim(store, claim).await.map(Work::Collect)
                }
            };
            tokio::pin!(work);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(4));
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    outcome = &mut work => break outcome,
                    _ = heartbeat.tick() => {
                        if !matches!(store.renew(claim).await, Ok(true)) {
                            break None;
                        }
                    }
                }
            }
        };
        if let Some(outcome) = outcome {
            let finished = match outcome {
                Work::Collect(outcome) => store.finish(claim, outcome).await,
                Work::Writeback(output) => {
                    let (job, state) = claim.writeback.as_ref().expect("writeback claim");
                    store.finish_writeback(claim, job, state, output).await
                }
            };
            match finished {
                Ok(true) => return,
                Ok(false) => {}
                Err(error) => {
                    eprintln!("Batch item {} could not be committed: {}", claim.id, error);
                }
            }
        }
        let _ = store.release_revoked(claim).await;
    }

    async fn process_claim(&self, store: &Store, claim: &Claim) -> Option<Outcome> {
        let _permit = self.batch_permit().await;
        if claim.query.is_empty() {
            return Some(Outcome::failed("缺少作品名称或番号。".into()));
        }
        let (query, year) = if claim.from_folder {
            super::folder::split_search_year(&claim.query)
        } else {
            (claim.query.as_str(), None)
        };
        let import = query.starts_with("emby:") || query.starts_with(super::EPISODE_QUERY_PREFIX);
        let emby = self.emby_settings().and_then(|settings| {
            if import {
                settings.connection()
            } else {
                settings.active()
            }
        });
        let home = if (claim.from_folder || import)
            && claim.candidate.is_none()
            && let Some(emby) = emby.as_deref()
        {
            self.gather_emby(emby, claim.kind, query, year).await
        } else {
            None
        };
        let result = if let Some(home) = home {
            Ok(home)
        } else if import {
            return Some(Outcome::failed(
                "无法读取该 Emby 条目，请检查媒体库与连接设置。".into(),
            ));
        } else if claim.kind == MediaKind::Av {
            self.gather_av(&claim.query).await
        } else {
            let candidate = if let Some(candidate) = &claim.candidate {
                candidate.clone()
            } else {
                let mut search = match self.gather_candidates(claim.kind, query).await {
                    Ok(search) => search,
                    Err(error) => return Some(Outcome::failed(error)),
                };
                if search.candidates.is_empty() {
                    return Some(if search.notes.is_empty() {
                        Outcome::new("not_found", Some("没有找到候选作品。".into()))
                    } else {
                        Outcome::failed(search.notes.join(" "))
                    });
                }
                let Some(candidate) = unique_candidate(query, year, &search) else {
                    for candidate in &mut search.candidates {
                        candidate.match_titles.clear();
                    }
                    let mut outcome = Outcome::new(
                        "choice",
                        (!search.notes.is_empty()).then(|| search.notes.join(" ")),
                    );
                    outcome.candidates = serde_json::to_value(search.candidates).ok();
                    return Some(outcome);
                };
                if !matches!(store.collecting(claim, &candidate).await, Ok(true)) {
                    return None;
                }
                candidate
            };
            self.gather_media(claim.kind, candidate.provider, &candidate.id)
                .await
        };

        let mut outcome = match result {
            Ok(result) => Outcome::collected(result),
            Err(error) => Outcome::failed(error),
        };
        if let Some(result) = outcome
            .result
            .as_ref()
            .filter(|result| result.has_fields() && emby_source(&result.record).is_none())
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
                    claim.id,
                    result,
                    &claim.source_files,
                )
                .await,
            );
        }
        Some(outcome)
    }
}

fn normalize(value: &str) -> String {
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
