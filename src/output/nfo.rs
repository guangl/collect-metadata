use std::{
    collections::{BTreeMap, HashSet},
    fmt::Write,
    path::Path,
    sync::LazyLock,
};

use regex::Regex;
use serde_json::Value;

use super::{OutputFile, media, settings::OutputOptions};

pub(super) fn work_xml(record: &Value, options: &OutputOptions, files: &[OutputFile]) -> String {
    let fields = &record["fields"];
    let episode = record["scope"] == "episode";
    let season = record["scope"] == "season";
    let series = record["media_type"] == "series" && !episode && !season;
    let root = if episode { "episodedetails" } else if season { "season" } else if series { "tvshow" } else { "movie" };
    let mut body = String::new();
    let av = record["media_type"] == "av";
    let original_title = field(fields, &["title_original", "title_ja"]);
    let title = field(
        fields,
        &[
            "title",
            "title_site",
            "title_original",
            "title_ja",
            "title_en",
        ],
    )
    .or_else(|| present(&record["query"]));
    let dvd_id = if av {
        present(&fields["dvd_id"]).or_else(|| present(&record["query"]))
    } else {
        None
    };
    let release = present(&fields["release_date"]);
    let collection = field(fields, &["collection", "series"]);
    let label = field(fields, &["label", "publisher"]);

    tag(&mut body, "title", title);
    tag(
        &mut body,
        "sorttitle",
        present(&fields["sort_title"]).or(dvd_id).or(title),
    );
    tag(&mut body, "originaltitle", original_title);
    for name in ["premiered", "release", "releasedate"] {
        tag(&mut body, name, release);
    }
    let year = fields["year"].as_u64().map(|value| value.to_string());
    tag(
        &mut body,
        "year",
        year.as_deref().or_else(|| {
            release
                .and_then(|date| date.get(..4))
                .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        }),
    );
    if season {
        number_tag(&mut body, "seasonnumber", fields["season_number"].as_u64());
    }
    if episode {
        tag(&mut body, "showtitle", present(&fields["series"]));
        tag(&mut body, "aired", release);
        number_tag(&mut body, "season", fields["season_number"].as_u64());
        number_tag(&mut body, "episode", fields["episode_number"].as_u64());
        number_tag(&mut body, "episodenumberend", fields["episode_last_number"].as_u64());
    }
    number_tag(&mut body, "runtime", fields["runtime_minutes"].as_u64());
    for (kind, key) in [
        ("tmdb", "tmdb_id"),
        ("imdb", "imdb_id"),
        ("tvdb", "tvdb_id"),
        ("bangumi", "bangumi_id"),
    ] {
        if let Some(id) = present(&fields[key]) {
            let _ = writeln!(
                body,
                "  <uniqueid type=\"{kind}\">{}</uniqueid>",
                escape(id)
            );
        }
    }
    if options.synopsis {
        synopsis(&mut body, fields);
        let release_tagline = release
            .filter(|_| av)
            .map(|date| format!("发行日期 {date}"));
        tag(
            &mut body,
            "tagline",
            present(&fields["tagline"]).or(release_tagline.as_deref()),
        );
    }
    tag(&mut body, "mpaa", present(&fields["official_rating"]));
    tag(&mut body, "customrating", present(&fields["custom_rating"]));
    if options.ratings {
        rating_tags(&mut body, fields);
    }
    for (key, name) in [
        ("genres", "genre"),
        ("countries", "country"),
        ("studio", "studio"),
        ("studio", "maker"),
    ] {
        for value in unique(texts(&fields[key])) {
            tag(&mut body, name, Some(value));
        }
    }
    tag(&mut body, "publisher", label);
    tag(&mut body, "label", label);
    tag(&mut body, "series", present(&fields["series"]));
    if !series && !episode && let Some(collection) = collection {
        let _ = writeln!(body, "  <set><name>{}</name></set>", escape(collection));
    }
    tag(&mut body, "num", dvd_id);

    let actors = || {
        unique(
            ["cast", "voice_actors", "actresses", "actors", "other_cast"]
                .into_iter()
                .flat_map(|key| texts(&fields[key])),
        )
    };
    if options.credits {
        for (key, name) in [("directors", "director"), ("writers", "credits")] {
            for value in unique(texts(&fields[key])) {
                tag(&mut body, name, Some(value));
            }
        }
        for actor in actors() {
            let _ = writeln!(body, "  <actor><name>{}</name></actor>", escape(actor));
        }
    }

    let mut tags = texts(&fields["tags"])
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if av {
        tags.extend(texts(&fields["genres"]).map(str::to_owned));
        tags.extend(
            dvd_id
                .and_then(|id| id.split_once('-'))
                .filter(|(prefix, _)| prefix.bytes().any(|byte| byte.is_ascii_alphabetic()))
                .map(|(prefix, _)| prefix.to_owned()),
        );
        tags.extend(present(&fields["censorship"]).map(str::to_owned));
        for (prefix, values) in [
            ("系列", collection.into_iter().collect::<Vec<_>>()),
            ("片商", texts(&fields["studio"]).collect()),
            ("发行", label.into_iter().collect()),
        ] {
            tags.extend(values.into_iter().map(|value| format!("{prefix}：{value}")));
        }
        if options.credits {
            tags.extend(actors().map(str::to_owned));
        }
    }
    tags.extend(present(&fields["resolution"]).map(str::to_owned));
    if fields["has_chinese_subtitles"] == true {
        tags.push("中文字幕".into());
    }
    for value in unique(tags.iter().map(String::as_str)) {
        tag(&mut body, "tag", Some(value));
    }
    let image = |kind, keys: &[&str]| {
        files
            .iter()
            .find(|file| file.kind == kind)
            .map(|file| file.path.as_str())
            .or_else(|| keys.iter().find_map(|key| media::web_url(&fields[*key])))
    };
    let poster = image("poster", &["poster_url", "cover_url"])
        .or_else(|| if episode { image("still", &[]) } else { None });
    let cover = image(
        "backdrop",
        if av {
            &["backdrop_url", "cover_url"]
        } else {
            &["backdrop_url"]
        },
    );
    tag(&mut body, "poster", poster);
    tag(&mut body, "thumb", poster);
    tag(&mut body, "cover", cover);
    if let Some(cover) = cover {
        let _ = writeln!(body, "  <fanart><thumb>{}</thumb></fanart>", escape(cover));
    }
    let trailer = media::links(record)
        .into_iter()
        .find(|link| link.kind == media::Kind::Trailer);
    tag(
        &mut body,
        "trailer",
        media::web_url(&fields["trailer_url"])
            .or_else(|| trailer.as_ref().map(|link| link.url.as_str())),
    );
    tag(
        &mut body,
        "website",
        media::web_url(&fields["official_url"]).or_else(|| {
            record["sources"]
                .as_array()?
                .iter()
                .filter(|source| source["status"] == "ok" || source["status"] == "partial")
                .find_map(|source| media::web_url(&source["url"]))
        }),
    );
    xml(root, &body)
}

