use serde_json::{Value, json};

use super::{list, nonempty_text, normalize};
use crate::providers::media::episode_title::field_locked;

pub(crate) fn missing_people(item: &Value) -> bool {
    item["People"].as_array().is_some_and(Vec::is_empty)
        && !field_locked(item, "Cast")
        && !field_locked(item, "People")
}

pub(crate) fn record_people(record: &Value) -> Vec<Value> {
    let is_episode = record["scope"] == "episode";
    let mut people = list(&record["sources"])
        .filter(|source| source["provider"] == "TMDB")
        .map(|source| {
            let data = &source["data"];
            let credits = if is_episode {
                &data["episode"]["credits"]
            } else if data["aggregate_credits"].is_object() {
                &data["aggregate_credits"]
            } else {
                &data["credits"]
            };
            tmdb_people(credits)
        })
        .find(|people| !people.is_empty())
        .unwrap_or_default();
    for (field, kind, role) in [
        ("cast", "Actor", ""),
        ("voice_actors", "Actor", "(voice)"),
        ("directors", "Director", ""),
        ("writers", "Writer", ""),
        ("producers", "Producer", ""),
    ] {
        for name in list(&record["fields"][field]).filter_map(nonempty_text) {
            add_person(&mut people, &name, kind, role);
        }
    }
    people
}

pub(crate) fn tmdb_people(credits: &Value) -> Vec<Value> {
    let mut people = Vec::new();
    for (field, kind) in [("cast", "Actor"), ("guest_stars", "GuestStar")] {
        for credit in list(&credits[field]) {
            let Some(name) = nonempty_text(&credit["name"]) else {
                continue;
            };
            let mut roles = Vec::new();
            for role in std::iter::once(&credit["character"])
                .chain(list(&credit["roles"]).map(|entry| &entry["character"]))
                .filter_map(nonempty_text)
            {
                if !roles.contains(&role) {
                    roles.push(role);
                }
            }
            add_person(&mut people, &name, kind, &roles.join(" / "));
        }
    }
    for credit in list(&credits["crew"]) {
        let Some(name) = nonempty_text(&credit["name"]) else {
            continue;
        };
        for job in std::iter::once(&credit["job"])
            .chain(list(&credit["jobs"]).map(|entry| &entry["job"]))
            .filter_map(nonempty_text)
        {
            let kind = match job.as_str() {
                "Director" => "Director",
                "Writer" | "Screenplay" => "Writer",
                "Producer" | "Executive Producer" => "Producer",
                _ => continue,
            };
            add_person(&mut people, &name, kind, &job);
        }
    }
    people
}

fn add_person(people: &mut Vec<Value>, name: &str, kind: &str, role: &str) {
    let normalized = normalize(name);
    if !people.iter().any(|person| {
        person["Type"] == kind
            && person["Name"]
                .as_str()
                .is_some_and(|name| normalize(name) == normalized)
    }) {
        people.push(json!({"Name":name, "Type":kind, "Role":role}));
    }
}

#[cfg(test)]
#[test]
fn keeps_tmdb_roles_and_deduplicates_normalized_names() {
    let record = json!({"fields":{"cast":[" A "], "voice_actors":["声優"],
    "directors":["Director"], "writers":["Writer"]},
    "sources":[{"provider":"TMDB", "data":{"aggregate_credits":{
        "cast":[{"name":"Ａ", "roles":[{"character":"Rikka (voice)"},{"character":"Rikka (voice)"}]}],
        "crew":[{"name":"Director","jobs":[{"job":"Director"}]}]
    }}}]});
    assert_eq!(
        record_people(&record),
        json!([
            {"Name":"Ａ", "Type":"Actor", "Role":"Rikka (voice)"},
            {"Name":"Director", "Type":"Director", "Role":"Director"},
            {"Name":"声優", "Type":"Actor", "Role":"(voice)"},
            {"Name":"Writer", "Type":"Writer", "Role":""}
        ])
        .as_array()
        .unwrap()
        .as_slice()
    );
}
