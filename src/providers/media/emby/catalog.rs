use std::collections::{HashMap, HashSet};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{
    Emby, EmbyLibrary, ImdbRating, MediaKind, credit_role, date, is_item_id, item_tags, list,
    nonempty_text, runtime, valid_id, work_kind,
};

const PREVIEW_FIELDS: &str = concat!(
    "Overview,Genres,OriginalTitle,ProductionYear,CommunityRating,CriticRating,",
    "OfficialRating,ProviderIds,Tags,TagItems"
);
const EPISODE_SUMMARY_FIELDS: &str = "PremiereDate,CommunityRating";

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
    official_rating: Option<String>,
    critic_rating: Option<f64>,
    imdb_rating: Option<ImdbRating>,
    tmdb_id: Option<String>,
    bangumi_id: Option<String>,
    tags: Vec<String>,
    runtime_minutes: Option<u64>,
    poster: Option<String>,
    backdrop: Option<String>,
    logo: Option<String>,
    landscape: Option<String>,
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
    premiere_at: Option<String>,
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
    Logo,
    Landscape,
    Episode,
    Portrait,
}

impl Emby {
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
                        ("Fields", PREVIEW_FIELDS),
                        ("EnableImages", "true"),
                        ("EnableImageTypes", "Primary,Backdrop,Logo,Thumb"),
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
                    official_rating: nonempty_text(&item["OfficialRating"]),
                    critic_rating: item["CriticRating"].as_f64(),
                    imdb_rating: self.imdb_rating(item),
                    tmdb_id: nonempty_text(&item["ProviderIds"]["Tmdb"]),
                    bangumi_id: nonempty_text(&item["ProviderIds"]["Bangumi"]),
                    tags: item_tags(item),
                    runtime_minutes: runtime(item),
                    poster: nonempty_text(&item["ImageTags"]["Primary"]),
                    backdrop: item["BackdropImageTags"]
                        .as_array()
                        .and_then(|tags| tags.first())
                        .and_then(nonempty_text),
                    logo: nonempty_text(&item["ImageTags"]["Logo"]),
                    landscape: nonempty_text(&item["ImageTags"]["Thumb"]),
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

    pub(crate) async fn missing_episode_overviews(
        &self,
        client: &Client,
        library: &str,
    ) -> Result<HashMap<String, Vec<String>>, String> {
        const PAGE_SIZE: usize = 200;
        let mut missing = HashMap::<String, Vec<String>>::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        loop {
            let page = self
                .items(
                    client,
                    &[
                        ("ParentId", library),
                        ("Recursive", "true"),
                        ("IncludeItemTypes", "Episode"),
                        ("HasOverview", "false"),
                        ("IsMissing", "false"),
                        ("EnableImages", "false"),
                        ("Fields", "Overview,LocationType"),
                        ("Limit", &PAGE_SIZE.to_string()),
                        ("StartIndex", &offset.to_string()),
                    ],
                )
                .await?;
            let items = page["Items"]
                .as_array()
                .ok_or("Emby 返回的分集格式无效。")?;
            for item in items {
                if item["Type"] == "Episode"
                    && item["LocationType"] != "Virtual"
                    && item["Overview"]
                        .as_str()
                        .is_none_or(|text| text.trim().is_empty())
                    && let (Some(series), Some(id)) =
                        (valid_id(&item["SeriesId"]), valid_id(&item["Id"]))
                    && seen.insert(id.to_owned())
                {
                    missing
                        .entry(series.to_owned())
                        .or_default()
                        .push(id.to_owned());
                }
            }
            offset += items.len();
            let last_page = page["TotalRecordCount"]
                .as_u64()
                .map_or(items.len() < PAGE_SIZE, |total| offset as u64 >= total);
            if last_page {
                return Ok(missing);
            }
            if items.is_empty() {
                return Err("Emby 未返回全部缺少简介的分集。".into());
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
            .series_children(
                client,
                id,
                &format!("Overview,OriginalTitle,{EPISODE_SUMMARY_FIELDS}"),
            )
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
                premiere_at: nonempty_text(&item["PremiereDate"]),
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

    pub(crate) async fn credits(
        &self,
        client: &Client,
        id: &str,
    ) -> Result<Vec<EmbyCredit>, String> {
        if !is_item_id(id) {
            return Err("Emby 条目 ID 无效。".into());
        }
        let result = self
            .items(client, &[("Ids", id), ("Fields", "People,Genres")])
            .await?;
        let item = list(&result["Items"])
            .find(|item| {
                valid_id(&item["Id"]) == Some(id)
                    && matches!(item["Type"].as_str(), Some("Movie" | "Series" | "Episode"))
            })
            .ok_or("Emby 未返回该作品的资料。")?;
        let mut seen = std::collections::HashSet::new();
        Ok(list(&item["People"])
            .filter_map(|person| {
                let name = nonempty_text(&person["Name"])?;
                let id = valid_id(&person["Id"]).map(str::to_owned);
                let role = credit_role(item, person);
                let detail = nonempty_text(&person["Role"]);
                let portrait = id.as_ref().and_then(|_| {
                    nonempty_text(&person["PrimaryImageTag"])
                        .or_else(|| nonempty_text(&person["ImageTags"]["Primary"]))
                });
                seen.insert((id.clone(), name.clone(), role, detail.clone()))
                    .then_some(EmbyCredit {
                        id,
                        name,
                        role,
                        detail,
                        portrait,
                    })
            })
            .take(200)
            .collect())
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
            EmbyPreviewImage::Logo => ("Logo", "800", "400"),
            EmbyPreviewImage::Landscape => ("Thumb", "1280", "720"),
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
}