pub(super) fn documents(
    record: &Value,
    options: &OutputOptions,
    source_files: &[String],
    files: &[OutputFile],
) -> BTreeMap<String, String> {
    let work = work_xml(record, options, files);
    let fields = &record["fields"];
    let episode = record["scope"] == "episode";
    let series = record["media_type"] == "series" && !episode;
    let mut result = BTreeMap::new();
    if series {
        result.insert("tvshow.nfo".to_owned(), work);
        if options.episodes {
            let paths = source_files
                .iter()
                .filter_map(|path| episode_path(path))
                .collect::<Vec<_>>();
            let regular_seasons = paths
                .iter()
                .map(|&(_, season, _)| season)
                .filter(|&season| season > 0)
                .collect::<HashSet<_>>();
            let bangumi_episodes = record["field_sources"]["episodes"] == "Bangumi";
            let subject_episodes = bangumi_episodes && regular_seasons.len() == 1;
            for (name, season, number) in paths {
                let mut matching =
                    fields["episodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|episode| {
                            episode["number"].as_u64() == Some(number)
                                && (episode["season"].as_u64() == Some(season)
                                    || (subject_episodes
                                        && season > 0
                                        && episode["season"].is_null()))
                        });
                if let Some(episode) = matching.next()
                    && (!bangumi_episodes || matching.next().is_none())
                {
                    result.insert(
                        format!("{name}.nfo"),
                        episode_xml(episode, season, number, options),
                    );
                }
            }
            if source_files.is_empty() {
                for episode in fields["episodes"].as_array().into_iter().flatten() {
                    if let (Some(name), Some(season), Some(number)) = (
                        episode["file_name"].as_str().and_then(filename),
                        episode["season"].as_u64(),
                        episode["number"].as_u64(),
                    ) {
                        result.insert(
                            format!("{name}.nfo"),
                            episode_xml(episode, season, number, options),
                        );
                    }
                }
            }
        }
    } else {
        for name in source_files.iter().filter_map(|path| filename(path)) {
            result.insert(format!("{name}.nfo"), work.clone());
        }
        if result.is_empty() {
            let name = fields["file_name"]
                .as_str()
                .and_then(filename)
                .unwrap_or(if episode { "episode" } else { "movie" });
            result.insert(format!("{name}.nfo"), work);
        }
    }
    result
}

