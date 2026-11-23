use std::{collections::HashMap, sync::Arc, time::Duration};

use reqwest::{Client, Method, Response, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use super::MediaKind;
use crate::{collect::Source, diagnostics, providers::field_map};

#[cfg(feature = "web")]
mod automatic;
mod catalog;
#[cfg(feature = "web")]
mod credits;
mod settings;
pub(crate) mod source_links;

pub use catalog::{
    EmbyCredit, EmbyLibraryPreview, EmbyPreviewImage, EmbySeriesPreview, EmbyWorkPreview,
};
#[cfg(feature = "web")]
pub(crate) use credits::{missing_people, record_people, tmdb_people};
pub use settings::{EmbyInput, EmbySettings, EmbySettingsView};
pub use source_links::ScrapeSourceLink;

const FIELDS: &str = concat!(
    "Path,Overview,People,ProviderIds,Genres,Studios,Tags,TagItems,Taglines,OriginalTitle,SortName,",
    "ProductionLocations,ProductionYear,CommunityRating,CriticRating,OfficialRating,",
    "CustomRating,MediaSources,MediaStreams,Chapters,ExternalUrls,HomePageUrl,RemoteTrailers"
);
// Emby omits these properties from /Items unless requested, even for a single ID.
const EPISODE_DETAIL_FIELDS: &str = concat!(
    "PremiereDate,ParentId,DateCreated,DateModified,Settings,FileName,VideoCodec,AudioCodec,",
    "Width,Height,PrimaryImageAspectRatio,AverageFrameRate,RealFrameRate,MimeType,PartCount,",
    "Collections,LocalTrailerCount,SpecialFeatureCount,SeriesStudio,Prefix,Etag,",
    "PresentationUniqueKey,DisplayPreferencesId,CanDelete,CanDownload,DisplayOrder,",
    "SortIndexNumber,SortParentIndexNumber,PreferredMetadataLanguage,",
    "PreferredMetadataCountryCode,LocationType",
);
pub(crate) const EPISODE_QUERY_PREFIX: &str = "emby-episode:";
const MAX_RESPONSE: usize = 12 * 1024 * 1024;

#[cfg(feature = "web")]
pub(crate) fn has_provider_id(item: &Value, provider: &str) -> bool {
    provider_id(item, provider).is_some()
}

#[cfg(feature = "web")]
pub(crate) fn provider_id<'a>(item: &'a Value, provider: &str) -> Option<&'a str> {
    item["ProviderIds"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.eq_ignore_ascii_case(provider))
        .filter_map(|(_, id)| id.as_str().map(str::trim))
        .find(|id| !id.is_empty())
}

#[derive(Serialize)]
pub struct EmbyLibrary {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct ImdbRating {
    value: f64,
    count: u64,
}

pub(crate) struct Emby {
    base: Url,
    authorization: HeaderValue,
    imdb_ratings: Arc<HashMap<String, ImdbRating>>,
}

impl Emby {
    fn new(
        server: &str,
        api_key: &str,
        imdb_ratings: &Arc<HashMap<String, ImdbRating>>,
    ) -> Result<Self, String> {
        let mut base = Url::parse(server).map_err(|_| "请输入有效的 Emby 服务地址。")?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("请输入 Emby 的 HTTP 或 HTTPS 服务地址。".into());
        }
        let original = base.path().trim_end_matches('/');
        let path = if original.is_empty() {
            "/emby/".to_owned()
        } else {
            format!("{original}/")
        };
        base.set_path(&path);
        if api_key.is_empty() || api_key.len() > 512 {
            return Err("请填写 Emby API 密钥。".into());
        }
        let mut authorization =
            HeaderValue::from_str(api_key).map_err(|_| "Emby API 密钥格式无效。")?;
        authorization.set_sensitive(true);
        Ok(Self {
            base,
            authorization,
            imdb_ratings: Arc::clone(imdb_ratings),
        })
    }

