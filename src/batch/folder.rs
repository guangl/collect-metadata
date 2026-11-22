use std::{borrow::Cow, collections::HashMap, sync::LazyLock};

use regex::Regex;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use super::BatchItemInput;
use crate::{MediaKind, collect::Identifier};

pub const MAX_FOLDER_FILES: usize = 30_000;
const MAX_PATH_BYTES: usize = 6 * 1024 * 1024;
const MAX_WORKS: usize = 2_000;

#[derive(Serialize)]
pub struct FolderBatchResult {
    pub id: Option<i64>,
    pub works: usize,
    pub files: usize,
    pub skipped: usize,
    pub unrecognized: Vec<String>,
}

#[derive(Serialize)]
pub struct FolderWorkPreview {
    pub kind: MediaKind,
    pub query: String,
    pub file_count: usize,
}

#[derive(Serialize)]
pub struct FolderPreview {
    pub works: Vec<FolderWorkPreview>,
    pub files: usize,
    pub skipped: usize,
    pub unrecognized: Vec<String>,
}

#[derive(Serialize)]
pub struct FolderSelection {
    pub folder: String,
    pub paths: Vec<String>,
    pub preview: FolderPreview,
}

impl FolderSelection {
    pub(crate) fn new(folder: String, paths: Vec<String>) -> Result<Self, String> {
        let preview = preview_folder(paths.clone())?;
        Ok(Self {
            folder,
            paths,
            preview,
        })
    }
}

pub fn preview_folder(paths: Vec<String>) -> Result<FolderPreview, String> {
    let (items, info) = classify_paths(paths)?;
    Ok(FolderPreview {
        works: items
            .into_iter()
            .map(|item| FolderWorkPreview {
                kind: item.kind,
                query: item.query,
                file_count: item.source_files.len(),
            })
            .collect(),
        files: info.files,
        skipped: info.skipped,
        unrecognized: info.unrecognized,
    })
}

#[cfg(feature = "desktop")]
pub fn scan_folder_paths(root: &std::path::Path) -> Result<Vec<String>, String> {
    let directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|_| "无法打开所选目录，请检查挂载和读取权限。")?;
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("媒体目录");
    scan_open_folder(&directory, name)
}

#[cfg(feature = "desktop")]
pub fn scan_native_folder(root: &std::path::Path) -> Result<FolderSelection, String> {
    FolderSelection::new(
        root.to_string_lossy().into_owned(),
        scan_folder_paths(root)?,
    )
}

#[cfg(any(feature = "desktop", feature = "web"))]
pub(super) fn scan_open_folder(root: &cap_std::fs::Dir, name: &str) -> Result<Vec<String>, String> {
    let mut directories = vec![(std::path::PathBuf::from("."), name.to_owned())];
    let mut paths = Vec::new();
    let mut total_bytes = 0;
    let mut visited = 0;
    let unreadable = |_| "无法读取所选目录中的部分文件，请检查目录权限。";
    while let Some((directory, relative)) = directories.pop() {
        for entry in root.read_dir(&directory).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let file_type = entry.file_type().map_err(unreadable)?;
            let name = entry.file_name();
            let label = name.to_string_lossy();
            let relative = format!("{relative}/{label}");
            if file_type.is_dir() && !label.starts_with('.') && !extra(&label) {
                directories.push((directory.join(name), relative));
            } else if file_type.is_file() {
                total_bytes += relative.len();
                paths.push(relative);
            }
            visited += 1;
            if paths.len() > MAX_FOLDER_FILES || total_bytes > MAX_PATH_BYTES || visited > 150_000 {
                return Err("文件夹太大，请选择更小的子目录。".into());
            }
        }
    }
    Ok(paths)
}

fn pattern(value: &str) -> Regex {
    Regex::new(value).expect("folder naming pattern")
}

static EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)(^|[ ._\[\](\)-])(?:s\d{1,3}[ ._-]*e\d{1,4}|\d{1,2}x\d{1,4}|第\s*[0-9一二三四五六七八九十百零两]+\s*(?:集|话|話))",
    )
});
static SEASON: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:season[ ._-]*\d{1,3}|s\d{1,3}|第\s*[0-9一二三四五六七八九十零两]+\s*季|specials?|特别篇|特別篇)$",
    )
});
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(^|[\s.(\[_-])((?:19[0-9]|20[0-3])[0-9])($|[\s.)\]_-])"));
static TRAILING_YEAR: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(.*?) \(((?:19[0-9]|20[0-3])[0-9])\)$"));
static ID: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)\b(tmdb|bangumi)(?:id)?[-_:= ]+([1-9][0-9]{0,9})\b"));
static NUMBERED_AV: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)^\s*([a-z]{2,8})[-_－]([0-9]{3,6})(?:$|[^a-z0-9])"));
static COMPACT_AV: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^([A-Z]{2,8})([0-9]{3,6})(?:$|[^A-Za-z0-9])"));
static LEADING_TAG: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*(?:\[([^\]]+)\]|【([^】]+)】)\s*"));
static RELEASE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:bd|dvd|web|chs|cht|sc|tc|jp|jpn|eng|multi|简体|繁体|简繁|简中|繁中|中文字幕|简繁内封)$",
    )
});
static RELEASING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)(?:^|[ ._\[\](\)【】-])(?:[248]k|(?:480|576|720|1080|1440|2160|4320)[pi]|web[ ._-]?(?:dl|rip)|blu[ ._-]?ray|b[dr]rip|hdrip|remux|hdtv|dvdrip|x26[45]|h[ .]?26[45]|hevc|av1|hdr10|hdr|dolby[ .]?vision|10bit|[57][.]1|aac|dts|atmos|proper|repack|sample|trailer|cd[0-9]|disc[0-9])(?:$|[^a-z0-9])",
    )
});

pub(super) fn classify_paths(
    paths: Vec<String>,
) -> Result<(Vec<BatchItemInput>, FolderBatchResult), String> {
    if paths.len() > MAX_FOLDER_FILES
        || paths.iter().map(String::len).sum::<usize>() > MAX_PATH_BYTES
    {
        return Err("文件夹太大。请选择包含 30,000 个以内文件的子目录。".into());
    }
    let mut items: Vec<BatchItemInput> = Vec::new();
    let mut groups = HashMap::new();
    let mut files = 0;
    let mut skipped = 0;
    let mut unrecognized = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut paths = paths;
    paths.sort_unstable();
    for value in paths {
        let path = value.replace('\\', "/");
        let parts = path.split('/').collect::<Vec<_>>();
        if path.len() > 4096
            || path.contains('\0')
            || parts.iter().any(|part| matches!(*part, "" | "." | ".."))
            || parts
                .first()
                .is_some_and(|part| part.as_bytes().get(1) == Some(&b':'))
        {
            return Err("只接受所选文件夹里的相对文件路径。".into());
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let (directories, filename) = parts.split_at(parts.len() - 1);
        let Some((stem, ext)) = filename[0].rsplit_once('.') else {
            skipped += 1;
            continue;
        };
        if !video_extension(ext)
            || parts.iter().any(|part| part.starts_with('.'))
            || extra(stem)
            || directories.iter().any(|part| extra(part))
        {
            skipped += 1;
            continue;
        }
        let Some((kind, query)) = identify(directories, stem) else {
            skipped += 1;
            if unrecognized.len() < 20 {
                unrecognized.push(path);
            }
            continue;
        };
        let key = (kind.code(), query.nfkc().collect::<String>().to_lowercase());
        let index = *groups.entry(key).or_insert_with(|| {
            let index = items.len();
            items.push(BatchItemInput {
                kind,
                query,
                source_files: Vec::new(),
            });
            index
        });
        items[index].source_files.push(path);
        files += 1;
    }
    if items.len() > MAX_WORKS {
        return Err("文件夹识别出超过 2,000 部作品。请选择更小的子目录。".into());
    }
    let info = FolderBatchResult {
        id: None,
        works: items.len(),
        files,
        skipped,
        unrecognized,
    };
    Ok((items, info))
}

fn video_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mkv"
            | "mp4"
            | "avi"
            | "mov"
            | "m4v"
            | "wmv"
            | "ts"
            | "m2ts"
            | "webm"
            | "mpg"
            | "mpeg"
            | "vob"
            | "flv"
            | "rmvb"
            | "iso"
            | "ogv"
            | "3gp"
    )
}