fn episode_xml(episode: &Value, season: u64, number: u64, options: &OutputOptions) -> String {
    let mut parts = String::new();
    tag(&mut parts, "title", episode["title"].as_str());
    tag(&mut parts, "season", Some(&season.to_string()));
    tag(&mut parts, "episode", Some(&number.to_string()));
    tag(&mut parts, "aired", episode["date"].as_str());
    if options.synopsis {
        synopsis(&mut parts, episode);
    }
    number_tag(&mut parts, "runtime", episode["runtime_minutes"].as_u64());
    if options.ratings {
        rating_tags(&mut parts, episode);
    }
    xml("episodedetails", &parts)
}

fn filename(relative: &str) -> Option<&str> {
    let name = Path::new(relative).file_stem()?.to_str()?;
    (!name.is_empty() && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']))
        .then_some(name)
}

fn episode_path(relative: &str) -> Option<(&str, u64, u64)> {
    static EP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:^|[ ._-])(?:S(\d{1,3})[ ._-]*E(\d{1,4})|(\d{1,2})x(\d{1,4}))(?:$|[^0-9])",
        )
        .unwrap()
    });
    let name = filename(relative)?;
    let found = EP.captures(name)?;
    let season = found
        .get(1)
        .or_else(|| found.get(3))?
        .as_str()
        .parse()
        .ok()?;
    let number = found
        .get(2)
        .or_else(|| found.get(4))?
        .as_str()
        .parse()
        .ok()?;
    Some((name, season, number))
}

fn texts(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
}

fn present(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn field<'a>(fields: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| present(&fields[*key]))
}

fn unique<'a>(values: impl Iterator<Item = &'a str>) -> impl Iterator<Item = &'a str> {
    let mut seen = HashSet::new();
    values
        .map(str::trim)
        .filter(move |value| !value.is_empty() && seen.insert(*value))
}

fn synopsis(body: &mut String, fields: &Value) {
    let plot = present(&fields["description"]);
    tag(body, "plot", plot);
    tag(body, "outline", plot);
    tag(
        body,
        "originalplot",
        present(&fields["description_original"]),
    );
}

fn number_tag(body: &mut String, name: &str, value: Option<impl ToString>) {
    tag(body, name, value.map(|value| value.to_string()).as_deref());
}