    fn imdb_rating(&self, item: &Value) -> Option<ImdbRating> {
        self.imdb_ratings
            .get(item["ProviderIds"]["Imdb"].as_str()?)
            .copied()
            .filter(|rating| (1.0..=10.0).contains(&rating.value) && rating.count > 0)
    }

    pub(crate) fn server_address(&self) -> &str {
        self.base.as_str()
    }

    fn endpoint(&self, path: &str) -> Url {
        self.base.join(path).expect("static Emby API path")
    }

    async fn response(&self, client: &Client, url: Url) -> Result<Response, String> {
        self.timed_response(client, Method::GET, url, Duration::from_secs(10))
            .await
    }

    async fn timed_response(
        &self,
        client: &Client,
        method: Method,
        url: Url,
        timeout: Duration,
    ) -> Result<Response, String> {
        let response = client
            .request(method, url.clone())
            .timeout(timeout)
            .header("X-Emby-Token", self.authorization.clone())
            .send()
            .await
            .map_err(|error| {
                let reason = diagnostics::http_reason(&error);
                diagnostics::warn("emby_request_failed", json!({"reason": reason}));
                if error.is_timeout() {
                    "Emby 请求超时。请检查 Emby 负载和应用到 Emby 的网络。"
                } else {
                    "无法连接 Emby。请确认应用能访问设置中的 Emby 地址。"
                }
            })?;
        if !response.status().is_success() || response.url() != &url {
            let status = response.status();
            if status != reqwest::StatusCode::NOT_FOUND {
                diagnostics::warn("emby_request_failed", json!({"status": status.as_u16()}));
            }
            let hint = match status.as_u16() {
                401 | 403 => "请在「家庭 Emby」中测试 API 密钥和权限。",
                404 => "请检查服务地址、代理子路径或媒体条目是否存在。",
                301..=399 => "请填写可直接访问的 Emby 地址，确认协议和代理子路径。",
                _ => "请查看 Emby 服务端日志。",
            };
            return Err(format!("Emby 请求失败（HTTP {}）。{hint}", status.as_u16()));
        }
        Ok(response)
    }

    async fn items(&self, client: &Client, filters: &[(&str, &str)]) -> Result<Value, String> {
        let mut url = self.endpoint("Items");
        url.query_pairs_mut()
            .append_pair("EnableUserData", "false")
            .append_pair("AddApiKeyToDirectStreamUrl", "false")
            .extend_pairs(filters);
        self.read_json(client, url).await
    }

