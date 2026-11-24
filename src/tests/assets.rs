use serde_json::json;

use super::*;

fn fixture_emby(server: &str) -> std::sync::Arc<Emby> {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("emby.json");
    std::fs::write(
        &config,
        json!({"enabled":true,"server":server,"api_key":"fixture"}).to_string(),
    )
    .unwrap();
    crate::EmbySettings::load(config)
        .unwrap()
        .connection()
        .unwrap()
}

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

#[test]
fn prefers_embys_associated_images_and_filters_portraits_by_role() {
    let emby = fixture_emby("http://127.0.0.1:3030");
    let record = json!({
        "fields":{"poster_url":"https://static.tvmaze.com/public.jpg"},
        "sources":[{"provider":"Emby","data":{
            "item":{"Id":"11","Type":"Series","ImageTags":{"Primary":"poster"},"BackdropImageTags":["backdrop"],
                "People":[
                    {"Id":"21","Name":"演员","Type":"Actor","PrimaryImageTag":"actor"},
                    {"Id":"22","Name":"声优","Type":"Actor","Role":"角色 (voice)","PrimaryImageTag":"voice"},
                    {"Id":"23","Name":"导演","Type":"Director","PrimaryImageTag":"director"},
                    {"Id":"24","Name":"编剧","Type":"Writer","PrimaryImageTag":"writer"},
                    {"Id":"25","Name":"无头像","Type":"Actor"},
                    {"Id":"26","Name":"幕后","Type":"Producer","PrimaryImageTag":"crew"}
                ]},
            "children":[{"Id":"31","Type":"Episode","ImageTags":{"Primary":"still"}},
                {"Id":"32","Type":"Season","ImageTags":{"Primary":"season"}}]
        }}]
    });
    let options = serde_json::from_value(json!({
        "poster":true,"backdrop":true,"stills":true,"portraits":["actor","voice","director","crew"],
    }))
    .unwrap();
    let assets = selected_assets(&record, &options, Some(&emby));
    assert!(assets.iter().all(|asset| asset.from_emby));
    let mut selected = assets
        .iter()
        .map(|asset| (asset.kind, asset.url.path(), asset.person.as_deref()))
        .collect::<Vec<_>>();
    selected.sort_unstable();
    assert_eq!(
        selected,
        [
            ("actor", "/emby/Items/21/Images/Primary", Some("演员")),
            ("backdrop", "/emby/Items/11/Images/Backdrop", None),
            ("crew", "/emby/Items/26/Images/Primary", Some("幕后")),
            ("director", "/emby/Items/23/Images/Primary", Some("导演")),
            ("poster", "/emby/Items/11/Images/Primary", None),
            ("still", "/emby/Items/31/Images/Primary", None),
            ("voice", "/emby/Items/22/Images/Primary", Some("声优")),
        ]
    );
}

#[test]
fn matches_portable_bangumi_portraits_without_accepting_arbitrary_hosts() {
    let record = json!({"sources":[
        {"provider":"Bangumi","data":{"subject":{"type":2},
            "characters":[{"actors":[
                {"name":"声优","images":{"large":"https://elsewhere.example/voice.jpg","medium":"https://lain.bgm.tv/pic/voice.jpg"}}
            ]}],
            "persons":[
                {"type":1,"name":"导演","relation":"导演","images":{"large":"https://lain.bgm.tv/pic/director.jpg"}},
                {"type":2,"name":"公司","relation":"制作","images":{"large":"https://lain.bgm.tv/pic/company.jpg"}}
            ]
        }},
        {"provider":"Bangumi","data":{"subject":{"type":6},
            "persons":[
                {"type":1,"name":"演员","relation":"主演","images":{"large":"https://lain.bgm.tv/pic/actor.jpg"}},
                {"type":1,"name":"丢弃","relation":"主演","images":{"large":"https://static.tvmaze.com/photo.jpg"}}
            ]
        }}
    ]});
    let options =
        serde_json::from_value(json!({"portraits":["voice","director","actor"]})).unwrap();
    let assets = selected_assets(&record, &options, None);
    assert_eq!(
        assets
            .iter()
            .map(|asset| (asset.kind, asset.url.host_str(), asset.person.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("voice", Some("lain.bgm.tv"), Some("声优")),
            ("director", Some("lain.bgm.tv"), Some("导演")),
            ("actor", Some("lain.bgm.tv"), Some("演员")),
        ]
    );
}

#[cfg(feature = "web")]
#[tokio::test]
async fn validates_emby_image_payloads_by_signature_and_limits_size() {
    use axum::{Router, extract::Path, routing::get};

    let app = Router::new().route(
        "/emby/Items/{id}/Images/Primary",
        get(|Path(id): Path<String>| async move {
            match id.as_str() {
                "1" => b"RIFF0000WEBPdata".to_vec(),
                "2" => b"GIF89apixels".to_vec(),
                "3" => b"GIF87apixels".to_vec(),
                "4" => b"<html>not an image</html>".to_vec(),
                "5" => vec![b'0'; 12 * 1024 * 1024 + 1],
                _ => b"\x89PNG\r\n\x1a\npixels".to_vec(),
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let emby = fixture_emby(&base);
    let client = Client::builder().no_proxy().build().unwrap();
    for (id, expected) in [
        ("1", Some("webp")),
        ("2", Some("gif")),
        ("3", Some("gif")),
        ("4", None),
        ("5", None),
        ("6", Some("png")),
    ] {
        let url = emby.image_url(&json!(id), "Primary").unwrap();
        let asset = Asset {
            url: url.clone(),
            kind: "poster",
            person: None,
            from_emby: true,
        };
        assert_eq!(
            download(&client, &asset, Some(&emby))
                .await
                .map(|(_, ext)| ext),
            expected
        );
        assert!(download(&client, &asset, None).await.is_none());
        assert!(
            public_response(&client, &url, "image/*", Duration::from_secs(1))
                .await
                .is_none()
        );
    }
    server.abort();
}
