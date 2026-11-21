use std::{
    collections::HashMap,
    env, fs, io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{Client, Response, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use super::MediaKind;
use crate::{collect::Source, providers::field_map};

const FIELDS: &str = "Path,Overview,People,ProviderIds,Genres,Studios,Tags,Taglines,OriginalTitle,SortName,ProductionLocations,MediaSources,MediaStreams,Chapters,ExternalUrls,HomePageUrl,RemoteTrailers";
pub(crate) const EPISODE_QUERY_PREFIX: &str = "emby-episode:";
const MAX_RESPONSE: usize = 12 * 1024 * 1024;

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

#[derive(Serialize)]
pub struct EmbyLibrary {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Serialize)]
pub struct EmbyWorkPreview {
    pub(crate) id: String,
    pub(crate) kind: MediaKind,
    pub(crate) title: String,
    year: Option<u16>,
    original_title: Option<String>,
    overview: Option<String>,
    genres: Vec<String>,
    rating: Option<f64>,
    runtime_minutes: Option<u64>,
    poster: Option<String>,
    backdrop: Option<String>,
}

#[derive(Serialize)]
pub struct EmbyLibraryPreview {
    #[serde(flatten)]
    pub(crate) library: EmbyLibrary,
    pub(crate) works: Vec<EmbyWorkPreview>,
}

#[derive(Serialize)]
pub struct EmbySeriesPreview {
    seasons: Vec<EmbySeasonPreview>,
    partial: bool,
}

#[derive(Default, Serialize)]
struct EmbySeasonPreview {
    number: Option<u64>,
    title: Option<String>,
    episodes: Vec<EmbyEpisodePreview>,
}

#[derive(Serialize)]
struct EmbyEpisodePreview {
    id: String,
    number: Option<u64>,
    last_number: Option<u64>,
    title: String,
    original_title: Option<String>,
    overview: Option<String>,
    date: Option<String>,
    runtime_minutes: Option<u64>,
    rating: Option<f64>,
    image: Option<String>,
}

#[derive(Serialize)]
pub struct EmbyCredit {
    id: Option<String>,
    name: String,
    role: &'static str,
    detail: Option<String>,
    portrait: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbyPreviewImage {
    Poster,
    Backdrop,
    Episode,
    Portrait,
}

pub struct EmbySettings {
    file: PathBuf,
    state: Mutex<EmbyInput>,
}

pub(crate) struct Emby {
    base: Url,
    authorization: HeaderValue,
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
        Ok(Self {
            file,
            state: Mutex::new(state),
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
        Emby::new(&input)?.validate(client).await
    }

    pub async fn save(
        &self,
        client: &Client,
        input: EmbyInput,
    ) -> Result<EmbySettingsView, String> {
        let input = self.resolve(input);
        if input.enabled {
            Emby::new(&input)?.validate(client).await?;
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
        Emby::new(&input).ok().map(Arc::new)
    }

    pub(crate) fn active(&self) -> Option<Arc<Emby>> {
        let input = self.state.lock().expect("Emby settings lock");
        input
            .enabled
            .then(|| Emby::new(&input).ok())
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

impl Emby {
    fn new(input: &EmbyInput) -> Result<Self, String> {
        let mut base = Url::parse(&input.server).map_err(|_| "请输入有效的 Emby 服务地址。")?;
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
        if input.api_key.is_empty() || input.api_key.len() > 512 {
            return Err("请填写 Emby API 密钥。".into());
        }
        let mut authorization =
            HeaderValue::from_str(&input.api_key).map_err(|_| "Emby API 密钥格式无效。")?;
        authorization.set_sensitive(true);
        Ok(Self {
            base,
            authorization,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        self.base.join(path).expect("static Emby API path")
    }

    async fn response(&self, client: &Client, url: Url) -> Result<Response, String> {
        self.timed_response(client, url, Duration::from_secs(10))
            .await
    }

    async fn timed_response(
        &self,
        client: &Client,
        url: Url,
        timeout: Duration,
    ) -> Result<Response, String> {
        let response = client
            .get(url.clone())
            .timeout(timeout)
            .header("X-Emby-Token", self.authorization.clone())
            .send()
            .await
            .map_err(|_| "无法连接 Emby。")?;
        if !response.status().is_success() || response.url() != &url {
            return Err(format!(
                "Emby 请求失败（HTTP {}）。",
                response.status().as_u16()
            ));
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

    pub(crate) async fn library_entries(
        &self,
        client: &Client,
        id: &str,
    ) -> Result<Vec<EmbyWorkPreview>, String> {
        let mut result = Vec::new();
        let mut offset = 0;
        const PAGE_SIZE: usize = 100;
        loop {
            let page = self
                .items(
                    client,
                    &[
                        ("ParentId", id),
                        ("Recursive", "true"),
                        ("IncludeItemTypes", "Movie,Series,Video"),
                        ("SortBy", "SortName"),
                        ("Fields", "Overview,Genres,OriginalTitle"),
                        ("EnableImages", "true"),
                        ("EnableImageTypes", "Primary,Backdrop"),
                        ("ImageTypeLimit", "1"),
                        ("Limit", "100"),
                        ("StartIndex", &offset.to_string()),
                    ],
                )
                .await?;
            let items = page["Items"]
                .as_array()
                .ok_or("Emby 返回的媒体库格式无效。")?;
            result.extend(items.iter().filter_map(|item| {
                let id = valid_id(&item["Id"])?;
                let kind = work_kind(item)?;
                let title = item["Name"]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(id);
                Some(EmbyWorkPreview {
                    id: id.to_owned(),
                    kind,
                    title: title.to_owned(),
                    year: item["ProductionYear"]
                        .as_u64()
                        .and_then(|year| year.try_into().ok()),
                    original_title: nonempty_text(&item["OriginalTitle"]),
                    overview: nonempty_text(&item["Overview"]),
                    genres: list(&item["Genres"]).filter_map(nonempty_text).collect(),
                    rating: item["CommunityRating"].as_f64(),
                    runtime_minutes: runtime(item),
                    poster: nonempty_text(&item["ImageTags"]["Primary"]),
                    backdrop: item["BackdropImageTags"]
                        .as_array()
                        .and_then(|tags| tags.first())
                        .and_then(nonempty_text),
                })
            }));
            offset += items.len();
            let last_page = page["TotalRecordCount"]
                .as_u64()
                .map_or(items.len() < PAGE_SIZE, |count| offset as u64 >= count);
            if items.is_empty() || last_page {
                break;
            }
        }
        Ok(result)
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
            return self.details(client, id, item_type).await.map(|source| (id.to_owned(), source));
        }
        if let Some(id) = query.strip_prefix(EPISODE_QUERY_PREFIX) {
            if kind != MediaKind::Series {
                return None;
            }
            return self.details(client, id, "Episode").await.map(|source| (id.to_owned(), source));
        }
        let mut params = vec![
            ("Recursive", "true"),
            ("IncludeItemTypes", item_type),
            ("Limit", "60"),
            ("Fields", "Path,ProviderIds,OriginalTitle,SortName"),
        ];
        let exact_tmdb = super::tmdb_id_query(query);
        let provider_id = exact_tmdb.map(|id| format!("Tmdb.{id}"));
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
            if let Some(tmdb) = exact_tmdb {
                return item["ProviderIds"]["Tmdb"].as_str() == Some(tmdb);
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

    pub(crate) async fn scrape_title(&self, client: &Client, id: &str, item_type: &str) -> Result<String, String> {
        let item = self.item(client, id, item_type, "").await?;
        let title = nonempty_text(&item["Name"]).ok_or("Emby 条目缺少标题。")?;
        if item_type == "Episode" && let Some(series) = nonempty_text(&item["SeriesName"]) {
            return Ok(format!("{series} · {title}"));
        }
        Ok(title)
    }

    async fn item(&self, client: &Client, id: &str, item_type: &str, fields: &str) -> Result<Value, String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let mut page = self.items(client, &[("Ids", id), ("Fields", fields)]).await?;
        page["Items"]
            .as_array_mut()
            .and_then(|items| items.first_mut())
            .filter(|item| {
                let matches = if item_type == "Movie,Video" {
                    matches_kind(item, MediaKind::Movie)
                } else {
                    item["Type"] == item_type
                };
                valid_id(&item["Id"]) == Some(id) && matches
            })
            .map(Value::take)
            .ok_or_else(|| "未找到指定类型的 Emby 条目。".into())
    }

    async fn details(&self, client: &Client, id: &str, item_type: &str) -> Option<Source> {
        let item = self.item(client, id, item_type, FIELDS).await.ok()?;
        nonempty_text(&item["Name"])?;
        let (children, complete) = if item_type == "Series" {
            self.series_children(client, id, FIELDS).await.unwrap_or_default()
        } else {
            (Vec::new(), true)
        };
        let mut details = fields(&item, &children);
        if item_type == "Episode" {
            details.extend(field_map([
                ("series", item["SeriesName"].clone()),
                ("season_number", item["ParentIndexNumber"].clone()),
                ("episode_number", item["IndexNumber"].clone()),
                ("episode_last_number", item["IndexNumberEnd"].clone()),
            ]));
        }
        let source_url = self.endpoint(&format!("web/index.html#!/item?id={id}"));
        let mut source = Source::found(
            "Emby",
            &source_url,
            details,
            json!({"item":item,"children":children}),
        );
        if !complete {
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

    pub(crate) async fn series_preview(
        &self,
        client: &Client,
        id: &str,
    ) -> Result<EmbySeriesPreview, String> {
        if !is_item_id(id) {
            return Err("Emby 剧集 ID 无效。".into());
        }
        let (children, complete) = self
            .series_children(client, id, "Overview,OriginalTitle")
            .await?;
        let mut seasons = HashMap::<Option<u64>, EmbySeasonPreview>::new();
        let mut season_numbers = HashMap::new();
        for item in children.iter().filter(|item| item["Type"] == "Season") {
            let Some(number) = item["IndexNumber"].as_u64() else {
                continue;
            };
            if let Some(id) = valid_id(&item["Id"]) {
                season_numbers.insert(id, number);
            }
            seasons.insert(
                Some(number),
                EmbySeasonPreview {
                    number: Some(number),
                    title: nonempty_text(&item["Name"]),
                    episodes: Vec::new(),
                },
            );
        }
        for item in children.iter().filter(|item| item["Type"] == "Episode") {
            let Some(id) = valid_id(&item["Id"]) else {
                continue;
            };
            let season = item["ParentIndexNumber"]
                .as_u64()
                .or_else(|| season_numbers.get(item["SeasonId"].as_str()?).copied());
            let group = seasons.entry(season).or_insert_with(|| EmbySeasonPreview {
                number: season,
                ..Default::default()
            });
            group.episodes.push(EmbyEpisodePreview {
                id: id.to_owned(),
                number: item["IndexNumber"].as_u64(),
                last_number: item["IndexNumberEnd"].as_u64(),
                title: nonempty_text(&item["Name"]).unwrap_or_else(|| "未命名分集".into()),
                original_title: nonempty_text(&item["OriginalTitle"]),
                overview: nonempty_text(&item["Overview"]),
                date: date(&item["PremiereDate"]).map(str::to_owned),
                runtime_minutes: runtime(item),
                rating: item["CommunityRating"].as_f64(),
                image: nonempty_text(&item["ImageTags"]["Primary"]),
            });
        }
        let mut seasons = seasons.into_values().collect::<Vec<_>>();
        seasons.sort_by_key(|season| season.number.unwrap_or(u64::MAX));
        for season in &mut seasons {
            season
                .episodes
                .sort_by_key(|episode| episode.number.unwrap_or(u64::MAX));
        }
        Ok(EmbySeriesPreview {
            seasons,
            partial: !complete,
        })
    }

    pub(crate) async fn credits(&self, client: &Client, id: &str) -> Result<Vec<EmbyCredit>, String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let result = self.items(client, &[("Ids", id), ("Fields", "People,Genres")]).await?;
        let item = list(&result["Items"]).find(|item| valid_id(&item["Id"]) == Some(id)
            && matches!(item["Type"].as_str(), Some("Movie" | "Series" | "Episode")))
            .ok_or("Emby 未返回该作品的资料。")?;
        let mut seen = std::collections::HashSet::new();
        Ok(list(&item["People"]).filter_map(|person| {
            let name = nonempty_text(&person["Name"])?;
            let id = valid_id(&person["Id"]).map(str::to_owned);
            let role = credit_role(item, person);
            let detail = nonempty_text(&person["Role"]);
            let portrait = id.as_ref().and_then(|_| nonempty_text(&person["PrimaryImageTag"])
                .or_else(|| nonempty_text(&person["ImageTags"]["Primary"])));
            seen.insert((id.clone(), name.clone(), role, detail.clone())).then_some(EmbyCredit {id, name, role, detail, portrait})
        }).take(200).collect())
    }

    pub(crate) fn image_url(&self, id: &Value, image_type: &str) -> Option<Url> {
        let id = valid_id(id)?;
        matches!(image_type, "Primary" | "Backdrop")
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
        self.timed_response(client, url.clone(), timeout).await.ok()
    }

    pub(crate) async fn preview_image(
        &self,
        client: &Client,
        id: &str,
        image: EmbyPreviewImage,
    ) -> Option<(Vec<u8>, &'static str)> {
        if !is_item_id(id) {
            return None;
        }
        let (kind, width, height) = match image {
            EmbyPreviewImage::Poster => ("Primary", "420", "630"),
            EmbyPreviewImage::Backdrop => ("Backdrop", "1280", "720"),
            EmbyPreviewImage::Episode => ("Primary", "640", "360"),
            EmbyPreviewImage::Portrait => ("Primary", "256", "384"),
        };
        let mut url = self.endpoint(&format!("Items/{id}/Images/{kind}"));
        url.query_pairs_mut()
            .append_pair("MaxWidth", width)
            .append_pair("MaxHeight", height)
            .append_pair("Quality", "85")
            .append_pair("EnableImageEnhancers", "false");
        crate::output::image_bytes(self.image_response(client, &url).await?).await
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

fn valid_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| is_item_id(id))
}

fn is_item_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn normalize(value: &str) -> String {
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
    let detail = person["Role"].as_str().unwrap_or_default().to_ascii_lowercase();
    let voice = ["(voice", "配音", "声优", "聲優", "声優"].iter().any(|part| detail.contains(part));
    let animation = || list(&item["Genres"]).filter_map(Value::as_str).any(|genre| {
        genre.eq_ignore_ascii_case("animation") || matches!(genre, "动画" | "動畫" | "动漫" | "アニメ")
    });
    match person["Type"].as_str().unwrap_or_default().to_ascii_lowercase().as_str() {
        "voiceactor" | "voice" | "voice actor" => "voice",
        "actor" | "gueststar" if voice || animation() => "voice",
        "actor" | "gueststar" => "actor",
        "director" | "series director" | "co-director" | "animation director"
        | "chief animation director" => "director",
        "writer" | "screenplay" | "series composition" | "teleplay" | "screenwriter" | "story" => "writer",
        "producer" => "producer",
        _ => "crew",
    }
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
        ("tagline", list(&item["Taglines"]).next().cloned().unwrap_or_default()),
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
        ("tags", item["Tags"].clone()),
        ("resolution", json!(resolution)),
        ("has_chinese_subtitles", json!(has_chinese_subtitles.then_some(true))),
        ("studio", json!(list(&item["Studios"]).filter_map(|studio| studio["Name"].as_str()).collect::<Vec<_>>())),
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
        ("official_url", item["HomePageUrl"].clone()),
        ("trailer_url", videos.first().map(|video| video["url"].clone()).unwrap_or_default()),
        ("videos", json!(videos)),
        ("file_name", json!(file_name(&item["Path"]))),
    ])
}
