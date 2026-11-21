use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

const LABELS: &[&str] = &[
    "AV",
    "番号",
    "有码番号",
    "无码番号",
    "素人番号",
    "FC2番号",
    "欧美番号",
    "国产番号",
    "动漫里番",
    "MYWIFE",
];

static SPECIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:FC2[ .-]*(?:PPV[ .-]*)?(?<fc2>[0-9]{3,10})",
        r"|(?<amateur>[0-9]{3}[A-Z]{2,8})[ .-]*(?<amateur_n>[0-9]{3,6})",
        r"|(?<numbered>MY[ .-]*WIFE|HEYZO|RJ|VJ|BJ|MDX?|MDS)[ .-]*(?<numbered_n>[0-9]{3,10})",
        r"|HEYDOUGA[ .-]*(?<hey_maker>[0-9]{4})[ .-](?<hey_n>[0-9]{3,6})",
        r"|(?:TOKYO[ .-]*HOT[ .-]*)?N[ .-]*(?<tokyo>[0-9]{4,6})",
        r"|(?:(?<site>1PONDO|CARIB(?:BEANCOM)?|CARIBPR|10MUSUME|PACOPACOMAMA)[ .-]+)?(?<date>[0-9]{6})[ .-](?<serial>[0-9]{2,4})",
        r"|(?<studio>BRAZZERS|NAUGHTY[ .-]*AMERICA|REALITY[ .-]*KINGS|BANG[ .-]*BROS|DIGITAL[ .-]*PLAYGROUND|MOFOS|TEEN[ .-]*FIDELITY)[ .-]+(?<date1>[0-9]{2})[ .-](?<date2>[0-9]{2})[ .-](?<date3>[0-9]{2}))",
    ))
    .expect("AV identifier pattern")
});

pub(crate) struct Identifier {
    pub(crate) formatted: String,
    pub(crate) compact: String,
    western_date: bool,
}

impl Identifier {
    pub(crate) fn parse(query: &str) -> Result<Self, String> {
        let query = normalize(query);
        let query = strip_label(&query);
        let (formatted, western_date) = if let Some((code, end, western)) = special(query)
            && end == query.len()
        {
            (code, western)
        } else {
            (
                query
                    .split('-')
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("-"),
                false,
            )
        };
        if formatted.len() > 64
            || !formatted.bytes().any(|byte| byte.is_ascii_digit())
            || formatted.split('-').any(|part| {
                part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        {
            return Err("请输入有效的番号或商品 ID，例如 ABC-123、FC2-PPV-1234567。".into());
        }
        let compact = formatted.replace('-', "").to_ascii_lowercase();
        Ok(Self {
            formatted,
            compact,
            western_date,
        })
    }

    pub(crate) fn dvd_candidate(&self) -> String {
        if !self.formatted.contains('-') && special(&self.formatted).is_none() {
            let prefix_len = self
                .formatted
                .bytes()
                .take_while(u8::is_ascii_alphabetic)
                .count();
            if prefix_len > 0
                && self.formatted[prefix_len..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            {
                let (prefix, digits) = self.formatted.split_at(prefix_len);
                return format!("{prefix}-{digits}");
            }
        }
        self.formatted.clone()
    }

    pub(crate) fn search_candidate(&self) -> String {
        if self.western_date
            && let Some((studio, date)) = self.formatted.split_once('-')
        {
            return format!("{studio} {}", date.replace('-', "."));
        }
        self.dvd_candidate()
    }

    pub(crate) fn matches(&self, value: &str) -> bool {
        Self::parse(value).is_ok_and(|other| self.compact == other.compact)
    }

    // Only unambiguous formats go here; folder scanning applies its stricter
    // generic prefix/number heuristic separately.
    pub(crate) fn special_filename(stem: &str) -> Option<String> {
        let stem = normalize(stem);
        let stem = strip_label(&stem);
        let (code, end, _) = special(stem)?;
        let suffix = &stem[end..];
        (suffix.is_empty() || suffix.starts_with([' ', '.', '-', '[', '(', '【', '（']))
            .then_some(code)
    }
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .nfkc()
        .collect::<String>()
        .to_ascii_uppercase()
        .replace(['–', '—', '−', '_'], "-")
}

fn strip_label(value: &str) -> &str {
    if let Some((label, rest)) = value.split_once(':')
        && LABELS.contains(&label.trim())
    {
        return rest.trim();
    }
    value
}

fn special(value: &str) -> Option<(String, usize, bool)> {
    let found = SPECIAL.captures(value)?;
    let end = found.get(0)?.end();
    let get = |name| found.name(name).map(|part| part.as_str());
    let (code, western) = if let Some(n) = get("fc2") {
        (format!("FC2-PPV-{n}"), false)
    } else if let Some(prefix) = get("amateur") {
        (format!("{prefix}-{}", get("amateur_n")?), false)
    } else if let Some(prefix) = get("numbered") {
        (
            format!(
                "{}-{}",
                prefix.replace([' ', '.', '-'], ""),
                get("numbered_n")?
            ),
            false,
        )
    } else if let Some(maker) = get("hey_maker") {
        (format!("HEYDOUGA-{maker}-{}", get("hey_n")?), false)
    } else if let Some(n) = get("tokyo") {
        (format!("N{n}"), false)
    } else if let Some(date) = get("date") {
        // MMDDYY: don't turn arbitrary numeric filenames into AV IDs.
        if !matches!(date[..2].parse::<u8>(), Ok(1..=12))
            || !matches!(date[2..4].parse::<u8>(), Ok(1..=31))
        {
            return None;
        }
        let serial = get("serial")?;
        let code = match get("site") {
            Some("CARIBBEANCOM") => format!("CARIB-{date}-{serial}"),
            Some(site) => format!("{site}-{date}-{serial}"),
            None => format!("{date}-{serial}"),
        };
        (code, false)
    } else {
        let studio = get("studio")?.replace([' ', '.', '-'], "");
        let (a, b, c) = (get("date1")?, get("date2")?, get("date3")?);
        // Allow both YY.MM.DD and MM.DD.YY without rewriting the date order.
        let a_num = a.parse::<u8>().ok()?;
        let b_num = b.parse::<u8>().ok()?;
        let c_num = c.parse::<u8>().ok()?;
        if !((1..=12).contains(&b_num) && (1..=31).contains(&c_num)
            || (1..=12).contains(&a_num) && (1..=31).contains(&b_num))
        {
            return None;
        }
        (format!("{studio}-{a}-{b}-{c}"), true)
    };
    Some((code, end, western))
}
