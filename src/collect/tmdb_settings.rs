use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use reqwest::Client;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::providers::media::Tmdb;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TmdbStatus {
    Unconfigured,
    Environment,
    Saved,
}

pub struct TmdbSettings {
    file: Option<PathBuf>,
    active: RwLock<Option<(TmdbStatus, Arc<Tmdb>)>>,
    saving: Mutex<()>,
}

impl TmdbSettings {
    pub fn from_file(path: PathBuf) -> Result<Self, String> {
        let stored = match fs::read_to_string(&path) {
            Ok(value) => Tmdb::new(&value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取已保存的 TMDB 配置。".into()),
        };
        Ok(Self::new(Some(path), stored))
    }

    pub(super) fn from_environment() -> Self {
        Self::new(None, None)
    }

    fn new(file: Option<PathBuf>, stored: Option<Tmdb>) -> Self {
        let active = stored
            .map(|tmdb| (TmdbStatus::Saved, Arc::new(tmdb)))
            .or_else(|| {
                let value = env::var("TMDB_READ_ACCESS_TOKEN").ok()?;
                Tmdb::new(&value).map(|tmdb| (TmdbStatus::Environment, Arc::new(tmdb)))
            });
        Self {
            file,
            active: RwLock::new(active),
            saving: Mutex::new(()),
        }
    }

    pub(super) fn status(&self) -> TmdbStatus {
        self.active
            .read()
            .expect("TMDB settings lock")
            .as_ref()
            .map_or(TmdbStatus::Unconfigured, |(status, _)| *status)
    }

    pub(super) fn provider(&self) -> Option<Arc<Tmdb>> {
        self.active
            .read()
            .expect("TMDB settings lock")
            .as_ref()
            .map(|(_, provider)| Arc::clone(provider))
    }

    pub(super) async fn test(&self, client: &Client, input: Option<&str>) -> Result<(), String> {
        let tmdb = match input.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => Arc::new(parse(value)?),
            None => self
                .provider()
                .ok_or("请填写 TMDB API 读取令牌或 v3 API Key。")?,
        };
        tmdb.validate(client).await
    }

    pub(super) async fn save(&self, client: &Client, input: String) -> Result<TmdbStatus, String> {
        let file = self
            .file
            .clone()
            .ok_or("此进程没有可写入的 TMDB 配置文件。")?;
        let tmdb = Arc::new(parse(&input)?);
        let _saving = self.saving.lock().await;
        tmdb.validate(client).await?;
        // Keep persistence and the active provider synchronized even if the calling request is dropped.
        let mut active = self.active.write().expect("TMDB settings lock");
        write_credential(&file, input.trim()).map_err(|_| "无法保存 TMDB 配置。")?;
        *active = Some((TmdbStatus::Saved, tmdb));
        Ok(TmdbStatus::Saved)
    }
}

fn parse(value: &str) -> Result<Tmdb, String> {
    Tmdb::new(value).ok_or_else(|| "请填写有效格式的 TMDB API 读取令牌或 v3 API Key。".into())
}

pub(super) fn write_credential(path: &Path, value: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    writeln!(temporary, "{value}")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}