fn extra(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "sample"
            | "samples"
            | "trailer"
            | "trailers"
            | "backdrops"
            | "theme-music"
            | "behind the scenes"
            | "deleted scenes"
            | "interviews"
            | "extras"
            | "featurettes"
            | "预告片"
            | "花絮"
            | "@eadir"
    ) || [".sample", "-sample", ".trailer", "-trailer"]
        .iter()
        .any(|ending| value.to_ascii_lowercase().ends_with(ending))
}

fn identify(dirs: &[&str], stem: &str) -> Option<(MediaKind, String)> {
    let raw_stem = stem;
    let stem = release_title(stem);
    let episode = EPISODE.find(&stem);
    let season_dir = dirs.iter().rposition(|part| SEASON.is_match(part));
    let series = episode.is_some() || season_dir.is_some();
    let kind = if series {
        MediaKind::Series
    } else {
        MediaKind::Movie
    };
    let work_dir = season_dir
        .and_then(|position| position.checked_sub(1).map(|i| dirs[i]))
        .or_else(|| dirs.last().copied().filter(|dir| !SEASON.is_match(dir)));
    let title_stem = episode.map_or(stem.as_ref(), |marker| &stem[..marker.start()]);
    if let Some(query) = catalog_query(raw_stem).or_else(|| work_dir.and_then(catalog_query)) {
        return Some((kind, query));
    }
    // Season and episode markers take precedence over a release code inside the show name.
    if !series
        && let Some(code) =
            av_code(&stem).or_else(|| dirs.last().and_then(|dir| av_code(&release_title(dir))))
    {
        return Some((MediaKind::Av, code));
    }
    let name = clean_title(title_stem).filter(|(name, _)| !generic_file(name));
    let directory = work_dir
        .and_then(|dir| clean_title(&release_title(dir)))
        .filter(|(name, _)| !generic_file(name));
    let title = if season_dir.is_some() && directory.is_some() {
        directory?
    } else if let Some((name, year)) = name {
        let year = year.or_else(|| {
            directory
                .as_ref()
                .filter(|(title, _)| title.eq_ignore_ascii_case(&name))
                .and_then(|(_, year)| *year)
        });
        (name, year)
    } else {
        directory?
    };
    let query = title
        .1
        .map_or_else(|| title.0.clone(), |year| format!("{} ({year})", title.0));
    Some((kind, query))
}

fn generic_file(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace())
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "video"
                | "movie"
                | "film"
                | "films"
                | "media"
                | "movies"
                | "series"
                | "tv"
                | "tv shows"
                | "root"
                | "stream"
                | "video ts"
                | "bdmv"
                | "媒体目录"
                | "电影"
                | "动画电影"
                | "剧集"
                | "电视剧"
                | "动漫"
        )
}

pub(super) fn av_code(stem: &str) -> Option<String> {
    if let Some(code) = Identifier::special_filename(stem) {
        return Some(code);
    }
    if let Some(found) = NUMBERED_AV.captures(stem) {
        let digits = &found[2];
        if !year_digits(digits) {
            return Some(format!("{}-{digits}", found[1].to_ascii_uppercase()));
        }
    }
    let found = COMPACT_AV.captures(stem)?;
    let digits = &found[2];
    (!year_digits(digits)).then(|| format!("{}-{digits}", &found[1]))
}

fn year_digits(value: &str) -> bool {
    matches!(value.parse::<u16>(), Ok(1900..=2039)) && value.len() == 4
}

pub(super) fn catalog_query(stem: &str) -> Option<String> {
    let matches = ID.captures(stem)?;
    let id = matches.get(2)?.as_str();
    if matches[1].eq_ignore_ascii_case("bangumi") {
        crate::providers::media::bangumi::id(id).map(|id| format!("bangumi:{id}"))
    } else {
        id.parse::<i32>().ok().map(|id| format!("tmdb:{id}"))
    }
}

