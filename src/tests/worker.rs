use crate::providers::media::Candidate;

use super::*;

fn offer(provider: MediaProvider, id: &str, title: &str, year: u16) -> Candidate {
    Candidate {
        provider,
        id: id.to_owned(),
        title: title.to_owned(),
        match_titles: vec![],
        year: Some(year),
        original_languages: vec![],
        cover_url: None,
        tvmaze_id: None,
        tmdb_id: None,
    }
}

#[test]
fn only_automatically_selects_unambiguous_matches() {
    let mut first = offer(MediaProvider::Wikidata, "Q2", "远方", 2019);
    first.match_titles = vec!["Ｔｈｅ\u{3000}Distance".to_owned()];
    let second = offer(MediaProvider::Tmdb, "550", "The Distance", 2023);
    let mut search = SearchResult {
        candidates: vec![first, second],
        notes: vec![],
    };
    assert!(unique_candidate("The Distance", None, &search).is_none());
    assert_eq!(
        unique_candidate("《The Distance》", Some(2019), &search)
            .unwrap()
            .id,
        "Q2"
    );
    assert_eq!(
        unique_candidate("tmdb：550", None, &search).unwrap().id,
        "550"
    );
    assert_eq!(unique_candidate("q2", None, &search).unwrap().id, "Q2");
    assert!(unique_candidate("Q3", None, &search).is_none());
    search.notes.push("一个来源暂时不可用".to_owned());
    assert!(unique_candidate("远方", None, &search).is_none());
    assert_eq!(
        unique_candidate("远方", Some(2019), &search).unwrap().id,
        "Q2"
    );
    assert!(unique_candidate("远方", Some(2020), &search).is_none());
}