fn rating_tags(body: &mut String, fields: &Value) {
    let rating = &fields["rating"];
    if let Some(score) = rating["value"].as_f64() {
        let scale = rating["scale"]
            .as_f64()
            .filter(|value| *value > 0.0)
            .unwrap_or(10.0);
        if (0.0..=scale).contains(&score) {
            let score = (score / scale * 1000.0).round() / 100.0;
            number_tag(body, "rating", Some(score));
            number_tag(body, "score", Some(score));
        }
    }
    number_tag(body, "votes", rating["count"].as_u64());
    number_tag(body, "criticrating", fields["critic_rating"].as_f64());
    let imdb = &fields["imdb_rating"];
    if let (Some(value), Some(count)) = (imdb["value"].as_f64(), imdb["count"].as_u64())
        && (1.0..=10.0).contains(&value) && count > 0
    {
        let _ = writeln!(body,
            "  <ratings><rating name=\"imdb\" max=\"10\" default=\"false\"><value>{value}</value><votes>{count}</votes></rating></ratings>"
        );
    }
}

fn tag(body: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(body, "  <{name}>{}</{name}>", escape(value));
    }
}

fn xml(root: &str, body: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{root}>\n{body}</{root}>\n")
}

fn escape(text: &str) -> String {
    let mut result = String::new();
    for char in text.chars().filter(|char| matches!(*char, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        match char {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '&' => result.push_str("&amp;"),
            _ => result.push(char),
        }
    }
    result
}

#[cfg(feature = "web")]
pub(super) fn fill_episode_title(xml: &str, item: &Value, title: &str) -> Option<String> {
    use crate::providers::media::episode_title::placeholder;

    let (root_end, fields) = nfo_fields(
        xml,
        "episodedetails",
        &[
            "title",
            "sorttitle",
            "season",
            "episode",
            "lockdata",
            "lockedfields",
        ],
    )?;
    let text = |key: &str| nfo_text(xml, &fields, key);
    for (field, number) in [("season", "ParentIndexNumber"), ("episode", "IndexNumber")] {
        if text(field)?.parse::<u64>().ok()? != item[number].as_u64()? {
            return None;
        }
    }
    let locks = text("lockedfields").unwrap_or_default();
    let locked = |name: &str| {
        locks
            .split('|')
            .any(|field| field.trim().eq_ignore_ascii_case(name))
    };
    if text("lockdata").is_some_and(|s| s.eq_ignore_ascii_case("true"))
        || locked("Name")
        || (fields.contains_key("title") && !placeholder(&text("title")?, item))
    {
        return None;
    }
    let title = escape(title);
    let change = |name: &str| {
        let (range, whole_tag) = fields
            .get(name)
            .cloned()
            .unwrap_or((root_end..root_end, true));
        let replacement = if whole_tag {
            format!("<{name}>{title}</{name}>")
        } else {
            title.clone()
        };
        (range, replacement)
    };
    let mut changes = vec![change("title")];
    if !locked("SortName") && text("sorttitle").is_some_and(|value| placeholder(&value, item)) {
        changes.push(change("sorttitle"));
    }
    changes.sort_unstable_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut result = xml.to_owned();
    for (range, replacement) in changes {
        result.replace_range(range, &replacement);
    }
    Some(result)
}

#[cfg(feature = "web")]
pub(super) fn fill_season_plot(xml: &str, number: u64, plot: &str) -> Option<String> {
    let (root_end, fields) = nfo_fields(
        xml,
        "season",
        &["seasonnumber", "plot", "lockdata", "lockedfields"],
    )?;
    let text = |key: &str| nfo_text(xml, &fields, key);
    if text("seasonnumber")?.parse::<u64>().ok()? != number
        || text("lockdata").is_some_and(|s| s.eq_ignore_ascii_case("true"))
        || text("lockedfields")
            .unwrap_or_default()
            .split('|')
            .any(|field| field.trim().eq_ignore_ascii_case("Overview"))
        || fields.contains_key("plot") && !text("plot")?.trim().is_empty()
    {
        return None;
    }
    let (range, whole_tag) = fields
        .get("plot")
        .cloned()
        .unwrap_or((root_end..root_end, true));
    let escaped = escape(plot);
    let replacement = if whole_tag {
        format!("<plot>{escaped}</plot>")
    } else {
        escaped
    };
    let mut result = xml.to_owned();
    result.replace_range(range, &replacement);
    Some(result)
}

#[cfg(feature = "web")]
type NfoFields = BTreeMap<String, (std::ops::Range<usize>, bool)>;

#[cfg(feature = "web")]
fn nfo_fields(xml: &str, root: &str, tracked: &[&str]) -> Option<(usize, NfoFields)> {
    use quick_xml::{Reader, events::Event};

    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut root_end = 0;
    let mut start = 0;
    let mut fields = NfoFields::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event().ok()? {
            Event::Start(tag) => {
                if depth == 0 {
                    if root_seen || tag.name().as_ref() != root {
                        return None;
                    }
                    root_seen = true;
                    root_end = reader.buffer_position() as usize;
                } else if depth == 1 {
                    start = reader.buffer_position() as usize;
                }
                depth += 1;
            }
            Event::End(tag) => {
                depth = depth.checked_sub(1)?;
                if depth == 1 {
                    let name = tag.name();
                    let name = name.as_ref();
                    if tracked.contains(&name)
                        && fields.insert(name.into(), (start..before, false)).is_some()
                    {
                        return None;
                    }
                }
            }
            Event::Empty(tag) if depth == 1 => {
                let name = tag.name();
                let name = name.as_ref();
                let end = reader.buffer_position() as usize;
                if tracked.contains(&name)
                    && fields.insert(name.into(), (before..end, true)).is_some()
                {
                    return None;
                }
            }
            Event::DocType(_) => return None,
            Event::Eof if root_seen && depth == 0 => break,
            Event::Eof => return None,
            _ => {}
        }
    }
    Some((root_end, fields))
}

