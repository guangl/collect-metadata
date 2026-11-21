pub(crate) mod av;
pub(crate) mod media;

use reqwest::{Client, StatusCode, Url, header};
use scraper::ElementRef;
use serde_json::Value;

use crate::collect::{Fields, Source};

#[derive(Clone)]
enum Failure {
    NotFound,
    Unavailable(String),
}

impl Failure {
    fn format_changed() -> Self {
        Self::Unavailable("页面格式变化，或来源要求登录。".into())
    }

    fn description(&self) -> &str {
        match self {
            Self::NotFound => "来源没有找到此条目。",
            Self::Unavailable(message) => message,
        }
    }
}

async fn request(client: &Client, url: &Url, accept: &str) -> Result<String, Failure> {
    request_with_auth(client, url, accept, None).await
}

async fn request_with_auth(
    client: &Client,
    url: &Url,
    accept: &str,
    authorization: Option<&header::HeaderValue>,
) -> Result<String, Failure> {
    let unavailable = |error: reqwest::Error| {
        Failure::Unavailable(
            if error.is_timeout() {
                "请求超时。"
            } else {
                "无法连接来源。"
            }
            .into(),
        )
    };

    let mut current = url.clone();
    for _ in 0..4 {
        let mut request = client.get(current.clone()).header(header::ACCEPT, accept);
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value.clone());
        }
        let mut response = request.send().await.map_err(unavailable)?;
        let status = response.status();
        match status {
            StatusCode::NOT_FOUND => return Err(Failure::NotFound),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(Failure::Unavailable(format!(
                    "来源限制访问（HTTP {status}）。"
                )));
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(Failure::Unavailable(
                    "来源限制请求频率（HTTP 429）。".into(),
                ));
            }
            _ if status.is_redirection() => {
                current = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| current.join(value.to_str().ok()?).ok())
                    .filter(|next| next.scheme() == "https" && next.host_str() == url.host_str())
                    .ok_or(Failure::format_changed())?;
            }
            _ if status.is_success() => {
                let limit = 4 * 1024 * 1024;
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(unavailable)? {
                    if bytes.len() + chunk.len() > limit {
                        return Err(Failure::Unavailable("来源响应超过大小限制。".into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                return String::from_utf8(bytes)
                    .map_err(|_| Failure::Unavailable("来源返回了无效的 UTF-8。".into()));
            }
            _ => return Err(Failure::Unavailable(format!("来源返回 HTTP {status}。"))),
        }
    }
    Err(Failure::Unavailable("来源重定向次数过多。".into()))
}

impl Source {
    fn found(provider: &'static str, url: &Url, fields: Fields, data: Value) -> Self {
        Self {
            provider,
            url: url.to_string(),
            status: "ok",
            note: None,
            fields,
            data,
        }
    }

    fn failure(provider: &'static str, url: &Url, error: Failure) -> Self {
        Self {
            provider,
            url: url.to_string(),
            status: match error {
                Failure::NotFound => "not_found",
                Failure::Unavailable(_) => "unavailable",
            },
            note: Some(error.description().to_owned()),
            fields: Fields::new(),
            data: Value::Null,
        }
    }
}

fn field_map<const N: usize>(values: [(&'static str, Value); N]) -> Fields {
    values
        .into_iter()
        .filter(|(_, value)| match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(array) => !array.is_empty(),
            _ => true,
        })
        .collect()
}

fn text(node: ElementRef<'_>) -> String {
    node.text()
        .flat_map(|value| value.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}
