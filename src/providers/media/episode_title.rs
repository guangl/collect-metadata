use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"^(?:第\s*[0-9一二三四五六七八九十百零〇两]+\s*[集话話回]|(?:episodes?|ep\.?|e|special)\s*[# ._-]*[0-9]+|s[0-9]+\s*[. _-]*e[0-9]+|tba|tbd|untitled|未命名分集|未命名|特别篇|特別篇)$"
).unwrap()
});
static FILE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^a-z0-9])s[0-9]+[. _-]*e[0-9]+(?:$|[^0-9])").unwrap());

pub(crate) fn usable<'a>(title: &'a Value, item: &Value) -> Option<&'a str> {
    title
        .as_str()
        .map(str::trim)
        .filter(|title| !placeholder(title, item))
}

pub(crate) fn placeholder(title: &str, item: &Value) -> bool {
    let folded = normalize(title);
    let bare = folded.trim_matches(|ch: char| ch.is_whitespace() || "[]()【】#".contains(ch));
    let filename = item["Path"]
        .as_str()
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .and_then(|name| name.rsplit_once('.'))
        .map(|(stem, _)| stem);
    bare.is_empty()
        || NUMBERED.is_match(bare)
        || bare
            .parse::<u64>()
            .ok()
            .is_some_and(|number| item["IndexNumber"].as_u64() == Some(number))
        || (FILE_NUMBER.is_match(&folded) && filename.is_some_and(|stem| normalize(stem) == folded))
}

fn normalize(s: &str) -> String {
    s.trim().nfkc().flat_map(char::to_lowercase).collect()
}

pub(crate) fn needs_title(item: &Value) -> bool {
    item["Type"] == "Episode"
        && !field_locked(item, "Name")
        && usable(&item["Name"], item).is_none()
}

pub(crate) fn field_locked(item: &Value, name: &str) -> bool {
    item["LockedFields"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|field| field == name)
}

#[cfg(test)]
#[test]
fn only_classifies_whole_placeholders() {
    let item = serde_json::json!({"Type":"Episode", "IndexNumber":7,
        "Path":r"D:\TV\剧名 S01E07.mkv", "Name":"第 ７ 集"});
    for name in [
        "",
        "【第七話】",
        "Episode # 07",
        "7",
        "S01E07",
        "剧名 S01E07",
    ] {
        assert!(placeholder(name, &item), "{name}");
    }
    for name in ["重逢", "86", "Episode 7: Reunion", "第七回的约定"] {
        assert!(!placeholder(name, &item), "{name}");
    }
    assert!(needs_title(&item));
    let mut locked = item;
    locked["LockedFields"] = serde_json::json!(["Name"]);
    assert!(!needs_title(&locked));
}