#[cfg(feature = "web")]
fn nfo_text<'a>(xml: &'a str, fields: &NfoFields, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    let (range, empty) = fields.get(key)?;
    let raw = if *empty {
        ""
    } else {
        xml.get(range.clone())?.trim()
    };
    (!raw.contains('<'))
        .then(|| quick_xml::escape::unescape(raw).ok())
        .flatten()
}

#[cfg(all(test, feature = "web"))]
#[test]
fn season_plot_patch_preserves_existing_values_and_validates_season() {
    let original = "<?xml version=\"1.0\"?><season><title>第 2 季</title><seasonnumber>2</seasonnumber><plot /></season>";
    let patched = fill_season_plot(original, 2, "A & B").unwrap();
    assert_eq!(
        patched,
        original.replace("<plot />", "<plot>A &amp; B</plot>")
    );
    assert!(fill_season_plot(&patched, 2, "Other").is_none());
    assert!(fill_season_plot(original, 3, "Other").is_none());
    let locked = original.replace("<plot />", "<lockedfields>Overview</lockedfields><plot />");
    assert!(fill_season_plot(&locked, 2, "Other").is_none());
    let no_plot = original.replace("<plot />", "");
    let added = fill_season_plot(&no_plot, 2, "A & B").unwrap();
    assert_eq!(added.replace("<plot>A &amp; B</plot>", ""), no_plot);
}

