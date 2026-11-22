use std::time::Duration;

use reqwest::{Client, Url, header::LOCATION};
use serde::Serialize;
use serde_json::Value;

use super::{Emby, MediaKind, list, work_kind};

#[derive(Serialize)]
pub struct ScrapeSourceLink {
    provider: &'static str,
    url: String,
}

impl Emby {
    pub(crate) async fn source_link_item(
        &self,
        client: &Client,
        id: &str,
    ) -> Result<Value, String> {
        self.item(
            client,
            id,
            "Movie,Series,Video",
            "Path,ProviderIds,ExternalUrls,HomePageUrl",
        )
        .await
    }
}

pub(crate) async fn work_source_links(
    client: &Client,
    item: &Value,
    sidecar_urls: &[String],
) -> Vec<ScrapeSourceLink> {
    let Some(kind) = work_kind(item) else {
        return Vec::new();
    };
    let tmdb_kind = if kind == MediaKind::Series {
        "tv"
    } else {
        "movie"
    };
    let ids = item["ProviderIds"].as_object();
    let provider_id = |provider: &str| {
        ids?.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(provider))
            .and_then(|(_, value)| value.as_str())
            .map(str::trim)
    };
    let urls: Vec<_> = list(&item["ExternalUrls"])
        .filter_map(|entry| entry["Url"].as_str())
        .chain(item["HomePageUrl"].as_str())
        .chain(sidecar_urls.iter().map(String::as_str))
        .filter_map(|url| Url::parse(url).ok())
        .collect();

    let mut links = Vec::new();
    for provider in ["TMDB", "Bangumi", "JavBus", "JavDB"] {
        let id = provider_id(provider);
        let id_path = id
            .and_then(|id| url_from_id(provider, tmdb_kind, id))
            .filter(|url| matches_provider(url, provider, tmdb_kind));
        let mut url = urls
            .iter()
            .filter(|url| matches_provider(url, provider, tmdb_kind))
            .filter(|url| {
                // The saved provider ID takes precedence over a stale external link.
                id_path.as_ref().is_none_or(|expected| {
                    let path = expected.path().trim_end_matches('/');
                    let actual = url.path().trim_end_matches('/');
                    actual == path
                        || provider == "TMDB"
                            && actual
                                .strip_prefix(path)
                                .is_some_and(|suffix| suffix.starts_with('-'))
                })
            })
            .max_by_key(|url| url.path().len())
            .cloned()
            .or(id_path);
        if provider == "TMDB"
            && let Some(page) = &url
            && page
                .path()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .is_some_and(numeric_id)
        {
            url = canonical_tmdb_page(client, page, tmdb_kind).await.or(url);
        }
        if let Some(mut url) = url {
            url.set_fragment(None);
            links.push(ScrapeSourceLink {
                provider,
                url: url.into(),
            });
        }
    }
    links
}

fn numeric_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id.parse::<u32>().is_ok_and(|id| id > 0)
}

fn alphanumeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn javbus_code(id: &str) -> bool {
    let Some((prefix, number)) = id.split_once(['-', '_']) else {
        return false;
    };
    alphanumeric_id(prefix)
        && alphanumeric_id(number)
        && number.bytes().any(|byte| byte.is_ascii_digit())
}

fn url_from_id(provider: &str, tmdb_kind: &str, id: &str) -> Option<Url> {
    if let Ok(url) = Url::parse(id) {
        return Some(url);
    }
    let url = match provider {
        "TMDB" if numeric_id(id) => format!("https://www.themoviedb.org/{tmdb_kind}/{id}"),
        "Bangumi" if numeric_id(id) => format!("https://bgm.tv/subject/{id}"),
        "JavBus" if javbus_code(id) => format!("https://www.javbus.com/{id}"),
        "JavDB" if alphanumeric_id(id) => format!("https://javdb.com/v/{id}"),
        _ => return None,
    };
    Url::parse(&url).ok()
}

fn matches_provider(url: &Url, provider: &str, tmdb_kind: &str) -> bool {
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let path = url.path().trim_end_matches('/');
    match (provider, url.host_str()) {
        ("TMDB", Some("www.themoviedb.org" | "themoviedb.org")) => path
            .strip_prefix(if tmdb_kind == "tv" { "/tv/" } else { "/movie/" })
            .is_some_and(|part| {
                let (id, slug) = part.split_once('-').unwrap_or((part, ""));
                numeric_id(id) && !slug.contains('/')
            }),
        ("Bangumi", Some("bgm.tv" | "bangumi.tv" | "chii.in")) => {
            path.strip_prefix("/subject/").is_some_and(numeric_id)
        }
        ("JavBus", Some("javbus.com" | "www.javbus.com")) => {
            path.strip_prefix('/').is_some_and(javbus_code)
        }
        ("JavDB", Some("javdb.com" | "www.javdb.com")) => {
            path.strip_prefix("/v/").is_some_and(alphanumeric_id)
        }
        _ => false,
    }
}

async fn canonical_tmdb_page(client: &Client, page: &Url, kind: &str) -> Option<Url> {
    let mut endpoint = Url::parse("https://www.themoviedb.org").ok()?;
    endpoint.set_path(page.path().trim_end_matches('/'));
    let response = client
        .head(endpoint.clone())
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    if !response.status().is_redirection() {
        return None;
    }
    let target = endpoint
        .join(response.headers().get(LOCATION)?.to_str().ok()?)
        .ok()?;
    let suffix = target.path().strip_prefix(endpoint.path())?;
    (matches_provider(&target, "TMDB", kind) && suffix.starts_with('-')).then_some(target)
}
