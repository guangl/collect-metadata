use serde_json::json;

use super::*;

#[test]
fn finds_tvmaze_stills_and_portraits_and_excludes_untrusted_images() {
    let record = json!({"fields": {"poster_url":"https://static.tvmaze.com.example.org/a.jpg",
        "episodes":[{"still_url":"https://static.tvmaze.com/one.jpg"}]},
        "sources":[{"provider":"TVmaze","fields":{"cover_url":"https://static.tvmaze.com/poster.jpg"},
            "data":{"gallery":[{"thumbnail_url":"https://static.tvmaze.com/one.jpg"},
                {"full_url":"https://static.tvmaze.com/gallery.jpg"}], "_embedded":{
                "episodeswithspecials":[{"image":{"original":"https://static.tvmaze.com/two.jpg"}}],
                "cast":[{"person":{"name":"演员","image":{"original":"https://static.tvmaze.com/actor.jpg"}}},
                    {"voice":true,"person":{"name":"配音","image":{"medium":"https://static.tvmaze.com/voice.jpg"}}}],
                "crew":[{"type":"Screenplay","person":{"name":"编剧","image":{"original":"https://static.tvmaze.com/writer.jpg"}}},
                    {"type":"Producer","person":{"name":"制片人","image":{"original":"https://static.tvmaze.com/producer.jpg"}}}]}}}]});
    let options =
        serde_json::from_value(json!({"stills":true,"portraits":["actor","voice","writer"]}))
            .unwrap();
    let assets = selected_assets(&record, &options, None);
    assert_eq!(
        assets
            .iter()
            .map(|item| (item.kind, item.url.path()))
            .collect::<Vec<_>>(),
        [
            ("poster", "/poster.jpg"),
            ("still", "/one.jpg"),
            ("still", "/gallery.jpg"),
            ("still", "/two.jpg"),
            ("actor", "/actor.jpg"),
            ("voice", "/voice.jpg"),
            ("writer", "/writer.jpg"),
        ]
    );
    assert_eq!(assets[5].person.as_deref(), Some("配音"));
    for rejected in [
        "http://static.tvmaze.com/image.jpg",
        "https://static.tvmaze.com:444/image.jpg",
        "https://user@static.tvmaze.com/image.jpg",
        "https://maliciousexampledmm.co.jp/image.jpg",
    ] {
        assert!(asset_url(rejected).is_none());
    }
    assert!(asset_url("https://cdn.dmm.co.jp/image.jpg").is_some());
}
