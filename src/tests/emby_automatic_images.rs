use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as Route, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

use super::*;

#[derive(Default)]
struct Images {
    saved: HashMap<String, Vec<Value>>,
    people: HashMap<String, Value>,
    uploads: Vec<(String, String, String)>,
}

struct Fixture {
    emby: Emby,
    client: Client,
    images: Arc<Mutex<Images>>,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let images = Arc::new(Mutex::new(Images::default()));
        let app = Router::new()
            .route("/emby/Users/Query", get(|| async {
                Json(json!({"Items":[{"Id":"7","Policy":{"IsAdministrator":true}}]}))
            }))
            .route("/emby/Users/7/Items/{id}", get(|State(images): State<Arc<Mutex<Images>>>, Route(id): Route<String>| async move {
                Json(images.lock().unwrap().people[&id].clone())
            }))
            .route("/emby/Items/{id}/Images", get(|State(images): State<Arc<Mutex<Images>>>, Route(id): Route<String>| async move {
                Json(images.lock().unwrap().saved.get(&id).cloned().unwrap_or_default())
            }))
            .route("/emby/Items/{id}/Images/{kind}", post(upload))
            .with_state(Arc::clone(&images));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let emby = Emby::new(&url, "fixture", &Arc::default()).unwrap();
        let client = Client::builder().no_proxy().build().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            emby,
            client,
            images,
            server,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn upload(
    State(images): State<Arc<Mutex<Images>>>,
    Route((id, kind)): Route<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    assert_eq!(headers["X-Emby-Token"], "fixture");
    assert_eq!(STANDARD.decode(body).unwrap(), b"image bytes");
    let mut images = images.lock().unwrap();
    let content_type = headers["content-type"].to_str().unwrap().to_owned();
    images
        .uploads
        .push((id.clone(), kind.clone(), content_type));
    if id != "99" {
        let root = if id == "98" {
            "/media"
        } else {
            "/server-metadata"
        };
        images.saved.entry(id).or_default().push(json!({
            "ImageType":kind, "Path":format!("{root}/file")
        }));
    }
    StatusCode::NO_CONTENT
}

#[tokio::test]
async fn item_images_preserve_existing_files_and_require_server_metadata_confirmation() {
    let fixture = Fixture::new().await;
    fixture
        .images
        .lock()
        .unwrap()
        .saved
        .insert("41".into(), vec![json!({"ImageType":"Primary"})]);
    let fields = json!({
        "poster_url":"poster","backdrop_url":"backdrop",
        "logo_url":"logo","landscape_url":"thumb",
    });
    let item = json!({"Id":"41","Type":"Movie"});
    let images = &fixture.images;
    fixture
        .emby
        .fill_missing_images(
            &fixture.client,
            &item,
            &fields,
            Path::new("/media"),
            |url| async move {
                let extension = match url.as_str() {
                    "backdrop" => {
                        // Another writer filled it while the image was being downloaded.
                        images
                            .lock()
                            .unwrap()
                            .saved
                            .get_mut("41")
                            .unwrap()
                            .push(json!({"ImageType":"Backdrop"}));
                        "jpg"
                    }
                    "logo" => "png",
                    "thumb" => "webp",
                    other => panic!("unexpected image request: {other}"),
                };
                Some((b"image bytes".to_vec(), extension))
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.images.lock().unwrap().uploads,
        [
            ("41".into(), "Logo".into(), "image/png".into()),
            ("41".into(), "Thumb".into(), "image/webp".into()),
        ]
    );
    fixture
        .emby
        .fill_missing_images(
            &fixture.client,
            &json!({"Id":"42","Type":"Movie","LockedFields":["Images"]}),
            &fields,
            Path::new("/media"),
            |_| async { panic!("images are locked") },
        )
        .await
        .unwrap();

    let episode = json!({"Id":"98","Type":"Episode"});
    let fields = json!({"still_url":"still","poster_url":"do not use the series poster"});
    let error = fixture
        .emby
        .fill_missing_images(
            &fixture.client,
            &episode,
            &fields,
            Path::new("/media"),
            |url| async move {
                assert_eq!(url, "still");
                Some((b"image bytes".to_vec(), "gif"))
            },
        )
        .await
        .unwrap_err();
    assert!(error.contains("服务器元数据目录"));
    assert_eq!(
        fixture.images.lock().unwrap().uploads.last().unwrap().2,
        "image/gif"
    );

    for (download, expected) in [(None, "无法读取"), (Some("bmp"), "格式无效")] {
        let result = fixture
            .emby
            .fill_missing_images(
                &fixture.client,
                &json!({"Id":"43","Type":"Episode"}),
                &fields,
                Path::new("/media"),
                |_| async move { download.map(|ext| (b"image bytes".to_vec(), ext)) },
            )
            .await;
        assert!(result.unwrap_err().contains(expected));
    }
}

#[tokio::test]
async fn portraits_recheck_identity_and_confirm_the_saved_image() {
    let fixture = Fixture::new().await;
    for id in ["51", "52", "53", "99"] {
        fixture
            .images
            .lock()
            .unwrap()
            .people
            .insert(id.into(), json!({"Type":"Person","Name":"Known person"}));
    }
    for (id, name) in [("invalid/id", "Known person"), ("51", "Another person")] {
        fixture
            .emby
            .fill_portrait(&fixture.client, id, name, || async {
                panic!("identity was not verified")
            })
            .await
            .unwrap();
    }
    let image = || async { Some((b"image bytes".to_vec(), "jpg")) };
    fixture
        .emby
        .fill_portrait(&fixture.client, "51", "Known person", image)
        .await
        .unwrap();
    fixture
        .emby
        .fill_portrait(&fixture.client, "51", "Known person", image)
        .await
        .unwrap();
    fixture
        .emby
        .fill_portrait(&fixture.client, "52", "Known person", || async {
            fixture.images.lock().unwrap().people.get_mut("52").unwrap()["Name"] =
                json!("Renamed concurrently");
            Some((b"image bytes".to_vec(), "jpg"))
        })
        .await
        .unwrap();
    fixture
        .emby
        .fill_portrait(&fixture.client, "53", "Known person", || async {
            Some((b"image bytes".to_vec(), "gif"))
        })
        .await
        .unwrap();
    assert!(
        fixture
            .emby
            .fill_portrait(&fixture.client, "53", "Known person", || async { None })
            .await
            .unwrap_err()
            .contains("来源头像无法读取")
    );
    assert!(
        fixture
            .emby
            .fill_portrait(&fixture.client, "99", "Known person", image)
            .await
            .unwrap_err()
            .contains("未确认头像保存")
    );
    assert_eq!(
        fixture.images.lock().unwrap().uploads,
        [
            ("51".into(), "Primary".into(), "image/jpeg".into()),
            ("99".into(), "Primary".into(), "image/jpeg".into()),
        ]
    );
}
