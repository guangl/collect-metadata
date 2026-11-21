use super::*;
use serde_json::Value;

#[test]
fn accepts_compact_and_multi_part_codes_without_merging_distinct_ids() {
    let id = Identifier::parse("  abC－001 ").unwrap();
    assert!(id.matches("abc001"));
    assert!(!id.matches("ABC-0012"));
    assert!(!id.matches("ABC-01"));
    assert_eq!(
        Identifier::parse("abc001").unwrap().dvd_candidate(),
        "ABC-001"
    );
    assert_eq!(
        Identifier::parse("FC2-PPV-123").unwrap().dvd_candidate(),
        "FC2-PPV-123"
    );
    assert!(Identifier::parse("../../ABC-123").is_err());
}

#[test]
fn normalizes_special_ids_for_exact_matching() {
    for (input, expected, alias, different) in [
        ("有码番号：ａｂｃ－００１", "ABC-001", "ABC - 001", "ABC-01"),
        (
            "无码番号：1pondo_010124_123",
            "1PONDO-010124-123",
            "1PONDO 010124-123",
            "CARIB-010124-123",
        ),
        (
            "素人番号：300mium001",
            "300MIUM-001",
            "300MIUM_001",
            "MIUM-001",
        ),
        (
            "ｆｃ２番号：fc2 ppv 1234567",
            "FC2-PPV-1234567",
            "FC2-1234567",
            "1234567",
        ),
        (
            "欧美番号：Naughty.America.24.01.31",
            "NAUGHTYAMERICA-24-01-31",
            "Naughty America 24.01.31",
            "Naughty America 24.01.30",
        ),
        ("国产番号：mdx0123", "MDX-0123", "MDX-0123", "MDX-123"),
        ("动漫里番：rj123456", "RJ-123456", "RJ123456", "VJ123456"),
        (
            "mywife：My Wife 0123",
            "MYWIFE-0123",
            "Mywife-0123",
            "Mywife-123",
        ),
        ("Tokyo-Hot n1234", "N1234", "N-1234", "N-12345"),
    ] {
        let id = Identifier::parse(input).unwrap();
        assert_eq!(id.formatted, expected, "{input}");
        assert_eq!(id.dvd_candidate(), expected);
        assert!(id.matches(alias), "{input} should match {alias}");
        assert!(
            !id.matches(different),
            "{input} should not match {different}"
        );
    }
    assert_eq!(
        Identifier::parse("Brazzers.24.01.31")
            .unwrap()
            .search_candidate(),
        "BRAZZERS 24.01.31"
    );
    for input in ["../../ABC-123", "FC2/123", "番号：", "Mywife-0123?x=1"] {
        assert!(Identifier::parse(input).is_err(), "{input}");
    }
}

#[tokio::test]
#[ignore = "访问公开的影片资料站点"]
async fn live_sources() {
    let app = Collector::new(None).unwrap();
    for (input, expected) in [("SSIS-001", "SSIS-001"), ("ssis00001", "SSIS-001")] {
        let response = app.lookup(input).await.unwrap();
        let record = response.record;
        let sources = record["sources"].as_array().unwrap();
        assert_eq!(record["fields"]["dvd_id"], expected);
        for source in sources {
            eprintln!(
                "{}: {} {}",
                source["provider"], source["status"], source["note"]
            );
            assert!(matches!(
                source["status"].as_str(),
                Some("ok" | "partial" | "unavailable")
            ));
        }
        assert!(
            sources
                .iter()
                .filter(|source| matches!(source["status"].as_str(), Some("ok" | "partial")))
                .count()
                >= 2
        );
        if sources[2]["status"] == "ok" {
            assert_ne!(record["fields"]["rating"], Value::Null);
        }
        assert_ne!(sources[0]["data"]["combined_json"], Value::Null);
        assert_ne!(sources[0]["data"]["dvd_lookup_json"], Value::Null);
    }
    let missing = app.lookup("TESTZZZZ-99999999").await.unwrap();
    assert!(missing.record["fields"].as_object().unwrap().is_empty());
    let sources = missing.record["sources"].as_array().unwrap();
    assert!(sources.iter().any(|source| source["status"] == "not_found"));
    assert!(
        sources
            .iter()
            .all(|source| matches!(source["status"].as_str(), Some("not_found" | "unavailable")))
    );
}
#[tokio::test]
#[ignore = "访问 Wikidata 和 TVmaze 的公开 API"]
async fn live_catalog() {
    let collector = Collector::new(None).unwrap();
    for (title, kind, expected_id, language) in [
        ("《寄生虫》", MediaKind::Movie, "Q61448040", "ko"),
        ("千と千尋の神隠し", MediaKind::Movie, "Q155653", "ja"),
        ("无间道", MediaKind::Movie, "Q714057", "yue"),
        ("黑暗荣耀", MediaKind::Series, "Q113197148", "ko"),
        ("Dark", MediaKind::Series, "Q28443710", "de"),
    ] {
        let start = std::time::Instant::now();
        let results = collector.search_media(kind, title).await.unwrap();
        let hit = results
            .candidates
            .iter()
            .find(|hit| hit.id == expected_id || (title == "Dark" && hit.tvmaze_id == Some(17861)))
            .unwrap_or_else(|| {
                panic!(
                    "missing {expected_id} for {title}; notes {:?}",
                    results.notes
                )
            });
        assert!(hit.original_languages.iter().any(|code| code == language));
        eprintln!(
            "search {title}: {:.2?}, candidates {}",
            start.elapsed(),
            results.candidates.len()
        );
    }
    let result = collector
        .collect_media(MediaKind::Series, MediaProvider::Wikidata, "Q28443710")
        .await
        .unwrap();
    assert_eq!(result.record["media_type"], "series");
    assert_eq!(
        result.record["fields"]["episodes"]
            .as_array()
            .unwrap()
            .len(),
        26
    );
    assert_eq!(result.record["field_sources"]["episodes"], "TVmaze");
}
