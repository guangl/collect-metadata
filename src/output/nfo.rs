use std::{collections::BTreeMap, fmt::Write, path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

use super::settings::OutputOptions;

pub(super) fn documents(
    record: &Value,
    options: &OutputOptions,
    source_files: &[String],
) -> BTreeMap<String, String> {
    let fields = &record["fields"];
    let series = record["media_type"] == "series";
    let root = if series { "tvshow" } else { "movie" };
    let mut body = String::new();
    let title = fields["title"]
        .as_str()
        .or_else(|| fields["title_original"].as_str())
        .or_else(|| record["query"].as_str());
    tag(&mut body, "title", title);
    tag(
        &mut body,
        "originaltitle",
        fields["title_original"].as_str(),
    );
    tag(&mut body, "premiered", fields["release_date"].as_str());
    tag(
        &mut body,
        "year",
        fields["release_date"]
            .as_str()
            .and_then(|date| date.get(..4)),
    );
    tag(
        &mut body,
        "runtime",
        fields["runtime_minutes"]
            .as_u64()
            .map(|value| value.to_string())
            .as_deref(),
    );
    for (kind, key) in [("tmdb", "tmdb_id"), ("imdb", "imdb_id")] {
        if let Some(id) = fields[key].as_str() {
            let _ = writeln!(
                body,
                "  <uniqueid type=\"{kind}\">{}</uniqueid>",
                escape(id)
            );
        }
    }
    if options.synopsis {
        tag(&mut body, "plot", fields["description"].as_str());
        tag(&mut body, "tagline", fields["tagline"].as_str());
    }
    if options.ratings {
        tag(
            &mut body,
            "rating",
            fields["rating"]["value"]
                .as_f64()
                .map(|value| value.to_string())
                .as_deref(),
        );
    }
    for (key, name) in [
        ("genres", "genre"),
        ("countries", "country"),
        ("studio", "studio"),
    ] {
        for value in texts(&fields[key]) {
            tag(&mut body, name, Some(value));
        }
    }
    if options.credits {
        for (key, name) in [("directors", "director"), ("writers", "credits")] {
            for value in texts(&fields[key]) {
                tag(&mut body, name, Some(value));
            }
        }
        for actor in ["cast", "actresses", "actors", "other_cast"]
            .into_iter()
            .flat_map(|key| texts(&fields[key]))
        {
            let _ = writeln!(body, "  <actor><name>{}</name></actor>", escape(actor));
        }
    }
    let work = xml(root, &body);
    let mut result = BTreeMap::new();
    if series {
        result.insert("tvshow.nfo".to_owned(), work);
        if options.episodes {
            for path in source_files {
                if let Some((name, season, number)) = episode_path(path)
                    && let Some(episode) =
                        fields["episodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|episode| {
                                episode["season"].as_u64() == Some(season)
                                    && episode["number"].as_u64() == Some(number)
                            })
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
                .unwrap_or("movie");
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
        tag(&mut parts, "plot", episode["description"].as_str());
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
        let docs = documents(&record, &options, &paths);
        assert_eq!(docs.len(), 3);
        assert!(docs["tvshow.nfo"].contains("A &amp; B &lt;C&gt;</title>"));
        assert!(docs["tvshow.nfo"].contains("<name>演员</name>"));
        assert!(docs["tvshow.nfo"].contains("<uniqueid type=\"imdb\">tt01</uniqueid>"));
        assert!(docs["Series S00E003.nfo"].contains("<plot>简介</plot>"));
        assert!(docs["Series 1x02.nfo"].contains("<season>1</season>"));
        assert_eq!(documents(&record, &options, &[]).len(), 2);
    }
}
