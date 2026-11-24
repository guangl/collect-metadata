use super::*;

#[test]
fn legacy_episode_requires_unique_season_and_episode_number_and_a_usable_source() {
    let mut record = json!({
        "fields":{"title":"Show","episodes":[
            {"season":3,"number":1,"title":"Wrong season"},
            {"season":1,"number":1,"title":"First","description":"Plot","date":"2026-01-01", "still_url":"https://static.tvmaze.com/still.jpg"}
        ]},
        "sources":[{"status":"unavailable","url":"https://example.org/failed"},
            {"status":"partial","url":"https://static.tvmaze.com/selected"}]
    });
    let mut item = json!({"ParentIndexNumber":1,"IndexNumber":1});
    for (provider, label) in [
        (MediaProvider::Tvmaze, "TVmaze"),
        (MediaProvider::Wikidata, "Wikidata"),
    ] {
        let result = exact_legacy_episode(&record, &item, provider).unwrap();
        assert_eq!(result.provider, label);
        assert_eq!(result.fields["title"], "First");
        assert_eq!(result.fields["description"], "Plot");
        assert_eq!(result.fields["release_date"], "2026-01-01");
        assert_eq!(result.fields["series"], "Show");
    }
    let fail = |record: &Value, item: &Value| {
        exact_legacy_episode(record, item, MediaProvider::Tvmaze)
            .err()
            .unwrap()
    };
    item["IndexNumber"] = Value::Null;
    assert!(fail(&record, &item).contains("缺少集号"));
    item["IndexNumber"] = json!(1);
    item["ParentIndexNumber"] = Value::Null;
    assert!(fail(&record, &item).contains("缺少季号"));
    item["ParentIndexNumber"] = json!(1);
    let duplicate = record["fields"]["episodes"][1].clone();
    record["fields"]["episodes"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(fail(&record, &item).contains("唯一的季集"));
    record["fields"]["episodes"].as_array_mut().unwrap().pop();
    record["sources"][1]["url"] = json!("invalid");
    assert!(fail(&record, &item).contains("来源地址无效"));
    record["sources"][1]["status"] = json!("unavailable");
    assert!(fail(&record, &item).contains("来源不可用"));
    record["fields"]["episodes"] = Value::Null;
    assert!(fail(&record, &item).contains("暂无分集资料"));
}
