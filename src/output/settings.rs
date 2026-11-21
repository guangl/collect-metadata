use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortraitRole {
    Actor,
    Voice,
    Director,
    Writer,
    Crew,
}

impl PortraitRole {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Actor => "actor",
            Self::Voice => "voice",
            Self::Director => "director",
            Self::Writer => "writer",
            Self::Crew => "crew",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OutputOptions {
    enabled: bool,
    directory: String,
    pub(super) directory_template: String,
    pub(super) filename_prefix: String,
    pub(super) metadata: bool,
    pub(super) synopsis: bool,
    pub(super) credits: bool,
    pub(super) ratings: bool,
    pub(super) episodes: bool,
    pub(super) raw: bool,
    pub(super) emby_nfo: bool,
    pub(super) poster: bool,
    pub(super) backdrop: bool,
    pub(super) stills: bool,
    pub(super) trailers: bool,
    pub(super) theme_videos: bool,
    pub(super) theme_music: bool,
    pub(super) extras: bool,
    pub(super) portraits: Vec<PortraitRole>,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: String::new(),
            directory_template: super::naming::DEFAULT_WORK_NAME.to_owned(),
            filename_prefix: String::new(),
            metadata: true,
            synopsis: true,
            credits: true,
            ratings: true,
            episodes: true,
            raw: false,
            emby_nfo: false,
            poster: true,
            backdrop: false,
            stills: false,
            trailers: false,
            theme_videos: false,
            theme_music: false,
            extras: false,
            portraits: Vec::new(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ExportPlan {
    pub(crate) owner: String,
    pub(super) directory: PathBuf,
    pub(super) options: OutputOptions,
}

#[derive(Deserialize, Serialize)]
struct SavedSettings {
    owner: String,
    options: OutputOptions,
}

#[derive(Serialize)]
pub struct OutputSettingsView {
    options: OutputOptions,
    root: Option<String>,
}

pub struct OutputSettings {
    file: PathBuf,
    root: Option<PathBuf>,
    state: Mutex<SavedSettings>,
}

impl OutputSettings {
    pub fn load(file: PathBuf, root: Option<PathBuf>) -> Result<Self, String> {
        let state = match fs::read_to_string(&file) {
            Ok(value) => serde_json::from_str(&value).map_err(|_| "输出设置格式无效。")?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => SavedSettings {
                owner: uuid::Uuid::new_v4().to_string(),
                options: OutputOptions::default(),
            },
            Err(_) => return Err("无法读取输出设置。".into()),
        };
        Ok(Self {
            file,
            root,
            state: Mutex::new(state),
        })
    }

    pub fn view(&self) -> OutputSettingsView {
        let state = self.state.lock().expect("output settings lock");
        self.view_for(&state)
    }

    fn view_for(&self, state: &SavedSettings) -> OutputSettingsView {
        OutputSettingsView {
            options: state.options.clone(),
            root: self
                .root
                .as_ref()
                .map(|root| root.to_string_lossy().into_owned()),
        }
    }

    pub fn save(&self, mut options: OutputOptions) -> Result<OutputSettingsView, String> {
        options.directory = options.directory.trim().to_owned();
        options.directory_template = options.directory_template.trim().to_owned();
        options.filename_prefix = options.filename_prefix.trim().to_owned();
        if !super::naming::validate(&options.directory_template)
            || !super::naming::validate(&options.filename_prefix)
        {
            return Err("命名中只能使用普通字符及受支持的占位符，长度最多 160 字节。".into());
        }
        if options.directory.len() > 4096 || options.directory.contains('\0') {
            return Err("输出目录无效。".into());
        }
        let mut roles = Vec::new();
        for role in options.portraits {
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
        options.portraits = roles;
        if options.enabled {
            if !(options.metadata
                || options.raw
                || options.emby_nfo
                || options.poster
                || options.backdrop
                || options.stills
                || options.trailers
                || options.theme_videos
                || options.theme_music
                || options.extras
                || !options.portraits.is_empty())
            {
                return Err("请选择至少一项要写出的资料。".into());
            }
            let directory = self.resolve(&options.directory)?;
            tempfile::NamedTempFile::new_in(&directory).map_err(|_| "输出目录不可写。")?;
        }
        let mut state = self.state.lock().expect("output settings lock");
        let updated = SavedSettings {
            owner: state.owner.clone(),
            options,
        };
        write_settings(&self.file, &updated).map_err(|_| "无法保存输出设置。")?;
        *state = updated;
        Ok(self.view_for(&state))
    }

    pub(crate) fn owner(&self) -> String {
        self.state
            .lock()
            .expect("output settings lock")
            .owner
            .clone()
    }

    pub(crate) fn durable_owner(&self) -> Result<String, String> {
        let state = self.state.lock().expect("output settings lock");
        if !self.file.is_file() {
            write_settings(&self.file, &state).map_err(|_| "无法保存本机后台进程的配置。")?;
        }
        Ok(state.owner.clone())
    }

    pub(crate) fn plan(&self) -> Result<Option<ExportPlan>, String> {
        let state = self.state.lock().expect("output settings lock");
        if !state.options.enabled {
            return Ok(None);
        }
        Ok(Some(ExportPlan {
            owner: state.owner.clone(),
            directory: self.resolve(&state.options.directory)?,
            options: state.options.clone(),
        }))
    }

    fn resolve(&self, value: &str) -> Result<PathBuf, String> {
        let path = Path::new(value);
        if let Some(root) = &self.root {
            if value.contains(['\\', ':'])
                || value.split('/').any(|part| part == "." || part == "..")
                || path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err("请输入输出挂载目录下的相对子目录。".into());
            }
            fs::create_dir_all(root).map_err(|_| "无法创建 Web 输出目录。")?;
            let mut directory = root.canonicalize().map_err(|_| "无法读取 Web 输出目录。")?;
            for part in path.components() {
                directory.push(part);
                make_child_dir(&directory).map_err(|_| "输出子目录不可用。")?;
            }
            Ok(directory)
        } else {
            if !path.is_absolute() {
                return Err("请选择一个输出文件夹。".into());
            }
            let directory = path.canonicalize().map_err(|_| "无法读取所选输出目录。")?;
            if !directory.is_dir() {
                return Err("所选输出位置不是目录。".into());
            }
            Ok(directory)
        }
    }
}

pub(super) fn make_child_dir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(path)?.file_type().is_dir() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn write_settings(path: &Path, config: &SavedSettings) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, config)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, to_value};