    async fn read_json(&self, client: &Client, url: Url) -> Result<Value, String> {
        let mut response = self.response(client, url).await?;
        let mut data = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "无法读取 Emby 返回的资料。")?
        {
            if data.len() + chunk.len() > MAX_RESPONSE {
                return Err("Emby 单页数据超过大小限制。".into());
            }
            data.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&data).map_err(|_| "Emby 返回的资料格式无效。".into())
    }

    pub(crate) async fn libraries(&self, client: &Client) -> Result<Vec<EmbyLibrary>, String> {
        let result = self
            .read_json(client, self.endpoint("Library/MediaFolders"))
            .await?;
        Ok(list(&result["Items"])
            .filter_map(|item| {
                let id = valid_id(&item["Id"])?;
                let name = item["Name"].as_str()?;
                Some(EmbyLibrary {
                    id: id.to_owned(),
                    name: name.to_owned(),
                })
            })
            .collect())
    }

    pub(crate) async fn library(&self, client: &Client, id: &str) -> Result<EmbyLibrary, String> {
        self.libraries(client)
            .await?
            .into_iter()
            .find(|library| library.id == id)
            .ok_or_else(|| "没有找到此 Emby 媒体库。".into())
    }

    #[cfg(feature = "web")]
    pub(crate) async fn refresh_sidecars(&self, client: &Client, id: &str) -> Result<(), String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let mut url = self.endpoint(&format!("Items/{id}/Refresh"));
        url.query_pairs_mut()
            .append_pair("Recursive", "false")
            .append_pair("MetadataRefreshMode", "FullRefresh")
            .append_pair("ImageRefreshMode", "FullRefresh")
            .append_pair("ReplaceAllMetadata", "false")
            .append_pair("ReplaceAllImages", "false");
        self.timed_response(client, Method::POST, url, Duration::from_secs(10))
            .await?;
        Ok(())
    }

    pub(crate) async fn refresh_metadata(
        &self,
        client: &Client,
        id: &str,
        recursive: bool,
        overwrite: bool,
        replace_images: bool,
    ) -> Result<(), String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let mut url = self.endpoint(&format!("Items/{id}/Refresh"));
        url.query_pairs_mut()
            .append_pair("Recursive", if recursive { "true" } else { "false" })
            .append_pair("MetadataRefreshMode", "FullRefresh")
            .append_pair(
                "ReplaceAllMetadata",
                if overwrite { "true" } else { "false" },
            )
            .append_pair(
                "ImageRefreshMode",
                if replace_images || !overwrite {
                    "FullRefresh"
                } else {
                    "Default"
                },
            )
            .append_pair(
                "ReplaceAllImages",
                if replace_images { "true" } else { "false" },
            );
        self.timed_response(client, Method::POST, url, Duration::from_secs(10))
            .await?;
        Ok(())
    }

    async fn validate(&self, client: &Client) -> Result<(), String> {
        let data = self
            .items(client, &[("Recursive", "true"), ("Limit", "1")])
            .await?;
        data["Items"]
            .is_array()
            .then_some(())
            .ok_or("Emby 未返回可读取的媒体库。".into())
    }

    pub(crate) async fn find(
        &self,
        client: &Client,
        kind: MediaKind,
        query: &str,
        year: Option<u16>,
    ) -> Option<(String, Source)> {
        let item_type = match kind {
            MediaKind::Series => "Series",
            MediaKind::Movie => "Movie,Video",
            MediaKind::Av => "Movie",
        };
        if let Some(id) = query.strip_prefix("emby:") {
            return self
                .details(client, id, item_type)
                .await
                .map(|source| (id.to_owned(), source));
        }
        if let Some(id) = query.strip_prefix(EPISODE_QUERY_PREFIX) {
            if kind != MediaKind::Series {
                return None;
            }
            return self
                .details(client, id, "Episode")
                .await
                .map(|source| (id.to_owned(), source));
        }
        let mut params = vec![
            ("Recursive", "true"),
            ("IncludeItemTypes", item_type),
            ("Limit", "60"),
            (
                "Fields",
                "Path,ProviderIds,OriginalTitle,SortName,ProductionYear",
            ),
        ];
        let explicit = super::tmdb_id_query(query)
            .map(|id| ("Tmdb", id))
            .or_else(|| super::bangumi::id_query(query).map(|id| ("Bangumi", id)));
        let provider_id = explicit.map(|(provider, id)| format!("{provider}.{id}"));
        if let Some(id) = &provider_id {
            params.push(("AnyProviderIdEquals", id));
        } else {
            params.push(("SearchTerm", query));
        }
        let candidates = self.items(client, &params).await.ok()?;
        let name = normalize(query);
        let mut exact = list(&candidates["Items"]).filter(|item| {
            if !matches_kind(item, kind)
                || year.is_some_and(|year| item["ProductionYear"].as_u64() != Some(u64::from(year)))
            {
                return false;
            }
            if let Some((provider, id)) = explicit {
                return item["ProviderIds"][provider].as_str() == Some(id);
            }
            ["Name", "OriginalTitle", "SortName"]
                .iter()
                .filter_map(|field| item[*field].as_str())
                .any(|title| normalize(title) == name)
        });
        let id = valid_id(&exact.next()?["Id"])?.to_owned();
        if exact.next().is_some() {
            return None;
        }
        self.details(client, &id, item_type)
            .await
            .map(|source| (id, source))
    }

    pub(crate) async fn scrape_title(
        &self,
        client: &Client,
        id: &str,
        item_type: &str,
    ) -> Result<String, String> {
        let item = self.item(client, id, item_type, "").await?;
        let title = nonempty_text(&item["Name"]).ok_or("Emby 条目缺少标题。")?;
        if item_type == "Episode"
            && let Some(series) = nonempty_text(&item["SeriesName"])
        {
            return Ok(format!("{series} · {title}"));
        }
        Ok(title)
    }

    pub(crate) async fn series_provider_id(
        &self,
        client: &Client,
        episode: &Value,
        provider: &str,
    ) -> Option<String> {
        let series = self
            .item(
                client,
                valid_id(&episode["SeriesId"])?,
                "Series",
                "ProviderIds",
            )
            .await
            .ok()?;
        nonempty_text(&series["ProviderIds"][provider])
    }

    pub(crate) async fn item(
        &self,
        client: &Client,
        id: &str,
        item_type: &str,
        fields: &str,
    ) -> Result<Value, String> {
        self.find_item(client, id, item_type, fields)
            .await?
            .ok_or_else(|| "未找到指定类型的 Emby 条目。".into())
    }

    async fn find_item(
        &self,
        client: &Client,
        id: &str,
        item_type: &str,
        fields: &str,
    ) -> Result<Option<Value>, String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let mut page = self
            .items(client, &[("Ids", id), ("Fields", fields)])
            .await?;
        let items = page["Items"]
            .as_array_mut()
            .ok_or("Emby 返回的条目格式无效。")?;
        Ok(items
            .iter_mut()
            .find(|item| {
                let matches = match item_type {
                    "Movie,Video" => matches_kind(item, MediaKind::Movie),
                    "Movie,Series,Video" => work_kind(item).is_some(),
                    _ => item["Type"] == item_type,
                };
                valid_id(&item["Id"]) == Some(id) && matches
            })
            .map(Value::take))
    }

    #[cfg(feature = "web")]
    pub(crate) async fn automatic_item(
        &self,
        client: &Client,
        id: &str,
        item_type: &str,
    ) -> Result<Option<Source>, String> {
        let requested = format!(
            "{FIELDS},PremiereDate,Settings,LocationType,ParentId,DateCreated,DateLastSaved"
        );
        let Some(item) = self.find_item(client, id, item_type, &requested).await? else {
            return Ok(None);
        };
        let fields = if item_type == "Episode" {
            episode_fields(&item)
        } else {
            fields(&item, &[])
        };
        let source_url = self.endpoint(&format!("web/index.html#!/item?id={id}"));
        Ok(Some(Source::found(
            "Emby",
            &source_url,
            fields,
            json!({"item":item}),
        )))
    }

    async fn details(&self, client: &Client, id: &str, item_type: &str) -> Option<Source> {
        let (item, extended) = if item_type == "Episode" {
            let fields = format!("{FIELDS},{EPISODE_DETAIL_FIELDS}");
            match self.item(client, id, item_type, &fields).await {
                Ok(item) => (item, true),
                Err(_) => (self.item(client, id, item_type, FIELDS).await.ok()?, false),
            }
        } else {
            (self.item(client, id, item_type, FIELDS).await.ok()?, true)
        };
        nonempty_text(&item["Name"])?;
        let (children, complete) = if item_type == "Series" {
            self.series_children(client, id, &format!("{FIELDS},PremiereDate"))
                .await
                .unwrap_or_default()
        } else {
            (Vec::new(), true)
        };
        let mut details = if item_type == "Episode" {
            episode_fields(&item)
        } else {
            fields(&item, &children)
        };
        if let Some(rating) = self.imdb_rating(&item) {
            details.insert(
                "imdb_rating",
                json!({"value":rating.value,"scale":10,"count":rating.count}),
            );
        }
        let source_url = self.endpoint(&format!("web/index.html#!/item?id={id}"));
        let mut source = Source::found(
            "Emby",
            &source_url,
            details,
            json!({"item":item,"children":children}),
        );
        if !extended {
            source.status = "partial";
            source.note = Some("已读取 Emby 分集基本资料，扩展字段暂不可用。".into());
        } else if !complete {
            source.status = "partial";
            source.note = Some("已读取 Emby 作品资料，部分季集资料暂不可用。".into());
        }
        Some(source)
    }

    async fn series_children(
        &self,
        client: &Client,
        id: &str,
        fields: &str,
    ) -> Result<(Vec<Value>, bool), String> {
        let mut children = Vec::new();
        loop {
            let page = self
                .items(
                    client,
                    &[
                        ("ParentId", id),
                        ("Recursive", "true"),
                        ("IncludeItemTypes", "Season,Episode"),
                        ("Fields", fields),
                        ("EnableImages", "true"),
                        ("StartIndex", &children.len().to_string()),
                        ("Limit", "100"),
                    ],
                )
                .await
                .and_then(|mut page| {
                    let total = page["TotalRecordCount"].as_u64();
                    match page["Items"].take() {
                        Value::Array(items) => Ok((items, total)),
                        _ => Err("Emby 返回的分集格式无效。".into()),
                    }
                });
            let (items, total) = match page {
                Ok(page) => page,
                Err(error) if children.is_empty() => return Err(error),
                Err(_) => return Ok((children, false)),
            };
            if items.is_empty() {
                return Ok((children, true));
            }
            let last_page = total.map_or(items.len() < 100, |total| {
                (children.len() + items.len()) as u64 >= total
            });
            children.extend(items);
            if last_page || children.len() >= 10_000 {
                return Ok((children, last_page));
            }
        }
    }

    pub(crate) fn image_url(&self, id: &Value, image_type: &str) -> Option<Url> {
        let id = valid_id(id)?;
        matches!(image_type, "Primary" | "Backdrop" | "Logo" | "Thumb")
            .then(|| self.endpoint(&format!("Items/{id}/Images/{image_type}")))
    }

    pub(crate) fn owns_source(&self, source: &Value) -> bool {
        valid_id(&source["data"]["item"]["Id"]).is_some_and(|id| {
            source["url"]
                == self
                    .endpoint(&format!("web/index.html#!/item?id={id}"))
                    .as_str()
        })
    }

    pub(crate) async fn theme_media(
        &self,
        client: &Client,
        id: &Value,
        audio: bool,
    ) -> Result<Vec<Value>, String> {
        let id = valid_id(id).ok_or("Emby 条目 ID 无效。")?;
        let endpoint = if audio { "ThemeSongs" } else { "ThemeVideos" };
        let mut url = self.endpoint(&format!("Items/{id}/{endpoint}"));
        url.query_pairs_mut()
            .append_pair("InheritFromParent", "false")
            .append_pair("Fields", "Path,MediaSources")
            .append_pair("AddApiKeyToDirectStreamUrl", "false");
        self.read_json(client, url).await?["Items"]
            .as_array()
            .cloned()
            .ok_or_else(|| "Emby 未返回主题媒体列表。".into())
    }

    pub(crate) async fn extras_user(&self, client: &Client) -> Result<String, String> {
        let users = self
            .read_json(
                client,
                self.endpoint("Users/Query?IsDisabled=false&Limit=100"),
            )
            .await?;
        list(&users["Items"])
            .filter(|user| {
                user["Policy"]["IsAdministrator"] == true && user["Policy"]["IsDisabled"] != true
            })
            .find_map(|user| valid_id(&user["Id"]))
            .map(str::to_owned)
            .ok_or_else(|| "Emby 未返回可读取附加媒体的管理员账户。".into())
    }

    pub(crate) async fn local_extras(
        &self,
        client: &Client,
        id: &Value,
        user: &str,
        trailers: bool,
    ) -> Result<Vec<Value>, String> {
        let id = valid_id(id)
            .filter(|_| is_item_id(user))
            .ok_or("Emby 条目或账户 ID 无效。")?;
        let endpoint = if trailers {
            "LocalTrailers"
        } else {
            "SpecialFeatures"
        };
        let url = self.endpoint(&format!(
            "Users/{user}/Items/{id}/{endpoint}?Fields=Path&EnableUserData=false"
        ));
        self.read_json(client, url)
            .await?
            .as_array()
            .cloned()
            .ok_or_else(|| "Emby 未返回附加媒体列表。".into())
    }

    pub(crate) fn media_file_url(&self, id: &Value) -> Option<Url> {
        let id = valid_id(id)?;
        Some(self.endpoint(&format!("Items/{id}/File")))
    }

    pub(crate) async fn media_response(
        &self,
        client: &Client,
        url: &Url,
        timeout: Duration,
    ) -> Option<Response> {
        if url.origin() != self.base.origin() || !url.path().starts_with(self.base.path()) {
            return None;
        }
        self.timed_response(client, Method::GET, url.clone(), timeout)
            .await
            .ok()
    }

    pub(crate) async fn image_response(&self, client: &Client, url: &Url) -> Option<Response> {
        self.media_response(client, url, Duration::from_secs(10))
            .await
    }
}

