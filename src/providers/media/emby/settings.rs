use std::{
    collections::HashMap,
    env, fs, io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{Emby, ImdbRating};

#[derive(Deserialize, Serialize, Default)]
pub struct EmbyInput {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    server: String,
    #[serde(default)]
    api_key: String,
}

#[derive(Serialize)]
pub struct EmbySettingsView {
    enabled: bool,
    server: String,
    configured: bool,
}

pub struct EmbySettings {
    file: PathBuf,
    state: Mutex<EmbyInput>,
    imdb_ratings: Arc<HashMap<String, ImdbRating>>,
}

impl EmbySettings {
    pub fn load(file: PathBuf) -> Result<Self, String> {
        let state = match fs::read_to_string(&file) {
            Ok(value) => serde_json::from_str(&value).map_err(|_| "Emby 设置格式无效。")?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let server = env::var("EMBY_URL").unwrap_or_default();
                let api_key = env::var("EMBY_API_KEY").unwrap_or_default();
                EmbyInput {
                    enabled: !server.is_empty() && !api_key.is_empty(),
                    server,
                    api_key,
                }
            }
            Err(_) => return Err("无法读取 Emby 设置。".into()),
        };
        let imdb_ratings = match fs::read_to_string(file.with_file_name("imdb-ratings.json")) {
            Ok(value) => serde_json::from_str(&value).map_err(|_| "IMDb 评分资料格式无效。")?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(_) => return Err("无法读取 IMDb 评分资料。".into()),
        };
        Ok(Self {
            file,
            state: Mutex::new(state),
            imdb_ratings: Arc::new(imdb_ratings),
        })
    }

    pub fn view(&self) -> EmbySettingsView {
        let config = self.state.lock().expect("Emby settings lock");
        view(&config)
    }

    fn resolve(&self, mut input: EmbyInput) -> EmbyInput {
        input.server = input.server.trim().trim_end_matches('/').to_owned();
        input.api_key = input.api_key.trim().to_owned();
        let saved = self.state.lock().expect("Emby settings lock");
        if input.api_key.is_empty() && input.server == saved.server {
            input.api_key.clone_from(&saved.api_key);
        }
        input
    }

    pub async fn test(&self, client: &Client, input: EmbyInput) -> Result<(), String> {
        let input = self.resolve(input);
        Emby::new(&input.server, &input.api_key, &self.imdb_ratings)?
            .validate(client)
            .await
    }

    pub async fn save(
        &self,
        client: &Client,
        input: EmbyInput,
    ) -> Result<EmbySettingsView, String> {
        let input = self.resolve(input);
        if input.enabled {
            Emby::new(&input.server, &input.api_key, &self.imdb_ratings)?
                .validate(client)
                .await?;
        }
        let directory = self.file.parent().ok_or("Emby 配置目录无效。")?;
        let mut state = self.state.lock().expect("Emby settings lock");
        let write = || -> io::Result<()> {
            fs::create_dir_all(directory)?;
            let mut file = tempfile::NamedTempFile::new_in(directory)?;
            serde_json::to_writer_pretty(&mut file, &input)?;
            file.as_file().sync_all()?;
            file.persist(&self.file)?;
            Ok(())
        };
        write().map_err(|_| "无法保存 Emby 设置。")?;
        *state = input;
        Ok(view(&state))
    }

    pub(crate) fn connection(&self) -> Option<Arc<Emby>> {
        let input = self.state.lock().expect("Emby settings lock");
        Emby::new(&input.server, &input.api_key, &self.imdb_ratings)
            .ok()
            .map(Arc::new)
    }

    #[cfg(feature = "web")]
    pub(crate) fn automatic_connection(&self, url: Option<&str>) -> Result<Arc<Emby>, String> {
        let input = self.state.lock().expect("Emby settings lock");
        let server = url.unwrap_or(&input.server);
        Emby::new(server, &input.api_key, &self.imdb_ratings).map(Arc::new)
    }

    pub(crate) fn active(&self) -> Option<Arc<Emby>> {
        let input = self.state.lock().expect("Emby settings lock");
        input
            .enabled
            .then(|| Emby::new(&input.server, &input.api_key, &self.imdb_ratings).ok())
            .flatten()
            .map(Arc::new)
    }
}

fn view(input: &EmbyInput) -> EmbySettingsView {
    EmbySettingsView {
        enabled: input.enabled,
        server: input.server.clone(),
        configured: !input.api_key.is_empty(),
    }
}
