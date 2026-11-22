use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, Response, Url};
use serde_json::{Value, json};

use super::{Emby, has_provider_id, is_item_id, list, nonempty_text, provider_id};
use crate::output::download_image;
use crate::providers::media::episode_title::{field_locked, needs_title, usable};

impl Emby {
    pub(crate) async fn repair_anime_metadata(
        &self,
        client: &Client,
        original: &Value,
        field: &str,
        text: &str,
    ) -> Result<bool, String> {
        if !matches!(field, "Name" | "Overview")
            || text.trim().is_empty()
            || text.chars().count() > 4000
        {
            return Err("修复字段或文字无效。".into());
        }
        let id = original["Id"]
            .as_str()
            .filter(|id| is_item_id(id))
            .ok_or("Emby 条目 ID 无效。")?;
        let edit_url = self.automatic_edit_url(client, id).await?;
        let mut item = self.read_json(client, edit_url.clone()).await?;
        let identity = [
            "Id",
            "Path",
            "Type",
            "ProviderIds",
            "SeriesId",
            "ParentIndexNumber",
            "IndexNumber",
        ];
        if !item["Path"].as_str().is_some_and(|value| {
            let path = Path::new(value);
            path.starts_with("/media/番剧") || path.starts_with("/media/剧场版")
        }) || identity
            .iter()
            .chain([&field])
            .any(|&key| item[key] != original[key])
        {
            return Err("Emby 条目在补全期间发生变化。".into());
        }
        if item["LockData"] == true
            || item["LockData"]["Enabled"] == true
            || item["IsLocked"] == true
            || field_locked(&item, field)
        {
            return Ok(false);
        }
        let mut locks = item["LockedFields"].as_array().cloned().unwrap_or_default();
        locks.push(json!(field));
        item[field] = json!(text);
        if field == "Name" {
            item["ForcedSortName"] = json!(text);
            if !field_locked(&item, "SortName") {
                locks.push(json!("SortName"));
            }
        }
        item["LockedFields"] = json!(locks);
        self.automatic_post(
            client,
            self.endpoint(&format!("Items/{id}")),
            "application/json",
            item.to_string(),
        )
        .await?;
        let saved = self.read_json(client, edit_url).await?;
        if ["Name", "Overview"]
            .iter()
            .chain(&identity)
            .any(|&key| saved[key] != item[key])
            || !locks
                .iter()
                .all(|value| value.as_str().is_some_and(|key| field_locked(&saved, key)))
        {
            return Err("Emby 没有确认补全写入。".into());
        }
        Ok(true)
    }

    pub(crate) async fn automatic_server_id(&self, client: &Client) -> Result<String, String> {
        let info = self
            .read_json(client, self.endpoint("System/Info/Public"))
            .await?;
        info["Id"]
            .as_str()
            .filter(|id| is_item_id(id))
            .map(str::to_owned)
            .ok_or_else(|| "Emby 未提供可用于 PostgreSQL 同步的服务器 ID。".into())
    }

    pub(crate) async fn enrich_automatic_item(
        &self,
        client: &Client,
        item: &Value,
        record: &Value,
    ) -> Result<(), String> {
        let id = item["Id"]
            .as_str()
            .filter(|id| is_item_id(id))
            .ok_or("Emby 条目 ID 无效。")?;
        let path = item["Path"]
            .as_str()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or("Emby 条目缺少媒体路径。")?;
        let root = self.internal_storage_root(client, path).await?;
        let fields = &record["fields"];
        let url = self.automatic_edit_url(client, id).await?;
        let mut current = self.read_json(client, url.clone()).await?;
        if current["Path"].as_str() != path.to_str()
            || current["Type"] != item["Type"]
            || current["Id"] != id
        {
            return Err("Emby 条目在采集期间发生变化，请重试。".into());
        }
        if current["LockData"] == true || current["IsLocked"] == true {
            return Err("Emby 条目已锁定。".into());
        }
        if merge_missing(&mut current, fields) {
            let expected_name = current["Name"].clone();
            self.automatic_post(
                client,
                self.endpoint(&format!("Items/{id}")),
                "application/json",
                current.to_string(),
            )
            .await?;
            let mut saved = self.read_json(client, url).await?;
            if saved["Name"] != expected_name || merge_missing(&mut saved, fields) {
                return Err("Emby 没有保存全部可写的补充字段。".into());
            }
        }

        let images_url = self.endpoint(&format!("Items/{id}/Images"));
        let mut existing = self.read_json(client, images_url.clone()).await?;
        let episode = item["Type"] == "Episode";
        for (kind, url) in [
            (
                "Primary",
                if episode {
                    fields["still_url"].as_str()
                } else {
                    fields["poster_url"]
                        .as_str()
                        .or_else(|| fields["cover_url"].as_str())
                },
            ),
            (
                "Backdrop",
                (!episode)
                    .then(|| fields["backdrop_url"].as_str())
                    .flatten(),
            ),
            (
                "Logo",
                (!episode).then(|| fields["logo_url"].as_str()).flatten(),
            ),
            (
                "Thumb",
                (!episode)
                    .then(|| fields["landscape_url"].as_str())
                    .flatten()
                    .filter(|url| Some(*url) != fields["backdrop_url"].as_str()),
            ),
        ] {
            let Some(url) = url else { continue };
            if list(&existing).any(|image| image["ImageType"] == kind) {
                continue;
            }
            let (bytes, extension) = download_image(client, url)
                .await
                .ok_or_else(|| format!("无法读取用于 Emby 写入的 {kind} 图片。"))?;
            let mime = match extension {
                "jpg" => "image/jpeg",
                "png" => "image/png",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => return Err("Emby 图片格式无效。".into()),
            };
            self.automatic_post(
                client,
                self.endpoint(&format!("Items/{id}/Images/{kind}")),
                mime,
                STANDARD.encode(bytes),
            )
            .await?;
            existing = self.read_json(client, images_url.clone()).await?;
            if !list(&existing).any(|image| {
                image["ImageType"] == kind
                    && image["Path"].as_str().is_some_and(|dest| {
                        Path::new(dest).is_absolute() && !Path::new(dest).starts_with(&root)
                    })
            }) {
                return Err(format!("Emby 未确认 {kind} 已存入服务器元数据目录。"));
            }
        }
        Ok(())
    }

