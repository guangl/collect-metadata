use std::{
    env, fs, io,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use reqwest::Client;
use serde::Serialize;
use tokio::sync::Mutex;

use super::tmdb_settings::write_credential;
use crate::providers::media::bangumi::Bangumi;

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BangumiStatus {
    #[default]
    Unconfigured,
    Environment,
    Saved,
}

pub struct BangumiSettings {
    file: Option<PathBuf>,
    active: RwLock<(BangumiStatus, Arc<Bangumi>)>,
    saving: Mutex<()>,
}

impl BangumiSettings {
    pub fn from_file(path: PathBuf) -> Result<Self, String> {
        let stored = match fs::read_to_string(&path) {
            Ok(value) => Bangumi::authenticated(&value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取已保存的 Bangumi 配置。".into()),
        };
        Ok(Self::new(Some(path), stored))
    }

    pub(super) fn from_environment() -> Self {
        Self::new(None, None)
    }

    fn new(file: Option<PathBuf>, stored: Option<Bangumi>) -> Self {
        let active = stored
            .map(|provider| (BangumiStatus::Saved, provider))
            .or_else(|| {
                let token = env::var("BANGUMI_ACCESS_TOKEN").ok()?;
                Bangumi::authenticated(&token)
                    .map(|provider| (BangumiStatus::Environment, provider))
            })
            .unwrap_or_default();
        Self {
            file,
            active: RwLock::new((active.0, Arc::new(active.1))),
            saving: Mutex::new(()),
        }
    }

    pub(super) fn status(&self) -> BangumiStatus {
        self.active.read().expect("Bangumi settings lock").0
    }

    pub(super) fn provider(&self) -> Arc<Bangumi> {
        Arc::clone(&self.active.read().expect("Bangumi settings lock").1)
    }

    pub(super) async fn test(&self, client: &Client, input: Option<&str>) -> Result<(), String> {
        let provider = match input.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => Arc::new(parse(value)?),
            None => {
                let active = self.active.read().expect("Bangumi settings lock");
                if matches!(active.0, BangumiStatus::Unconfigured) {
                    return Err("请填写 Bangumi 访问令牌。".into());
                }
                Arc::clone(&active.1)
            }
        };
        provider.validate(client).await
    }

    pub(super) async fn save(
        &self,
        client: &Client,
        input: String,
    ) -> Result<BangumiStatus, String> {
        let file = self
            .file
            .as_deref()
            .ok_or("此进程没有可写入的 Bangumi 配置文件。")?;
        let provider = Arc::new(parse(&input)?);
        let _saving = self.saving.lock().await;
        provider.validate(client).await?;
        let mut active = self.active.write().expect("Bangumi settings lock");
        write_credential(file, input.trim()).map_err(|_| "无法保存 Bangumi 配置。")?;
        *active = (BangumiStatus::Saved, provider);
        Ok(BangumiStatus::Saved)
    }
}

fn parse(value: &str) -> Result<Bangumi, String> {
    Bangumi::authenticated(value).ok_or_else(|| "请填写有效格式的 Bangumi 访问令牌。".into())
}