#[cfg(all(test, feature = "web"))]
#[test]
fn existing_nfo_title_patch_keeps_other_bytes_and_requires_matching_episode() {
    let original = "<?xml version=\"1.0\"?>\n<episodedetails>\n  <title>第 7 集</title>\n  <sorttitle>Episode 7</sorttitle>\n  <season>1</season><episode>7</episode>\n  <actor><title>第 7 集</title></actor><plot>A &amp; B</plot>\n</episodedetails>";
    let mut item = serde_json::json!({"IndexNumber":7,"ParentIndexNumber":1});
    let patched = fill_episode_title(original, &item, "A & B").unwrap();
    assert_eq!(patched, original.replacen("第 7 集", "A &amp; B", 1).replacen("Episode 7", "A &amp; B", 1));
    assert!(fill_episode_title(&patched, &item, "Different").is_none());
    let blank = original.replacen("<title>第 7 集</title>", "<title/>", 1);
    assert!(fill_episode_title(&blank, &item, "Filled").unwrap().contains("<title>Filled</title>"));
    assert!(fill_episode_title(&original.replace("<season>", "<lockedfields>Name|Overview</lockedfields><season>"), &item, "Different").is_none());
    assert!(fill_episode_title(&original.replace("<season>", "<lockdata>true</lockdata><season>"), &item, "Different").is_none());
    item["ParentIndexNumber"] = serde_json::json!(2);
    assert!(fill_episode_title(original, &item, "Different").is_none());
}

