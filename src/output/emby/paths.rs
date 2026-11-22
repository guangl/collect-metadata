use std::{
    collections::HashSet,
    ffi::OsString,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub(in crate::output) struct PathMapping {
    pub(super) server: String,
    pub(super) local: String,
}

impl PathMapping {
    pub(in crate::output) fn validate(
        paths: &mut [Self],
        web: bool,
        root: Option<&Path>,
    ) -> Result<(), String> {
        if paths.len() > 32 {
            return Err("最多可配置 32 条 Emby 媒体路径映射。".into());
        }
        if web && root.is_none() && !paths.is_empty() {
            return Err("请先为 Web 服务配置可写的 EMBY_WRITEBACK_ROOT。".into());
        }
        let mut servers = HashSet::new();
        for path in paths {
            path.server = path.server.trim().replace('\\', "/");
            path.local = path.local.trim().to_owned();
            let Some(server) = ServerPath::parse(&path.server) else {
                return Err("Emby 端路径必须是绝对路径，例如 /media/Movies。".into());
            };
            let local = Path::new(&path.local);
            if path.local.len() > 4096
                || path.local.contains('\0')
                || local.components().any(|part| part == Component::ParentDir)
                || (web
                    && local
                        .components()
                        .any(|part| !matches!(part, Component::Normal(_))))
                || (!web && !local.is_absolute())
            {
                return Err(if web {
                    "请输入可写挂载根目录下的相对路径。"
                } else {
                    "请输入本机可访问的媒体目录绝对路径。"
                }
                .into());
            }
            let mut key = format!("{}:{}", server.root, server.parts.join("/"));
            if server.windows() {
                key.make_ascii_lowercase();
            }
            if !servers.insert(key) {
                return Err("每个 Emby 端目录只能配置一条路径映射。".into());
            }
        }
        Ok(())
    }
}

struct ServerPath<'a> {
    root: &'a str,
    parts: Vec<&'a str>,
}

impl<'a> ServerPath<'a> {
    fn parse(path: &'a str) -> Option<Self> {
        if path.len() > 4096 || path.contains('\0') {
            return None;
        }
        let (root, remainder) = if let Some(remainder) = path.strip_prefix("//") {
            ("//", remainder)
        } else if let Some(remainder) = path.strip_prefix('/') {
            ("/", remainder)
        } else if matches!(path.as_bytes(), [b'A'..=b'Z' | b'a'..=b'z', b':', b'/', ..]) {
            (&path[..2], &path[3..])
        } else {
            return None;
        };
        let parts: Vec<_> = remainder
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.iter().any(|part| matches!(*part, "." | "..")) || (root == "//" && parts.len() < 2)
        {
            return None;
        }
        Some(Self { root, parts })
    }

    fn windows(&self) -> bool {
        self.root != "/"
    }

    fn relative_to(&self, base: &Self) -> Option<&[&str]> {
        let equal = |left: &str, right: &str| {
            if self.windows() {
                left.eq_ignore_ascii_case(right)
            } else {
                left == right
            }
        };
        (equal(self.root, base.root)
            && self.parts.len() >= base.parts.len()
            && self
                .parts
                .iter()
                .zip(&base.parts)
                .all(|(left, right)| equal(left, right)))
        .then(|| &self.parts[base.parts.len()..])
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct MediaPaths {
    paths: Vec<PathMapping>,
    web_root: Option<PathBuf>,
}

impl MediaPaths {
    pub(in crate::output) fn new(
        paths: Vec<PathMapping>,
        web: bool,
        web_root: Option<PathBuf>,
    ) -> Self {
        Self {
            paths: if web && web_root.is_none() {
                Vec::new()
            } else {
                paths
            },
            web_root,
        }
    }

    pub(crate) fn work_source_urls(&self, path: &str) -> Vec<String> {
        const MAX_NFO_BYTES: u64 = 512 * 1024;
        let Some(target) = self.target(path, false) else {
            return Vec::new();
        };
        let Ok(dir) = target.directory() else {
            return Vec::new();
        };
        let names = [target.video_sidecar(".nfo"), "movie.nfo".into()];
        for name in names {
            if !dir
                .symlink_metadata(&name)
                .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_NFO_BYTES)
            {
                continue;
            }
            let mut xml = String::new();
            if dir
                .open(&name)
                .and_then(|file| file.take(MAX_NFO_BYTES + 1).read_to_string(&mut xml))
                .is_ok()
                && xml.len() as u64 <= MAX_NFO_BYTES
            {
                let urls = private_source_urls(&xml);
                if !urls.is_empty() {
                    return urls;
                }
            }
        }
        Vec::new()
    }

    pub(super) fn target(&self, path: &str, series: bool) -> Option<Target> {
        let normalized = path.replace('\\', "/");
        let server = ServerPath::parse(&normalized)?;
        let (mapping, tail) = self
            .paths
            .iter()
            .filter_map(|mapping| {
                let base = ServerPath::parse(&mapping.server)?;
                Some((mapping, server.relative_to(&base)?))
            })
            .min_by_key(|(_, tail)| tail.len())?;
        let mut relative = PathBuf::new();
        let root = match &self.web_root {
            Some(root) => {
                let local = Path::new(&mapping.local);
                if local
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return None;
                }
                relative.push(local);
                root.clone()
            }
            None => {
                let local = PathBuf::from(&mapping.local);
                if !local.is_absolute() {
                    return None;
                }
                local
            }
        };
        for part in tail {
            let mut components = Path::new(part).components();
            if !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
            {
                return None;
            }
            relative.push(part);
        }
        let video = if series {
            None
        } else {
            if tail.is_empty() {
                return None;
            }
            let name = relative.file_name()?;
            Path::new(name).file_stem()?;
            Some(name.to_owned())
        };
        if video.is_some() {
            relative.pop();
        }
        Some(Target {
            root,
            relative,
            video,
        })
    }
}

