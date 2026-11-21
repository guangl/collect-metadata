use std::{collections::BTreeMap, env, fs, path::PathBuf, sync::RwLock, time::Duration};

use futures_util::{StreamExt, stream};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) mod records;

pub(crate) const DEFAULT: &str = "zh-CN";
const MAX_LANGUAGES: usize = 8;

pub struct LanguageSettings {
    path: Option<PathBuf>,
    languages: RwLock<Vec<String>>,
    translator: Option<Translator>,
}

#[derive(Serialize)]
pub struct LanguageSettingsView {
    pub languages: Vec<String>,
    pub translation_configured: bool,
}

impl LanguageSettings {
    pub fn load(path: Option<PathBuf>) -> Result<Self, String> {
        let languages = match path.as_ref().map(fs::read).transpose() {
            Ok(Some(bytes)) => normalize_languages(
                serde_json::from_slice(&bytes).map_err(|_| "元数据语言配置文件格式无效。")?,
            )?,
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err("无法读取元数据语言配置。".into());
            }
            _ => vec![DEFAULT.into()],
        };
        Ok(Self {
            path,
            languages: RwLock::new(languages),
            translator: Translator::from_environment(),
        })
    }

    pub fn view(&self) -> LanguageSettingsView {
        LanguageSettingsView {
            languages: self
                .languages
                .read()
                .expect("language settings lock")
                .clone(),
            translation_configured: self.translator.is_some(),
        }
    }

    pub fn save(&self, languages: Vec<String>) -> Result<LanguageSettingsView, String> {
        let languages = normalize_languages(languages)?;
        let mut current = self.languages.write().expect("language settings lock");
        if let Some(path) = &self.path {
            let persist = || -> std::io::Result<()> {
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new("."));
                fs::create_dir_all(parent)?;
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                serde_json::to_writer(&mut file, &languages)?;
                file.as_file().sync_all()?;
                file.persist(path)?;
                Ok(())
            };
            persist().map_err(|_| "无法保存元数据语言配置。")?;
        }
        *current = languages;
        drop(current);
        Ok(self.view())
    }

    pub(crate) fn targets(&self, emby_language: Option<&str>) -> Vec<String> {
        emby_language
            .and_then(normalize_locale)
            .map(|language| vec![language])
            .unwrap_or_else(|| self.view().languages)
    }

    pub(crate) async fn localize(
        &self,
        client: &Client,
        targets: &[String],
        texts: &Texts,
        official: &Translations,
        cached: &Translations,
    ) -> Translations {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(22);
        let targets = targets.to_vec();
        stream::iter(targets)
            .map(|target| async move {
                let native = official.get(&target);
                let previous = cached.get(&target);
                let mut localized = native.cloned().unwrap_or_default();
                let mut missing = Vec::new();
                for (key, text) in texts {
                    if localized.contains_key(key) {
                        continue;
                    }
                    if let Some(existing) = previous.and_then(|fields| fields.get(key)) {
                        localized.insert(key.clone(), existing.clone());
                    } else if already_localized(text, &target) {
                        localized.insert(key.clone(), text.clone());
                    } else if text.chars().count() <= 4000 {
                        missing.push((key, text));
                    }
                }
                if let Some(translator) = &self.translator {
                    // Bound time and provider load for large series. Keep original text for
                    // any fields not returned successfully within this request's budget.
                    let mut offset = 0;
                    let mut size = 0;
                    for end in 0..=missing.len() {
                        let next = missing.get(end).map_or(0, |(_, text)| text.len());
                        if end == missing.len() || end - offset == 24 || size + next > 16_000 {
                            if end > offset {
                                let batch = &missing[offset..end];
                                let Ok(Some(translated)) = tokio::time::timeout_at(
                                    deadline,
                                    translator.translate(client, &target, batch),
                                )
                                .await
                                else {
                                    break;
                                };
                                localized.extend(translated);
                            }
                            offset = end;
                            size = 0;
                        }
                        size += next;
                    }
                }
                (target, localized)
            })
            .buffer_unordered(4)
            .filter(|(_, fields)| std::future::ready(!fields.is_empty()))
            .collect()
            .await
    }
}

pub(crate) type Texts = BTreeMap<String, String>;
pub(crate) type Translations = BTreeMap<String, Texts>;