#[cfg(all(test, feature = "web"))]
#[test]
fn season_plot_patch_rejects_ambiguous_or_incomplete_documents() {
    for xml in [
        "<movie><seasonnumber>2</seasonnumber><plot/></movie>",
        "<season><seasonnumber>2</seasonnumber><plot/><plot/></season>",
        "<season><seasonnumber>2</seasonnumber><plot>Existing</plot><plot>Duplicate</plot></season>",
        "<!DOCTYPE season><season><seasonnumber>2</seasonnumber><plot/></season>",
        "<season><seasonnumber>2</seasonnumber><plot/>",
    ] {
        assert!(fill_season_plot(xml, 2, "New plot").is_none(), "{xml}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn writes_compatibility_fields_from_real_values() {
        let record = json!({
            "media_type": "av", "query": "ABC-123",
            "fields": {
                "title": "译名", "title_ja": "原題", "release_date": "2024-04-03",
                "description": "简介", "description_original": "原文 & <>",
                "rating": {"value": 4.1, "scale": 5, "count": 45},
                "official_rating": "R", "custom_rating": "PG", "critic_rating": 92,
                "studio": ["Studio"], "label": "Label", "series": "Title & Series",
                "genres": ["Drama"], "tags": ["Drama"], "cast": ["Actor"], "actresses": ["Actor"],
                "trailer_url": "https://www.youtube.com/watch?v=demo&x=2",
                "official_url": "https://example.com/film"
            }
        });
        let options = OutputOptions::default();
        let files = [OutputFile {
            path: "poster.jpg".into(),
            kind: "poster",
            source_url: None,
            person: None,
        }];
        let docs = documents(&record, &options, &[], &files);
        let nfo = &docs["movie.nfo"];
        for expected in [
            "<title>译名</title>",
            "<sorttitle>ABC-123</sorttitle>",
            "<originaltitle>原題</originaltitle>",
            "<outline>简介</outline>",
            "<originalplot>原文 &amp; &lt;&gt;</originalplot>",
            "<releasedate>2024-04-03</releasedate>",
            "<mpaa>R</mpaa>",
            "<customrating>PG</customrating>",
            "<rating>8.2</rating>",
            "<score>8.2</score>",
            "<votes>45</votes>",
            "<criticrating>92</criticrating>",
            "<maker>Studio</maker>",
            "<publisher>Label</publisher>",
            "<set><name>Title &amp; Series</name></set>",
            "<tag>片商：Studio</tag>",
            "<poster>poster.jpg</poster>",
            "<trailer>https://www.youtube.com/watch?v=demo&amp;x=2</trailer>",
            "<website>https://example.com/film</website>",
        ] {
            assert!(nfo.contains(expected), "{expected} missing from {nfo}");
        }
        assert_eq!(nfo.matches("<actor>").count(), 1);
        assert_eq!(nfo.matches("<tag>Drama</tag>").count(), 1);
        let options =
            serde_json::from_value(json!({"synopsis":false, "ratings":false, "credits":false}))
                .unwrap();
        let docs = documents(&record, &options, &[], &[]);
        for field in [
            "originalplot",
            "outline",
            "criticrating",
            "rating",
            "votes",
            "actor",
        ] {
            assert!(!docs["movie.nfo"].contains(&format!("<{field}>")));
        }
    }

    #[test]
    fn exports_artwork_and_imdb_votes_with_a_safe_video_name() {
        let record = json!({
            "media_type": "movie",
            "query": "Fallback title",
            "fields": {
                "backdrop_url": "https://example.com/fanart.jpg",
                "imdb_rating": {"value": 8.1, "count": 370}
            }
        });
        let paths = ["movie.mkv".to_owned(), "unsafe:name.mkv".to_owned()];
        let docs = documents(&record, &OutputOptions::default(), &paths, &[]);
        assert_eq!(docs.len(), 1);
        let xml = &docs["movie.nfo"];
        assert!(xml.contains("<title>Fallback title</title>"));
        assert!(xml.contains("<fanart><thumb>https://example.com/fanart.jpg</thumb></fanart>"));
        assert!(xml.contains("<value>8.1</value><votes>370</votes>"));
    }

    #[test]
    fn exports_bangumi_subject_episodes_only_for_one_numbered_season() {
        let record = json!({
            "media_type": "series",
            "field_sources": {"episodes": "Bangumi"},
            "fields": {
                "title": "动画",
                "bangumi_id": "253",
                "episodes": [{"season": null, "number": 2, "title": "本季第二集"}],
            },
        });
        let mut options = OutputOptions::default();
        options.episodes = true;
        let mut paths = vec!["Season 02/S02E02.mkv".to_owned()];
        let docs = documents(&record, &options, &paths, &[]);
        assert!(docs["tvshow.nfo"].contains("<uniqueid type=\"bangumi\">253</uniqueid>"));
        assert!(docs["S02E02.nfo"].contains("<title>本季第二集</title>"));
        assert!(docs["S02E02.nfo"].contains("<season>2</season>"));

        paths.push("Season 01/S01E02.mkv".to_owned());
        let docs = documents(&record, &options, &paths, &[]);
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("tvshow.nfo"));
    }

    #[test]
    fn exports_special_episodes_and_preserves_xml_and_video_names() {
        let record = json!({"media_type":"series","query":"标题",
            "fields":{"title":"A & B <C>\u{0001}","imdb_id":"tt01","genres":["动画"],
                "cast":["演员"], "writers":["编剧"], "tagline":"标语",
                "episodes":[{"season":0,"number":3,"title":"特别篇","date":"2024-01-01","description":"简介"},
                    {"season":1,"number":2,"title":"第二集","file_name":"Series 1x02.mkv"}]}});
        let options = OutputOptions::default();
        let paths = [
            "Shows/Series S00E003.mkv",
            "Shows/Series 1x02.mkv",
            "Shows/Series S03E01.mkv",
        ]
        .map(str::to_owned);
        let docs = documents(&record, &options, &paths, &[]);
        assert_eq!(docs.len(), 3);
        assert!(docs["tvshow.nfo"].contains("A &amp; B &lt;C&gt;</title>"));
        assert!(docs["tvshow.nfo"].contains("<name>演员</name>"));
        assert!(docs["tvshow.nfo"].contains("<uniqueid type=\"imdb\">tt01</uniqueid>"));
        assert!(docs["Series S00E003.nfo"].contains("<plot>简介</plot>"));
        assert!(docs["Series 1x02.nfo"].contains("<season>1</season>"));
        assert_eq!(documents(&record, &options, &[], &[]).len(), 2);
    }
}