fn private_source_urls(xml: &str) -> Vec<String> {
    use quick_xml::{Reader, escape::unescape, events::Event};

    let mut reader = Reader::from_str(xml);
    let mut depth = 0;
    let mut urls = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) if depth == 0 && tag.name().as_ref() != "movie" => break,
            Ok(Event::Start(tag))
                if depth == 1 && matches!(tag.name().as_ref(), "javbusid" | "javdbid") =>
            {
                if let Ok(value) = reader.read_text(tag.name())
                    && let Ok(value) = unescape(value.as_ref().trim())
                {
                    urls.push(value.into_owned());
                }
            }
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) if depth <= 1 => break,
            Ok(Event::End(_)) => depth -= 1,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    urls
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Target {
    root: PathBuf,
    relative: PathBuf,
    video: Option<OsString>,
}

impl Target {
    pub(super) fn directory(&self) -> io::Result<Dir> {
        let root = Dir::open_ambient_dir(&self.root, ambient_authority())?;
        let dir = if self.relative.as_os_str().is_empty() {
            root
        } else {
            root.open_dir(&self.relative)?
        };
        if let Some(video) = &self.video
            && !dir.symlink_metadata(video)?.is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular video file",
            ));
        }
        Ok(dir)
    }

    pub(super) fn display_path(&self) -> String {
        self.root
            .join(&self.relative)
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn stem(&self) -> Option<&str> {
        Path::new(self.video.as_ref()?).file_stem()?.to_str()
    }

    pub(super) fn video_sidecar(&self, suffix: &str) -> String {
        format!(
            "{}{suffix}",
            self.stem().expect("mapped Emby video has a file stem")
        )
    }

    pub(super) fn backdrop_stem(&self) -> String {
        let Some(stem) = self.stem() else {
            return "fanart".into();
        };
        // A folder named for the video (including Emby's version suffix) can use shared fanart.
        if self
            .relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|folder| {
                stem == folder
                    || stem
                        .strip_prefix(folder)
                        .is_some_and(|suffix| suffix.starts_with(" - "))
            })
        {
            "fanart".into()
        } else {
            self.video_sidecar("-thumb")
        }
    }
}

impl Target {
    pub(super) fn is_video(&self) -> bool {
        self.video.is_some()
    }

    pub(super) fn is_video_name(&self, name: &str) -> bool {
        self.video
            .as_ref()
            .is_some_and(|video| video.eq_ignore_ascii_case(name))
    }

    pub(super) fn path(&self, name: &str) -> String {
        self.root
            .join(&self.relative)
            .join(name)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_the_most_specific_server_path_by_component() {
        let root = tempfile::tempdir().unwrap();
        let mut mappings = vec![
            PathMapping {
                server: r"Z:\Media".into(),
                local: "general".into(),
            },
            PathMapping {
                server: r"Z:\Media\TV".into(),
                local: "series".into(),
            },
        ];
        PathMapping::validate(&mut mappings, true, Some(root.path())).unwrap();
        let paths = MediaPaths::new(mappings, true, Some(root.path().into()));

        let target = paths
            .target(r"z:\media\tv\剧集\Season 1\第 1 集.mp4", false)
            .unwrap();
        assert_eq!(target.relative, Path::new("series/剧集/Season 1"));
        assert_eq!(target.video_sidecar(".nfo"), "第 1 集.nfo");
        assert_eq!(
            paths
                .target(r"Z:\Media\TV Archive\Film.mkv", false)
                .unwrap()
                .relative,
            Path::new("general/TV Archive")
        );
        for path in [
            r"Z:\MediaArchive\Film.mkv",
            r"Z:\Media\..\Film.mp4",
            "https://media/show",
        ] {
            assert!(paths.target(path, false).is_none());
        }
    }
}