fn normalize_languages(input: Vec<String>) -> Result<Vec<String>, String> {
    if input.len() > MAX_LANGUAGES {
        return Err(format!("最多选择 {MAX_LANGUAGES} 种元数据语言。"));
    }
    let mut languages = Vec::new();
    for tag in input {
        let tag = normalize_locale(&tag).ok_or("无效的语言代码，例如 zh-CN、ja 或 en-US。")?;
        if !languages.contains(&tag) {
            languages.push(tag);
        }
    }
    if languages.is_empty() {
        languages.push(DEFAULT.into());
    }
    Ok(languages)
}

pub(crate) fn normalize_locale(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase().replace('_', "-");
    let mut parts = value.split('-');
    let base = parts.next()?;
    if matches!(base, "und" | "zxx" | "mul") {
        return None;
    }
    let base = if base == "chi" { "zh" } else { base };
    let language = match base.len() {
        2 => isolang::Language::from_639_1(base)?,
        3 => isolang::Language::from_639_3(base)?,
        _ => return None,
    };
    let base = language.to_639_1().unwrap_or_else(|| language.to_639_3());
    let mut script = None;
    let mut region = None;
    for part in parts {
        if part.len() == 4
            && script.is_none()
            && region.is_none()
            && part.bytes().all(|c| c.is_ascii_lowercase())
        {
            script = Some(part);
        } else if region.is_none()
            && ((part.len() == 2 && part.bytes().all(|c| c.is_ascii_lowercase()))
                || (part.len() == 3 && part.bytes().all(|c| c.is_ascii_digit())))
        {
            region = Some(part);
        } else {
            return None;
        }
    }
    if base == "zh" {
        return Some(
            match (script, region) {
                (Some("hant"), Some("hk")) | (None, Some("hk")) => "zh-HK",
                (Some("hant"), _) | (None, Some("tw" | "mo")) => "zh-TW",
                _ => "zh-CN",
            }
            .into(),
        );
    }
    let mut tag = base.to_owned();
    if let Some(script) = script {
        tag.push('-');
        tag.push_str(&script[..1].to_ascii_uppercase());
        tag.push_str(&script[1..]);
    }
    if let Some(region) = region {
        tag.push('-');
        tag.push_str(&region.to_ascii_uppercase());
    }
    Some(tag)
}

pub(crate) fn language_base(locale: &str) -> &str {
    locale.split('-').next().unwrap_or(locale)
}

fn already_localized(text: &str, target: &str) -> bool {
    let base = language_base(target);
    // A language detector cannot infer regional spelling or distinguish Chinese scripts.
    // For Chinese, trust explicit provider translations or let the backend normalize it.
    if base == "zh" {
        return false;
    }
    whatlang::detect(text).is_some_and(|info| {
        info.is_reliable()
            && isolang::Language::from_639_3(info.lang().code()).is_some_and(|language| {
                base == language.to_639_1().unwrap_or_else(|| language.to_639_3())
            })
    })
}

struct Translator {
    endpoint: Url,
    token: String,
    permits: tokio::sync::Semaphore,
    retry_after: std::sync::Mutex<Option<tokio::time::Instant>>,
}

#[derive(Deserialize)]
struct TranslationResult {
    translations: Vec<Option<String>>,
}

impl Translator {
    fn from_environment() -> Option<Self> {
        let endpoint = Url::parse(env::var("METADATA_TRANSLATION_URL").ok()?.trim()).ok()?;
        let token = env::var("METADATA_TRANSLATION_TOKEN")
            .ok()?
            .trim()
            .to_owned();
        (matches!(endpoint.scheme(), "http" | "https")
            && endpoint.host().is_some()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.fragment().is_none()
            && !token.is_empty())
        .then(|| Self::new(endpoint, token))
    }

    fn new(endpoint: Url, token: String) -> Self {
        Self {
            endpoint,
            token,
            permits: tokio::sync::Semaphore::new(1),
            retry_after: std::sync::Mutex::new(None),
        }
    }

