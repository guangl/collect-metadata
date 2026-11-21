use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::LookupResponse;

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{([a-z]+)\}").unwrap());
pub(super) const DEFAULT_WORK_NAME: &str = "work-{row}";

pub(super) fn validate(value: &str) -> bool {
    if value.len() > 160
        || value
            .chars()
            .any(|c| c.is_control() || "/\\:<>|\"?*".contains(c))
    {
        return false;
    }
    TOKEN
        .captures_iter(value)
        .all(|part| matches!(&part[1], "title" | "year" | "id" | "type" | "row"))
        && !TOKEN.replace_all(value, "").contains(['{', '}'])
}

pub(super) struct Names<'a> {
    record: &'a LookupResponse,
    row: i64,
}

impl<'a> Names<'a> {
    pub(super) fn new(record: &'a LookupResponse, row: i64) -> Self {
        Self { record, row }
    }

    fn render(&self, pattern: &str) -> String {
        let record = &self.record.record;
        let fields = &record["fields"];
        let row = self.row.to_string();
        let title = ["title", "title_original", "title_ja", "title_en"]
            .into_iter()
            .find_map(|field| fields[field].as_str())
            .or_else(|| record["query"].as_str())
            .unwrap_or("work");
        let year = fields["release_date"]
            .as_str()
            .and_then(|date| date.get(..4))
            .unwrap_or("");
        let values = [
            ("title", title),
            ("year", year),
            ("id", self.record.code()),
            ("type", record["media_type"].as_str().unwrap_or("av")),
            ("row", &row),
        ];
        let rendered = TOKEN.replace_all(pattern, |part: &Captures<'_>| {
            values
                .iter()
                .find(|pair| pair.0 == &part[1])
                .map(|pair| pair.1)
                .unwrap_or_default()
        });
        sanitize(&rendered)
    }

    pub(super) fn directory(&self, pattern: &str) -> String {
        let name = self.render(pattern);
        if name.is_empty() || pattern == DEFAULT_WORK_NAME {
            format!("work-{}", self.row)
        } else {
            format!("{name}-{}", self.row)
        }
    }

    pub(super) fn files(&self, pattern: &str) -> FileNames {
        FileNames(self.render(pattern))
    }
}

pub(super) struct FileNames(String);

impl FileNames {
    pub(super) fn file(&self, base: &str) -> String {
        if self.0.is_empty() {
            base.to_owned()
        } else {
            format!("{}-{base}", self.0)
        }
    }
}

fn sanitize(value: &str) -> String {
    let mut result = String::new();
    for char in value.trim().chars() {
        if result.len() + char.len_utf8() > 120 {
            break;
        }
        result.push(if char.is_control() || "/\\:<>|\"?*".contains(char) {
            '_'
        } else {
            char
        });
    }
    let result = result.trim_matches(['.', ' ', '-', '_']);
    if reserved(result.split('.').next().unwrap_or_default()) {
        format!("_{result}")
    } else {
        result.to_owned()
    }
}

fn reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper.get(3..).is_some_and(|number| {
            matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        }) && (upper.starts_with("COM") || upper.starts_with("LPT"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn validates_templates_and_sanitizes_values_for_both_platforms() {
        for pattern in ["", "{title} ({year})", "{type}-{id}-{row}", "影片"] {
            assert!(validate(pattern), "{pattern}");
        }
        for pattern in [
            "../{title}",
            "{path}",
            "{{title}}",
            "{title",
            "foo\\bar",
            "a\nb",
        ] {
            assert!(!validate(pattern), "{pattern}");
        }
        assert!(!validate(&"片".repeat(54)));

        let record = LookupResponse {
            record: json!({"media_type":"movie","work_id":"tmdb:10","query":"Title",
                "fields":{"title":"..A/B: 片名. ","release_date":"2024-02-01"}}),
            saved: None,
            storage_error: None,
        };
        let names = Names::new(&record, 42);
        assert_eq!(names.directory(DEFAULT_WORK_NAME), "work-42");
        assert_eq!(names.directory(""), "work-42");
        assert_eq!(names.directory("{title} ({year})"), "A_B_ 片名.  (2024)-42");
        assert_eq!(names.files("{id}").file("poster.jpg"), "tmdb_10-poster.jpg");
        assert_eq!(names.files("").file("poster.jpg"), "poster.jpg");
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("lpt1.txt"), "_lpt1.txt");
        assert_eq!(sanitize("lpt0.txt"), "lpt0.txt");
        assert_eq!(sanitize(&"片".repeat(41)), "片".repeat(40));
    }
}