    pub(crate) async fn fill_episode_bangumi_id(
        &self,
        client: &Client,
        item: &Value,
        bangumi_id: u32,
    ) -> Result<bool, String> {
        let id = item["Id"]
            .as_str()
            .filter(|id| is_item_id(id))
            .ok_or("Emby 条目 ID 无效。")?;
        let path = item["Path"]
            .as_str()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or("Emby 条目缺少媒体路径。")?;
        self.internal_storage_root(client, path).await?;
        let url = self.automatic_edit_url(client, id).await?;
        let mut current = self.read_json(client, url.clone()).await?;
        if current["Type"] != "Episode"
            || [
                "Id",
                "Path",
                "SeriesId",
                "ParentIndexNumber",
                "IndexNumber",
                "IndexNumberEnd",
                "PremiereDate",
                "Name",
            ]
            .iter()
            .any(|&field| current[field] != item[field])
            || provider_id(&current, "Tvdb") != provider_id(item, "Tvdb")
        {
            return Err("Emby 分集在核对映射期间发生变化，请重试。".into());
        }
        if current["LockData"] == true
            || current["LockData"]["Enabled"] == true
            || current["IsLocked"] == true
            || !merge_missing_provider_id(&mut current, "Bangumi", Some(&bangumi_id.to_string()))
        {
            return Ok(false);
        }
        self.automatic_post(
            client,
            self.endpoint(&format!("Items/{id}")),
            "application/json",
            current.to_string(),
        )
        .await?;
        let saved = self.read_json(client, url).await?;
        if current["ProviderIds"]
            .as_object()
            .into_iter()
            .flatten()
            .any(|(provider, id)| saved["ProviderIds"][provider] != *id)
        {
            return Err("Emby 没有保留全部分集来源 ID。".into());
        }
        Ok(true)
    }

    async fn automatic_edit_url(&self, client: &Client, id: &str) -> Result<Url, String> {
        let admin = self.extras_user(client).await?;
        let mut url = self.endpoint(&format!("Users/{admin}/Items/{id}"));
        url.query_pairs_mut()
            .append_pair("Fields", "ChannelMappingInfo")
            .append_pair(
                "ExcludeFields",
                "Chapters,MediaSources,MediaStreams,Subviews",
            );
        Ok(url)
    }

    async fn internal_storage_root(&self, client: &Client, path: &Path) -> Result<PathBuf, String> {
        let libraries = self
            .read_json(client, self.endpoint("Library/VirtualFolders"))
            .await?;
        let (root, library) = list(&libraries)
            .flat_map(|library| {
                list(&library["Locations"]).filter_map(move |location| {
                    let root = Path::new(location.as_str()?);
                    path.starts_with(root).then_some((root, library))
                })
            })
            .max_by_key(|(root, _)| root.components().count())
            .ok_or("Emby 条目未匹配可配置的媒体库。")?;
        let options = &library["LibraryOptions"];
        if options["SaveLocalMetadata"] != false
            || !options["MetadataSavers"]
                .as_array()
                .is_some_and(Vec::is_empty)
        {
            return Err("Emby 媒体库需关闭原生旁挂保存，由 Collect Metadata 写入视频目录。".into());
        }
        Ok(root.to_owned())
    }

    async fn automatic_post(
        &self,
        client: &Client,
        url: Url,
        content_type: &str,
        body: String,
    ) -> Result<Response, String> {
        let response = client
            .post(url.clone())
            .timeout(std::time::Duration::from_secs(20))
            .header("X-Emby-Token", self.authorization.clone())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            .map_err(|_| "无法连接 Emby 写入资料。")?;
        if response.status().is_success() && response.url() == &url {
            Ok(response)
        } else {
            Err(format!(
                "Emby 写入资料失败（HTTP {}）。",
                response.status().as_u16()
            ))
        }
    }
}

