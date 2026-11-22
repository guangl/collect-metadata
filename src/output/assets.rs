use std::{collections::HashSet, time::Duration};

use reqwest::{Client, Response, Url, header};
use serde_json::Value;

use super::settings::{OutputOptions, PortraitRole as Role};
use crate::providers::media::{
    bangumi,
    emby::{Emby, credit_role},
};

pub(super) struct Asset {
    pub(super) url: Url,
    pub(super) kind: &'static str,
    pub(super) person: Option<String>,
    from_emby: bool,
}

fn entries(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

pub(super) fn allowed(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && (matches!(
            host,
            "image.tmdb.org"
                | "static.tvmaze.com"
                | "lain.bgm.tv"
                | "upload.wikimedia.org"
                | "commons.wikimedia.org"
                | "www.javdatabase.com"
        ) || [
            "dmm.co.jp",
            "dmm.com",
            "jdbstatic.com",
            "javdatabase.com",
            "r18.dev",
        ]
        .iter()
        .any(|base| {
            host == *base
                || host
                    .strip_suffix(base)
                    .is_some_and(|part| part.ends_with('.'))
        }))
}

fn asset_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    allowed(&url).then_some(url)
}

fn tmdb_image(value: &Value) -> Option<Url> {
    let name = value.as_str()?.strip_prefix('/')?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    asset_url(&format!("https://image.tmdb.org/t/p/w185/{name}"))
}

pub(super) fn selected_assets(
    record: &Value,
    options: &OutputOptions,
    emby: Option<&Emby>,
) -> Vec<Asset> {
    let mut assets = Vec::new();
    let fields = &record["fields"];
    let sources = &record["sources"];
    if let Some(emby) = emby
        && let Some(source) = entries(sources).find(|source| source["provider"] == "Emby")
    {
        return emby_assets(emby, &source["data"], options);
    }
    let source_image = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            std::iter::once(&fields[*key])
                .chain(entries(sources).map(|source| &source["fields"][*key]))
                .filter_map(Value::as_str)
                .find_map(asset_url)
        })
    };
    for (enabled, kind, keys) in [
        (options.poster, "poster", &["poster_url", "cover_url"][..]),
        (options.backdrop, "backdrop", &["backdrop_url"][..]),
    ] {
        if enabled && let Some(url) = source_image(keys) {
            assets.push(Asset {
                url,
                kind,
                person: None,
                from_emby: false,
            });
        }
    }
    if options.stills {
        let urls = entries(&fields["episodes"])
            .filter_map(|episode| episode["still_url"].as_str())
            .chain(entries(sources).flat_map(|source| {
                entries(&source["fields"]["episodes"])
                    .filter_map(|episode| episode["still_url"].as_str())
                    .chain(still_urls(&source["data"]))
            }));
        let mut seen = HashSet::new();
        for url in urls
            .filter_map(asset_url)
            .filter(|url| seen.insert(url.clone()))
            .take(20)
        {
            assets.push(Asset {
                url,
                kind: "still",
                person: None,
                from_emby: false,
            });
        }
    }
    if !options.portraits.is_empty() {
        let mut by_role = HashSet::new();
        let mut portraits = entries(sources)
            .flat_map(portraits)
            .filter(|(role, url, name)| {
                options.portraits.contains(role)
                    && by_role.insert((role.code(), url.clone(), name.to_lowercase()))
            });
        assets.extend(portraits.by_ref().take(100).map(|(role, url, name)| Asset {
            url,
            kind: role.code(),
            person: Some(name),
            from_emby: false,
        }));
    }
    assets
}

fn emby_assets(emby: &Emby, data: &Value, options: &OutputOptions) -> Vec<Asset> {
    let mut result = Vec::new();
    let mut insert = |id: &Value, image_type, kind, person: Option<String>| {
        if let Some(url) = emby.image_url(id, image_type) {
            result.push(Asset {
                url,
                kind,
                person,
                from_emby: true,
            });
        }
    };
    let item = &data["item"];
    if item["ImageTags"]["Primary"].is_string() {
        if item["Type"] == "Episode" && options.stills {
            insert(&item["Id"], "Primary", "still", None);
        } else if options.poster {
            insert(&item["Id"], "Primary", "poster", None);
        }
    }
    if options.backdrop && entries(&item["BackdropImageTags"]).next().is_some() {
        insert(&item["Id"], "Backdrop", "backdrop", None);
    }
    if options.stills {
        for entry in entries(&data["children"])
            .filter(|item| item["Type"] == "Episode" && item["ImageTags"]["Primary"].is_string())
            .take(20)
        {
            insert(&entry["Id"], "Primary", "still", None);
        }
    }
    let roles = entries(&item["People"]).filter_map(|credit| {
        let role = match credit_role(item, credit) {
            "voice" => Role::Voice,
            "actor" => Role::Actor,
            "director" => Role::Director,
            "writer" => Role::Writer,
            _ => Role::Crew,
        };
        (options.portraits.contains(&role) && credit["PrimaryImageTag"].is_string())
            .then_some((credit, role))
    });
    for (credit, role) in roles.take(100) {
        insert(
            &credit["Id"],
            "Primary",
            role.code(),
            credit["Name"].as_str().map(str::to_owned),
        );
    }
    result
}

fn still_urls(data: &Value) -> impl Iterator<Item = &str> {
    entries(&data["gallery"])
        .filter_map(|image| {
            image["full_url"]
                .as_str()
                .or_else(|| image["thumbnail_url"].as_str())
        })
        .chain(
            entries(&data["_embedded"]["episodeswithspecials"])
                .filter_map(|ep| ep["image"]["original"].as_str()),
        )
}