fn release_title(stem: &str) -> Cow<'_, str> {
    let mut tail = stem;
    let mut title = None;
    let mut year = None;
    let mut has_release_tags = false;
    while let Some(tag) = LEADING_TAG.captures(tail) {
        tail = &tail[tag.get(0).expect("matched tag").end()..];
        let value = tag
            .get(1)
            .or_else(|| tag.get(2))
            .expect("tag content")
            .as_str()
            .trim();
        if year_digits(value) {
            year = Some(value);
        } else if RELEASE_TAG.is_match(value)
            || RELEASING
                .find(value)
                .is_some_and(|marker| marker.start() == 0)
            || catalog_query(value).is_some()
        {
            has_release_tags |= title.is_some();
        } else if !has_release_tags && !generic_file(value) {
            // In an all-bracket release, the title usually follows the group name.
            title = Some(value);
            year = None;
        }
    }
    let Some(title) = title else {
        return Cow::Borrowed(tail);
    };
    let suffix = before_release_info(tail)
        .trim_matches([' ', '.', '_', '-', '[', ']', '【', '】', '(', ')']);
    let suffix_year = year_digits(suffix).then_some(suffix);
    if suffix.is_empty() || suffix_year.is_some() {
        return suffix_year.or(year).map_or(Cow::Borrowed(title), |year| {
            format!("{title} ({year})").into()
        });
    }
    Cow::Borrowed(tail)
}

fn before_release_info(stem: &str) -> &str {
    RELEASING
        .find(stem)
        .map_or(stem, |marker| &stem[..marker.start()])
}

