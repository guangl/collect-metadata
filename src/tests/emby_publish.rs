use super::*;
use crate::output::emby::{FileKind, paths::PathMapping};
use cap_std::ambient_authority;

#[test]
fn writes_never_replace_a_newer_sidecar_or_lose_the_original_if_backups_fail() {
    let root = tempfile::tempdir().unwrap();
    let paths = MediaPaths::new(
        vec![PathMapping {
            server: "/media".into(),
            local: root.path().to_str().unwrap().to_owned(),
        }],
        false,
        None,
    );
    let target = paths.target("/media/episode.mkv", false).unwrap();
    let video = root.path().join("episode.mkv");
    fs::write(video, b"video").unwrap();
    let nfo = root.path().join("episode.nfo");
    let staged_path = root.path().join("staged.nfo");
    fs::write(&staged_path, b"new metadata").unwrap();
    let dir = target.directory().unwrap();
    let mut file = PlannedFile {
        key: "episode:nfo".into(),
        source: "/media/episode.mkv".into(),
        item_id: json!("30"),
        kind: FileKind::Nfo,
        public_image: None,
        target: Some(target),
        destinations: Vec::new(),
        blocked: None,
        create_only: true,
    };
    let mut staged = StagedFile {
        destination: Destination {
            name: "episode.nfo".into(),
            existing: Existing::Missing,
        },
        path: staged_path,
    };

    fs::create_dir(&nfo).unwrap();
    let error = publish_file(&file, &staged, "backup").err().unwrap();
    assert!(error.contains("目标在写回时已变更"));
    assert!(nfo.is_dir());
    fs::remove_dir(&nfo).unwrap();

    fs::write(&nfo, b"metadata created during the scrape").unwrap();
    assert!(publish_file(&file, &staged, "backup").is_ok());
    assert_eq!(
        fs::read(&nfo).unwrap(),
        b"metadata created during the scrape"
    );
    file.create_only = false;
    assert!(
        publish_file(&file, &staged, "backup")
            .err()
            .unwrap()
            .contains("在预览后已变更")
    );
    staged.destination.existing = Existing::read(&dir, "episode.nfo");

    let backup_directory = root.path().join(BACKUP_DIRECTORY);
    fs::write(&backup_directory, b"not a directory").unwrap();
    assert!(
        publish_file(&file, &staged, "backup")
            .err()
            .unwrap()
            .contains("无法保留旧文件")
    );
    assert_eq!(
        fs::read(&nfo).unwrap(),
        b"metadata created during the scrape"
    );
    fs::remove_file(&backup_directory).unwrap();

    fs::create_dir(&backup_directory).unwrap();
    let backup = backup_directory.join("backup");
    fs::create_dir(&backup).unwrap();
    assert!(
        publish_file(&file, &staged, "backup")
            .err()
            .unwrap()
            .contains("无法保留旧文件")
    );
    fs::remove_dir(&backup).unwrap();
    assert!(publish_file(&file, &staged, "backup").is_ok());
    assert_eq!(
        fs::read(backup).unwrap(),
        b"metadata created during the scrape"
    );
    assert_eq!(fs::read(nfo).unwrap(), b"new metadata");
}

#[test]
fn an_atomic_write_that_loses_the_precondition_cleans_up_its_temporary_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
    let existing = root.path().join("episode.nfo");
    fs::write(&existing, b"another writer").unwrap();
    let result = write_atomic(&dir, "episode.nfo", b"tentative".as_slice(), false, || {
        false
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(existing).unwrap(), b"another writer");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn retry_reuses_confirmed_artwork_and_reports_invalid_fallback_sources() {
    let root = tempfile::tempdir().unwrap();
    let paths = MediaPaths::new(
        vec![PathMapping {
            server: "/media".into(),
            local: root.path().to_str().unwrap().to_owned(),
        }],
        false,
        None,
    );
    let source = "/media/movie.mkv";
    let target = paths.target(source, false).unwrap();
    fs::write(root.path().join("movie.mkv"), b"media").unwrap();
    fs::write(root.path().join("movie-poster.jpg"), b"preserved image").unwrap();
    let file = |kind: FileKind, name: &str, public_image| PlannedFile {
        key: format!("{}\n{source}", kind.label()),
        source: source.to_owned(),
        item_id: json!("30"),
        kind,
        public_image,
        target: Some(target.clone()),
        destinations: vec![Destination {
            name: name.into(),
            existing: Existing::Missing,
        }],
        blocked: None,
        create_only: false,
    };
    let state = WritebackState {
        remaining: None,
        files: [(
            format!("poster\n{source}"),
            FileResult::Written {
                path: root
                    .path()
                    .join("movie-poster.jpg")
                    .to_string_lossy()
                    .into_owned(),
                backup: None,
            },
        )]
        .into_iter()
        .collect(),
    };
    let record = json!({"media_type":"movie","fields":{"title":"Movie"}});
    let client = Client::builder().no_proxy().build().unwrap();
    for (url, expected) in [
        ("not an absolute URL", "来源图片地址无效"),
        (
            "https://example.invalid/image.png",
            "无法下载公开来源的图片",
        ),
    ] {
        let job = WritebackJob {
            id: "fixture".into(),
            owner: "fixture".into(),
            stage: "fixture".into(),
            files: vec![
                file(FileKind::Nfo, "movie.nfo", None),
                file(FileKind::Backdrop, "movie-fanart.jpg", Some(url.into())),
            ],
        };
        let prepared = prepare(&client, None, &paths, &job, &record, &state).await;
        let image = prepared
            .files
            .iter()
            .find(|(file, _)| matches!(file.kind, FileKind::Backdrop))
            .unwrap();
        assert!(image.1.as_ref().err().unwrap().contains(expected));
        let nfo = prepared
            .files
            .iter()
            .find(|(file, _)| matches!(file.kind, FileKind::Nfo))
            .unwrap();
        let xml = fs::read_to_string(&nfo.1.as_ref().unwrap().path).unwrap();
        assert!(xml.contains("movie-poster.jpg"), "{xml}");
        assert!(!xml.contains("movie-fanart.jpg"), "{xml}");
    }
    assert_eq!(
        fs::read(root.path().join("movie-poster.jpg")).unwrap(),
        b"preserved image"
    );
    assert!(!root.path().join("movie-fanart.jpg").exists());
}

#[tokio::test]
async fn staging_rejects_blocked_destinations_and_missing_staging_directories() {
    let root = tempfile::tempdir().unwrap();
    let mut destination = Destination {
        name: "episode.nfo".into(),
        existing: Existing::Blocked,
    };
    let error = stage_file(root.path(), &destination, "new NFO").await.err().unwrap();
    assert!(error.contains("目标不是普通文件"), "{error}");
    destination.existing = Existing::Missing;
    let error = stage_file(&root.path().join("absent"), &destination, "new NFO")
        .await
        .err()
        .unwrap();
    assert!(error.contains("无法暂存写回文件"), "{error}");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