    async fn translate(
        &self,
        client: &Client,
        target: &str,
        batch: &[(&String, &String)],
    ) -> Option<Texts> {
        let _permit = self.permits.acquire().await.ok()?;
        if self
            .retry_after
            .lock()
            .ok()?
            .is_some_and(|retry| tokio::time::Instant::now() < retry)
        {
            return None;
        }
        let result = self.request(client, target, batch).await;
        if result.is_none() {
            *self.retry_after.lock().ok()? =
                Some(tokio::time::Instant::now() + Duration::from_secs(30));
        }
        result
    }

    async fn request(
        &self,
        client: &Client,
        target: &str,
        batch: &[(&String, &String)],
    ) -> Option<Texts> {
        let result = client.post(self.endpoint.clone()).bearer_auth(&self.token)
            .timeout(Duration::from_secs(18))
            .json(&json!({"target_language":target,"texts":batch.iter().map(|(_, text)| text).collect::<Vec<_>>()}))
            .send().await.ok()?.error_for_status().ok()?;
        const MAX_RESPONSE: usize = 160_000;
        let mut body = Vec::new();
        let mut chunks = result.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.ok()?;
            if body.len() + chunk.len() > MAX_RESPONSE {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        let result: TranslationResult = serde_json::from_slice(&body).ok()?;
        if result.translations.len() != batch.len() {
            return None;
        }
        Some(
            batch
                .iter()
                .zip(result.translations)
                .filter_map(|((key, _), value)| {
                    let value = value?;
                    let value = value.trim();
                    (!value.is_empty() && value.chars().count() <= 8000)
                        .then(|| ((*key).clone(), value.to_owned()))
                })
                .collect(),
        )
    }
}

pub(crate) fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_receives_only_missing_text_and_failures_do_not_label_originals() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/translate",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = std::thread::spawn(move || {
            for (locale, status, response) in [
                ("zh-CN", "200 OK", r#"{"translations":["中文简介"]}"#),
                ("ja", "503 Service Unavailable", ""),
            ] {
                let (mut connection, _) = listener.accept().unwrap();
                connection
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut input = String::new();
                let (header, body) = loop {
                    let mut buffer = [0; 1024];
                    let len = connection.read(&mut buffer).unwrap();
                    assert!(len > 0);
                    input.push_str(std::str::from_utf8(&buffer[..len]).unwrap());
                    if let Some((header, body)) = input.split_once("\r\n\r\n") {
                        let size = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        if body.len() >= size {
                            break (header, body);
                        }
                    }
                };
                assert!(header.contains("authorization: Bearer fixture"));
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"target_language":locale,"texts":["An English synopsis."]})
                );
                write!(connection, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
        });
        let settings = LanguageSettings {
            path: None,
            languages: RwLock::new(vec![]),
            translator: Some(Translator::new(endpoint, "fixture".into())),
        };
        let client = Client::builder().no_proxy().build().unwrap();
        let texts = Texts::from([
            ("title".into(), "Original Title".into()),
            ("description".into(), "An English synopsis.".into()),
        ]);
        let official = Translations::from([
            (
                "zh-CN".into(),
                Texts::from([("title".into(), "官方片名".into())]),
            ),
            (
                "ja".into(),
                Texts::from([("title".into(), "公式名".into())]),
            ),
        ]);
        let variants = settings
            .localize(
                &client,
                &["zh-CN".into(), "ja".into()],
                &texts,
                &official,
                &Translations::new(),
            )
            .await;
        assert_eq!(variants["zh-CN"]["title"], "官方片名");
        assert_eq!(variants["zh-CN"]["description"], "中文简介");
        assert!(!variants["ja"].contains_key("description"));
        server.join().unwrap();
    }

    #[test]
    fn priority_defaults_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("language.json");
        let settings = LanguageSettings::load(Some(path.clone())).unwrap();
        assert_eq!(settings.targets(None), ["zh-CN"]);
        settings
            .save(vec!["JA".into(), "zh-hant".into(), "ja".into()])
            .unwrap();
        assert_eq!(settings.targets(Some("en_gb")), ["en-GB"]);
        assert_eq!(settings.targets(Some("")), ["ja", "zh-TW"]);
        assert!(settings.save(vec!["bad%tag".into()]).is_err());
        assert_eq!(
            LanguageSettings::load(Some(path)).unwrap().targets(None),
            ["ja", "zh-TW"]
        );
    }
}