    use super::*;

    #[test]
    fn loads_legacy_naming_defaults_and_rejects_invalid_updates() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.json");
        let root = temp.path().join("export");
        fs::write(
            &config,
            r#"{"owner":"legacy","options":{"enabled":true,"directory":"old"}}"#,
        )
        .unwrap();
        let settings = OutputSettings::load(config.clone(), Some(root.clone())).unwrap();
        let initial = settings.plan().unwrap().unwrap();
        assert_eq!(initial.owner, "legacy");
        assert_eq!(initial.options.directory_template, "work-{row}");
        assert!(initial.options.filename_prefix.is_empty());
        assert!(initial.directory.ends_with("export/old"));

        let options = |value| serde_json::from_value(value).unwrap();
        for bad in [
            json!({"directory_template":"../outside"}),
            json!({"enabled":true,"directory":"C:\\Windows"}),
            json!({"enabled":true,"directory":"test", "metadata":false, "poster":false}),
        ] {
            assert!(settings.save(options(bad)).is_err());
        }
        assert_eq!(settings.plan().unwrap().unwrap().options.directory, "old");
        let saved = settings
            .save(options(json!({"enabled":true,"directory":" new/film ",
            "directory_template":" {title} ","portraits":["writer","writer","crew"]})))
            .unwrap();
        assert_eq!(
            to_value(saved).unwrap()["options"]["portraits"],
            json!(["writer", "crew"])
        );
        let reloaded = OutputSettings::load(config, Some(root)).unwrap();
        let new_plan = reloaded.plan().unwrap().unwrap();
        assert_eq!(new_plan.owner, initial.owner);
        assert_eq!(new_plan.options.directory_template, "{title}");
        assert!(new_plan.directory.ends_with("new/film"));
    }
}