fn clean_title(stem: &str) -> Option<(String, Option<u16>)> {
    let stem = before_release_info(stem);
    let candidate_year = YEAR
        .captures_iter(stem)
        .filter_map(|found| found.get(2))
        .filter(|year| {
            !stem[..year.start()]
                .trim_matches([' ', '.', '_', '-', '[', '('])
                .is_empty()
        })
        .filter(|year| {
            !stem[year.end()..]
                .trim_matches([' ', '.', '_', '-', ']', ')'])
                .chars()
                .any(char::is_alphanumeric)
        })
        .last();
    let (stem, year) = candidate_year.map_or((stem, None), |found| {
        (&stem[..found.start()], found.as_str().parse().ok())
    });
    let name = stem
        .trim_matches([' ', '.', '_', '-', '[', ']', '【', '】', '(', ')', '{', '}'])
        .replace(['.', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (name.chars().count() <= 160 && name.chars().any(char::is_alphanumeric)).then_some((name, year))
}

pub(super) fn split_search_year(query: &str) -> (&str, Option<u16>) {
    if let Some(found) = TRAILING_YEAR.captures(query) {
        let name = found.get(1).expect("title capture").as_str();
        (name, found[2].parse().ok())
    } else {
        (query, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_episodes_and_versions_while_preserving_release_identifiers() {
        let paths = [
            "Media/Dark (2017)/Season 01/S01E01.mkv",
            "Media/Dark (2017)/Season 02/Some Episode S02E01.MKV",
            "Media/films/Parasite.2019.2160p.BluRay.mkv",
            "Media/films/Parasite (2019) [1080p].mp4",
            "Media/films/Blade.Runner.2049.2017.1080p.mkv",
            "Media/films/[tmdb-550].mp4",
            "Media/films/Example [tmdbid=550].mp4",
            "Media/films/[bangumi-160209].mkv",
            "Media/星际牛仔 [bangumi-253]/Season 01/S01E02.mkv",
            "Media/av/[Release] ssis-001-C.mkv",
            "Media/av/SSIS001.mp4",
            "Media/av/FC2-PPV-1234567.mp4",
            "Media/movie.nfo",
            "Media/films/Parasite.sample.mkv",
            "Media/films/123.mp4",
        ]
        .map(str::to_owned)
        .to_vec();
        let (items, info) = classify_paths(paths).unwrap();
        let get = |query| items.iter().find(|item| item.query == query).unwrap();
        assert_eq!(info.works, 8);
        assert_eq!(info.files, 12);
        assert_eq!(info.skipped, 3);
        assert_eq!(get("Dark (2017)").kind, MediaKind::Series);
        assert_eq!(get("Dark (2017)").source_files.len(), 2);
        assert_eq!(get("Parasite (2019)").kind, MediaKind::Movie);
        assert_eq!(get("Parasite (2019)").source_files.len(), 2);
        assert_eq!(get("Blade Runner 2049 (2017)").kind, MediaKind::Movie);
        assert_eq!(get("tmdb:550").kind, MediaKind::Movie);
        assert_eq!(get("bangumi:160209").kind, MediaKind::Movie);
        assert_eq!(get("bangumi:253").kind, MediaKind::Series);
        assert_eq!(get("SSIS-001").source_files.len(), 2);
        assert_eq!(get("FC2-PPV-1234567").kind, MediaKind::Av);
        assert_eq!(info.unrecognized, ["Media/films/123.mp4"]);
    }

    #[test]
    fn recognizes_special_av_formats_in_files_and_directories() {
        for (dir, stem, expected) in [
            ("AV", "ABP-123.1080p", "ABP-123"),
            ("AV", "010124_123.1080p", "010124-123"),
            ("AV", "300mium001-1080p", "300MIUM-001"),
            ("AV", "[Release] ＦＣ２ ＰＰＶ 1234567", "FC2-PPV-1234567"),
            ("AV", "Brazzers.24.01.31.1080p", "BRAZZERS-24-01-31"),
            ("AV", "md001", "MD-001"),
            ("AV", "rj123456-1080p", "RJ-123456"),
            ("My Wife 0123", "video", "MYWIFE-0123"),
        ] {
            assert_eq!(
                identify(&[dir], stem),
                Some((MediaKind::Av, expected.into()))
            );
        }
        for stem in [
            "2024-01-31",
            "999999-123",
            "FC2PPV1234567extra",
            "Brazzers.24.99.99",
        ] {
            assert!(!matches!(
                identify(&["movies"], stem),
                Some((MediaKind::Av, _))
            ));
        }
        assert_eq!(
            identify(&["Show"], "RJ123456 S01E01"),
            Some((MediaKind::Series, "RJ123456".into()))
        );
        let preview = preview_folder(vec![
            "AV/FC2_1234567.mkv".into(),
            "AV/FC2-PPV-1234567.mp4".into(),
        ])
        .unwrap();
        assert_eq!(preview.works.len(), 1);
        assert_eq!(preview.works[0].file_count, 2);
    }

    #[test]
    fn rejects_paths_outside_selected_folder() {
        for path in [
            "/etc/file.mp4",
            "C:\\Users\\movie.mkv",
            "Media/../movie.mkv",
        ] {
            assert!(classify_paths(vec![path.into()]).is_err());
        }
    }

    #[test]
    fn keeps_the_title_in_bracketed_animated_movie_releases() {
        let paths = [
            "动画电影/[字幕组][你的名字。][2016][1080p][HEVC][简繁内封].mkv",
            "动画电影/【字幕组】你的名字。 (2016)【1080p】.mp4",
            "动画电影/[天气之子] (2019) [BDRip].mkv",
            "动画电影/[红辣椒].mkv",
            "动画电影/[字幕组] 2001 A Space Odyssey (1968) [1080p].mkv",
            "动画电影/[HEVC][1080p].mkv",
        ]
        .map(str::to_owned)
        .to_vec();
        let (items, info) = classify_paths(paths).unwrap();
        assert_eq!(info.unrecognized, ["动画电影/[HEVC][1080p].mkv"]);
        assert_eq!(info.works, 4);
        assert_eq!(info.files, 5);
        for (query, files) in [
            ("你的名字。 (2016)", 2),
            ("天气之子 (2019)", 1),
            ("红辣椒", 1),
            ("2001 A Space Odyssey (1968)", 1),
        ] {
            let item = items.iter().find(|item| item.query == query).unwrap();
            assert_eq!(item.kind, MediaKind::Movie);
            assert_eq!(item.source_files.len(), files);
        }
    }
}