fn voice_character(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        let text = text.to_ascii_lowercase();
        text.contains("(voice)")
            || text.contains("(voice:")
            || text.contains("配音")
            || text.contains("声优")
    })
}

fn crew_role(value: &str) -> Role {
    let job = value.to_ascii_lowercase();
    if matches!(
        job.as_str(),
        "director"
            | "series director"
            | "co-director"
            | "animation director"
            | "chief animation director"
            | "导演"
            | "监督"
    ) {
        Role::Director
    } else if matches!(
        job.as_str(),
        "writer"
            | "screenplay"
            | "teleplay"
            | "screenwriter"
            | "series composition"
            | "story"
            | "脚本"
            | "编剧"
            | "系列构成"
    ) {
        Role::Writer
    } else {
        Role::Crew
    }
}

fn portraits(source: &Value) -> Vec<(Role, Url, String)> {
    let data = &source["data"];
    let mut result = Vec::new();
    match source["provider"].as_str() {
        Some("TMDB") => {
            let animated = entries(&data["genres"]).any(|genre| genre["id"] == 16);
            let credits = if data["aggregate_credits"].is_object() {
                &data["aggregate_credits"]
            } else {
                &data["credits"]
            };
            for credit in entries(&credits["cast"]) {
                if let (Some(url), Some(name)) =
                    (tmdb_image(&credit["profile_path"]), credit["name"].as_str())
                {
                    let voice = animated
                        || voice_character(&credit["character"])
                        || entries(&credit["roles"])
                            .any(|role| voice_character(&role["character"]));
                    result.push((
                        if voice { Role::Voice } else { Role::Actor },
                        url,
                        name.to_owned(),
                    ));
                }
            }
            for credit in entries(&credits["crew"]) {
                if let (Some(url), Some(name)) =
                    (tmdb_image(&credit["profile_path"]), credit["name"].as_str())
                {
                    let jobs = std::iter::once(&credit["job"])
                        .chain(entries(&credit["jobs"]).map(|job| &job["job"]));
                    for role in jobs.filter_map(Value::as_str).map(crew_role) {
                        result.push((role, url.clone(), name.to_owned()));
                    }
                }
            }
        }
        Some("TVmaze") => {
            let embedded = &data["_embedded"];
            let image = |person: &Value| {
                person["image"]["original"]
                    .as_str()
                    .or_else(|| person["image"]["medium"].as_str())
                    .and_then(asset_url)
            };
            for credit in entries(&embedded["cast"]) {
                let person = &credit["person"];
                if let (Some(url), Some(name)) = (image(person), person["name"].as_str()) {
                    let role = if credit["voice"] == true {
                        Role::Voice
                    } else {
                        Role::Actor
                    };
                    result.push((role, url, name.to_owned()));
                }
            }
            for credit in entries(&embedded["crew"]) {
                let person = &credit["person"];
                if let (Some(url), Some(name), Some(job)) = (
                    image(person),
                    person["name"].as_str(),
                    credit["type"].as_str(),
                ) {
                    result.push((crew_role(job), url, name.to_owned()));
                }
            }
        }
        Some("Bangumi") => {
            let image = |person: &Value| {
                [&person["images"]["large"], &person["images"]["medium"]]
                    .into_iter()
                    .filter_map(Value::as_str)
                    .filter_map(asset_url)
                    .find(|url| url.host_str() == Some("lain.bgm.tv"))
            };
            let actor_role = if data["subject"]["type"] == 2 {
                Role::Voice
            } else {
                Role::Actor
            };
            for actor in
                entries(&data["characters"]).flat_map(|character| entries(&character["actors"]))
            {
                if let (Some(url), Some(name)) = (image(actor), actor["name"].as_str()) {
                    result.push((actor_role, url, name.to_owned()));
                }
            }
            for person in entries(&data["persons"]).filter(|person| person["type"] == 1) {
                if let (Some(url), Some(name), Some(job)) = (
                    image(person),
                    person["name"].as_str(),
                    person["relation"].as_str(),
                ) {
                    let role = if actor_role == Role::Actor && bangumi::is_cast(job) {
                        Role::Actor
                    } else {
                        crew_role(job)
                    };
                    result.push((role, url, name.to_owned()));
                }
            }
        }
        _ => {}
    }
    result
}

pub(super) async fn download(
    client: &Client,
    asset: &Asset,
    emby: Option<&Emby>,
) -> Option<(Vec<u8>, &'static str)> {
    if asset.from_emby {
        return image_bytes(emby?.image_response(client, &asset.url).await?).await;
    }
    image_bytes(public_response(client, &asset.url, "image/*", Duration::from_secs(10)).await?)
        .await
}

pub(super) async fn public_response(
    client: &Client,
    original: &Url,
    accept: &str,
    timeout: Duration,
) -> Option<Response> {
    let mut url = original.clone();
    for _ in 0..4 {
        if !allowed(&url) {
            return None;
        }
        let response = client
            .get(url.clone())
            .header(header::ACCEPT, accept)
            .timeout(timeout)
            .send()
            .await
            .ok()?;
        if response.status().is_redirection() {
            url = url
                .join(response.headers().get(header::LOCATION)?.to_str().ok()?)
                .ok()?;
            continue;
        }
        return (response.status().is_success() && response.url() == &url).then_some(response);
    }
    None
}

pub(crate) async fn image_bytes(mut response: Response) -> Option<(Vec<u8>, &'static str)> {
    const LIMIT: usize = 12 * 1024 * 1024;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if data.len() + chunk.len() > LIMIT {
            return None;
        }
        data.extend_from_slice(&chunk);
    }
    let extension = if data.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "webp"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "gif"
    } else {
        return None;
    };
    Some((data, extension))
}

#[cfg(test)]
#[path = "../tests/assets.rs"]
mod tests;
