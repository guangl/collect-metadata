use std::{
    collections::HashMap,
    env, fs,
    sync::{Arc, OnceLock},
};

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use super::episodes::Episode;
use super::{Tmdb, id};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Group {
    group: String,
    subgroup: String,
}

pub(super) type Groups = HashMap<u32, HashMap<u64, Group>>;

fn parse_groups(text: &str) -> Result<Groups, String> {
    let groups: Groups = serde_json::from_str(text)
        .map_err(|error| format!("TMDB 分组格式无效：{error}"))?;
    if groups.iter().any(|(series, seasons)| {
        *series == 0
            || seasons.is_empty()
            || seasons.iter().any(|(season, entry)| {
                *season == 0
                    || [&entry.group, &entry.subgroup].iter().any(|id| {
                        id.len() != 24 || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
    }) {
        return Err("TMDB 分组必须提供有效的剧集 ID、普通季号、分组 ID 和子组 ID。".into());
    }
    Ok(groups)
}

fn groups() -> Result<&'static Groups, String> {
    static GROUPS: OnceLock<Result<Groups, String>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            let Some(path) = env::var_os("EMBY_AUTO_SCRAPE_TMDB_GROUPS").filter(|s| !s.is_empty())
            else {
                return Ok(Groups::new());
            };
            let text = fs::read_to_string(path)
                .map_err(|error| format!("无法读取已核对的 TMDB 分组：{error}"))?;
            parse_groups(&text)
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn validate_episode_groups() -> Result<(), String> {
    groups().map(|_| ())
}

impl Tmdb {
    #[cfg(test)]
    pub(crate) fn with_group_fixture(mut self, text: &str) -> Self {
        self.episode_group_fixture = Some(parse_groups(text).unwrap());
        self
    }

    fn episode_groups(&self) -> Result<&Groups, String> {
        #[cfg(test)]
        if let Some(groups) = &self.episode_group_fixture {
            return Ok(groups);
        }
        groups()
    }

    pub(super) fn has_group(&self, series: u32, season: u64) -> bool {
        self.episode_groups().is_ok_and(|groups| {
            groups
                .get(&series)
                .is_some_and(|seasons| seasons.contains_key(&season))
        })
    }

    pub(super) async fn grouped_episode(
        &self,
        client: &Client,
        item: &Value,
        series: u32,
        timezone: &TimeZone,
    ) -> Result<Option<(Episode, Date)>, String> {
        let Some(season) = item["ParentIndexNumber"].as_u64() else {
            return Ok(None);
        };
        let Some(mapping) = self.episode_groups()?
            .get(&series)
            .and_then(|seasons| seasons.get(&season))
        else {
            return Ok(None);
        };
        let key = format!("group:{}", mapping.group);
        let group = if let Some(group) = self.series_details.get(&key) {
            group
        } else {
            let endpoint = self.endpoint(&format!("tv/episode_group/{}", mapping.group), "en");
            let group = Arc::new(
                self.get(client, &endpoint)
                    .await
                    .map_err(|error| format!("TMDB 分集分组：{}", error.description()))?,
            );
            if group["id"] != mapping.group {
                return Err("TMDB 返回的分集分组 ID 不符。".into());
            }
            self.series_details.insert(&key, Arc::clone(&group));
            group
        };
        Ok(resolve_episode(
            &group, mapping, item, series, season, timezone,
        ))
    }
}

fn resolve_episode(
    group: &Value,
    mapping: &Group,
    item: &Value,
    series: u32,
    season: u64,
    timezone: &TimeZone,
) -> Option<(Episode, Date)> {
    let local = item["IndexNumber"].as_u64().filter(|&n| n > 0)?;
    if item["IndexNumberEnd"]
        .as_u64()
        .is_some_and(|end| end > local)
    {
        return None;
    }
    let mut subgroups = group["groups"]
        .as_array()?
        .iter()
        .filter(|subgroup| subgroup["id"] == mapping.subgroup && subgroup["order"] == season);
    let subgroup = subgroups.next().filter(|_| subgroups.next().is_none())?;
    let mut matches = subgroup["episodes"]
        .as_array()?
        .iter()
        .filter(|episode| episode["order"] == local - 1);
    let episode = matches.next().filter(|_| matches.next().is_none())?;
    let episode_id = episode["id"].as_u64().filter(|&n| n > 0)?;
    if episode["show_id"] != series {
        return None;
    }
    let expected = item["ProviderIds"]["Tmdb"]
        .as_str()
        .filter(|value| !value.is_empty());
    if expected.is_some_and(|expected| id(expected).map(u64::from) != Some(episode_id)) {
        return None;
    }
    let aired = episode["air_date"].as_str()?.parse::<Date>().ok()?;
    let stamp = item["PremiereDate"]
        .as_str()
        .filter(|date| !date.is_empty());
    if let Some(stamp) = stamp {
        let local_date = stamp.parse::<Date>().ok().or_else(|| {
            stamp
                .parse::<Timestamp>()
                .ok()
                .map(|stamp| timezone.to_datetime(stamp).date())
        })?;
        if local_date != aired {
            return None;
        }
    } else if expected.is_none() {
        let marker = format!("S{season:02}E{local:02}");
        let basename = item["Path"].as_str()?.rsplit(['/', '\\']).next()?;
        if !basename
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|part| part.eq_ignore_ascii_case(&marker))
        {
            return None;
        }
    }
    Some((
        Episode {
            series,
            season: episode["season_number"].as_u64().filter(|&n| n > 0)?,
            number: episode["episode_number"].as_u64().filter(|&n| n > 0)?,
            id: Some(episode_id),
        },
        aired,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_does_not_accept_specials_ambiguous_series_or_malformed_ids() {
        let group = "1234567890abcdef12345678";
        let subgroup = "abcdef1234567890abcdef12";
        let mapping = |series: &str, season: &str, group: &str| {
            json!({series:{season:{"group":group,"subgroup":subgroup}}}).to_string()
        };
        assert!(parse_groups(&mapping("16", "2", group)).is_ok());
        for input in [
            mapping("0", "2", group),
            mapping("16", "0", group),
            mapping("16", "2", "abcdef0123456789abcdefg0"),
            r#"{"16":{}}"#.to_owned(),
        ] {
            assert!(parse_groups(&input).err().unwrap().contains("有效的剧集 ID"));
        }
        assert!(parse_groups("[]").err().unwrap().contains("分组格式无效"));
        assert!(parse_groups(&json!({"16":{"2":{"group":group,"subgroup":subgroup,"typo":true}}}).to_string()).is_err());
    }

    #[test]
    fn alternate_order_requires_unique_episode_with_local_airdate_and_matching_ids() {
        let mapping = Group {
            group: "group".into(),
            subgroup: "subgroup".into(),
        };
        let mut group = json!({"groups":[{"id":"subgroup","order":3,"episodes":[
            {"order":0,"show_id":16,"season_number":1,"episode_number":40,"id":52,"air_date":"2026-01-02"}
        ]}]});
        let mut item = json!({"IndexNumber":1,"PremiereDate":"2026-01-01T16:00:00.0000000Z"});
        let zone = TimeZone::get("Asia/Shanghai").unwrap();
        let (episode, _) = resolve_episode(&group, &mapping, &item, 16, 3, &zone).unwrap();
        assert_eq!(
            (episode.season, episode.number, episode.id),
            (1, 40, Some(52))
        );
        assert!(resolve_episode(&group, &mapping, &item, 17, 3, &zone).is_none());
        assert!(resolve_episode(&group, &mapping, &item, 16, 3, &TimeZone::UTC).is_none());
        item["ProviderIds"] = json!({"Tmdb":"99"});
        assert!(resolve_episode(&group, &mapping, &item, 16, 3, &zone).is_none());
        item["ProviderIds"] = Value::Null;
        item["PremiereDate"] = Value::Null;
        item["Path"] = json!("/media/番剧/剧名 S03E10.mp4");
        assert!(resolve_episode(&group, &mapping, &item, 16, 3, &zone).is_none());
        item["Path"] = json!("/media/番剧/剧名 S03E01.mp4");
        assert!(resolve_episode(&group, &mapping, &item, 16, 3, &zone).is_some());
        let duplicate = group["groups"][0]["episodes"][0].clone();
        group["groups"][0]["episodes"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(resolve_episode(&group, &mapping, &item, 16, 3, &zone).is_none());
    }
}