pub(crate) fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn work_kind(item: &Value) -> Option<MediaKind> {
    match item["Type"].as_str()? {
        "Series" => Some(MediaKind::Series),
        "Movie" => Some(MediaKind::Movie),
        "Video"
            if item["IsFolder"] != true
                && item["ExtraType"].is_null()
                && item["LocationType"] != "Virtual" =>
        {
            Some(MediaKind::Movie)
        }
        _ => None,
    }
}

fn matches_kind(item: &Value, kind: MediaKind) -> bool {
    let Some(actual) = work_kind(item) else {
        return false;
    };
    actual == kind || (kind == MediaKind::Av && item["Type"] == "Movie")
}

fn nonempty_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn item_tags(item: &Value) -> Vec<String> {
    if item["TagItems"].is_array() {
        return list(&item["TagItems"])
            .filter_map(|tag| nonempty_text(&tag["Name"]))
            .collect();
    }
    list(&item["Tags"]).filter_map(nonempty_text).collect()
}

fn valid_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| is_item_id(id))
}

pub(crate) fn is_item_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn normalize(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn date(value: &Value) -> Option<&str> {
    value.as_str()?.get(..10)
}

fn runtime(item: &Value) -> Option<u64> {
    item["RunTimeTicks"]
        .as_u64()
        .map(|ticks| ticks / 600_000_000)
        .filter(|minutes| *minutes > 0)
}

fn file_name(value: &Value) -> Option<&str> {
    value
        .as_str()?
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
}

pub(crate) fn credit_role(item: &Value, person: &Value) -> &'static str {
    let detail = person["Role"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let voice = ["(voice", "配音", "声优", "聲優", "声優"]
        .iter()
        .any(|part| detail.contains(part));
    let animation = || {
        list(&item["Genres"])
            .filter_map(Value::as_str)
            .any(|genre| {
                genre.eq_ignore_ascii_case("animation")
                    || matches!(genre, "动画" | "動畫" | "动漫" | "アニメ")
            })
    };
    match person["Type"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "voiceactor" | "voice" | "voice actor" => "voice",
        "actor" | "gueststar" if voice || animation() => "voice",
        "actor" | "gueststar" => "actor",
        "director"
        | "series director"
        | "co-director"
        | "animation director"
        | "chief animation director" => "director",
        "writer" | "screenplay" | "series composition" | "teleplay" | "screenwriter" | "story" => {
            "writer"
        }
        "producer" => "producer",
        _ => "crew",
    }
}

pub(crate) fn episode_fields(item: &Value) -> crate::collect::Fields {
    let mut details = fields(item, &[]);
    details.extend(field_map([
        ("series", item["SeriesName"].clone()),
        ("season_number", item["ParentIndexNumber"].clone()),
        ("episode_number", item["IndexNumber"].clone()),
        ("episode_last_number", item["IndexNumberEnd"].clone()),
    ]));
    details
}

fn fields(item: &Value, children: &[Value]) -> crate::collect::Fields {
    let people = |role: &str| {
        list(&item["People"])
            .filter(|credit| {
                let kind = credit_role(item, credit);
                kind == role || (role == "actor" && kind == "voice")
            })
            .filter_map(|credit| credit["Name"].as_str())
            .collect::<Vec<_>>()
    };
    let episodes = children.iter().filter(|entry| entry["Type"] == "Episode").map(|entry| json!({
        "id": entry["Id"], "season":entry["ParentIndexNumber"], "number":entry["IndexNumber"],
        "title":entry["Name"], "description":entry["Overview"], "date":date(&entry["PremiereDate"]),
            "runtime_minutes":entry["RunTimeTicks"].as_u64().map(|ticks| ticks / 600_000_000),
            "file_name":file_name(&entry["Path"]),
    })).collect::<Vec<_>>();
    let seasons = children
        .iter()
        .filter(|entry| entry["Type"] == "Season")
        .count();
    let streams = &item["MediaStreams"];
    let resolution = list(streams)
        .filter(|stream| stream["Type"] == "Video")
        .find_map(|stream| {
            let width = stream["Width"].as_u64().filter(|width| *width > 0)?;
            let height = stream["Height"].as_u64().filter(|height| *height > 0)?;
            Some(format!("{width}×{height}"))
        });
    let has_chinese_subtitles = list(streams)
        .filter(|stream| stream["Type"] == "Subtitle")
        .filter_map(|stream| stream["Language"].as_str())
        .any(|language| {
            matches!(
                language.to_ascii_lowercase().split(['-', '_']).next(),
                Some("zh" | "chi" | "zho" | "cmn" | "yue")
            )
        });
    let videos = list(&item["RemoteTrailers"])
        .filter_map(|entry| {
            let url = entry["Url"].as_str()?;
            Some(json!({"type": "Trailer", "name": entry["Name"], "url": url}))
        })
        .collect::<Vec<_>>();
    field_map([
        ("title", item["Name"].clone()),
        ("title_original", item["OriginalTitle"].clone()),
        ("sort_title", item["SortName"].clone()),
        ("description", item["Overview"].clone()),
        (
            "tagline",
            list(&item["Taglines"]).next().cloned().unwrap_or_default(),
        ),
        ("release_date", json!(date(&item["PremiereDate"]))),
        ("year", item["ProductionYear"].clone()),
        ("official_rating", item["OfficialRating"].clone()),
        ("custom_rating", item["CustomRating"].clone()),
        ("critic_rating", item["CriticRating"].clone()),
        (
            "runtime_minutes",
            json!(
                item["RunTimeTicks"]
                    .as_u64()
                    .map(|ticks| ticks / 600_000_000)
            ),
        ),
        ("genres", item["Genres"].clone()),
        ("tags", json!(item_tags(item))),
        ("resolution", json!(resolution)),
        (
            "has_chinese_subtitles",
            json!(has_chinese_subtitles.then_some(true)),
        ),
        (
            "studio",
            json!(
                list(&item["Studios"])
                    .filter_map(|studio| studio["Name"].as_str())
                    .collect::<Vec<_>>()
            ),
        ),
        ("countries", item["ProductionLocations"].clone()),
        ("cast", json!(people("actor"))),
        ("voice_actors", json!(people("voice"))),
        ("directors", json!(people("director"))),
        ("writers", json!(people("writer"))),
        ("producers", json!(people("producer"))),
        (
            "rating",
            item["CommunityRating"]
                .as_f64()
                .map(|value| json!({"value":value,"scale":10}))
                .unwrap_or_default(),
        ),
        ("season_count", json!((seasons > 0).then_some(seasons))),
        (
            "episode_count",
            json!((!episodes.is_empty()).then_some(episodes.len())),
        ),
        ("episodes", json!(episodes)),
        ("imdb_id", item["ProviderIds"]["Imdb"].clone()),
        ("tmdb_id", item["ProviderIds"]["Tmdb"].clone()),
        ("tvdb_id", item["ProviderIds"]["Tvdb"].clone()),
        ("bangumi_id", item["ProviderIds"]["Bangumi"].clone()),
        ("official_url", item["HomePageUrl"].clone()),
        (
            "trailer_url",
            videos
                .first()
                .map(|video| video["url"].clone())
                .unwrap_or_default(),
        ),
        ("videos", json!(videos)),
        ("file_name", json!(file_name(&item["Path"]))),
    ])
}