fn merge_missing_provider_id(item: &mut Value, key: &str, id: Option<&str>) -> bool {
    if !has_provider_id(item, key)
        && !list(&item["LockedFields"])
            .any(|field| matches!(field.as_str(), Some("ProviderIds" | "ExternalIds")))
        && let Some(id) = id.filter(|id| id.parse::<u32>().is_ok_and(|n| n > 0))
    {
        let ids = &mut item["ProviderIds"];
        let key = ids
            .as_object()
            .and_then(|ids| ids.keys().find(|name| name.eq_ignore_ascii_case(key)))
            .cloned()
            .unwrap_or_else(|| key.to_owned());
        ids[key] = json!(id);
        return true;
    }
    false
}

fn merge_missing(item: &mut Value, fields: &Value) -> bool {
    let mut changed = false;
    if needs_title(item) && let Some(title) = usable(&fields["title"], item) {
        let title = json!(title);
        if !field_locked(item, "SortName") && usable(&item["ForcedSortName"], item).is_none() {
            item["ForcedSortName"] = title.clone();
            item["SortName"] = title.clone();
        }
        item["Name"] = title;
        changed = true;
    }
    for (key, source) in [("Tmdb", "tmdb_id"), ("Bangumi", "bangumi_id")] {
        changed |= merge_missing_provider_id(item, key, fields[source].as_str());
    }
    for (key, locked, source) in [
        ("Overview", "Overview", "description"),
        ("OriginalTitle", "Name", "title_original"),
        ("OfficialRating", "OfficialRating", "official_rating"),
    ] {
        if nonempty_text(&item[key]).is_none()
            && !list(&item["LockedFields"]).any(|field| field == locked)
            && let Some(value) = nonempty_text(&fields[source])
        {
            item[key] = json!(value);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_only_the_missing_bangumi_id_for_a_complete_episode() {
        let mut episode = json!({
            "Type":"Episode", "Name":"原有标题", "Overview":"原有简介",
            "ImageTags":{"Primary":"existing-image"}, "ProviderIds":{"Tvdb":"12006499"}
        });
        let mut expected = episode.clone();
        expected["ProviderIds"]["Bangumi"] = json!("1749199");
        assert!(merge_missing_provider_id(&mut episode, "Bangumi", Some("1749199")));
        assert_eq!(episode, expected);
        assert!(!merge_missing_provider_id(&mut episode, "Bangumi", Some("1749200")));
        assert_eq!(episode, expected);

        let mut legacy = json!({"ProviderIds":{"bangumi":"123456"}});
        assert!(!merge_missing_provider_id(&mut legacy, "Bangumi", Some("1749199")));
        assert_eq!(legacy["ProviderIds"], json!({"bangumi":"123456"}));

        let mut locked = json!({"LockedFields":["ExternalIds"],"ProviderIds":{"Tvdb":"12006499"}});
        assert!(!merge_missing_provider_id(&mut locked, "Bangumi", Some("1749199")));
        assert_eq!(locked["ProviderIds"], json!({"Tvdb":"12006499"}));
    }

    #[test]
    fn only_fills_absent_unlocked_fields() {
        let mut item = json!({"Overview":"Keep this", "ProviderIds":{"Tmdb":"83","Imdb":"tt1"},
            "LockedFields":["OfficialRating"]});
        let fields = json!({"description":"Other", "tmdb_id":"93", "bangumi_id":"29",
            "official_rating":"R", "title_original":"Original"});
        assert!(merge_missing(&mut item, &fields));
        assert!(!merge_missing(&mut item, &fields));
        assert_eq!(
            item,
            json!({"Overview":"Keep this", "ProviderIds":{"Tmdb":"83","Imdb":"tt1","Bangumi":"29"},
            "LockedFields":["OfficialRating"],"OriginalTitle":"Original"})
        );
    }

    #[test]
    fn fills_episode_display_title_without_overwriting_manual_metadata() {
        let mut item = json!({"Type":"Episode", "Name":"第 7 集", "IndexNumber":7,
            "ForcedSortName":"第 7 集","SortName":"第 7 集", "Overview":"Keep this",
            "LockedFields":["Overview"]});
        let fields = json!({"title":"重逢", "description":"Other"});
        assert!(!merge_missing(&mut item, &json!({"title":"Episode 7"})));
        assert!(merge_missing(&mut item, &fields));
        assert_eq!(item["Name"], "重逢");
        assert_eq!(item["ForcedSortName"], "重逢");
        assert_eq!(item["Overview"], "Keep this");
        assert!(!merge_missing(&mut item, &json!({"title":"Alternate"})));

        item["Name"] = json!("第 7 集");
        item["LockedFields"] = json!(["Name"]);
        assert!(!merge_missing(&mut item, &fields));
        item["LockedFields"] = json!(["SortName"]);
        assert!(merge_missing(&mut item, &json!({"title":"会合"})));
        assert_eq!(item["ForcedSortName"], "重逢");
    }
}
