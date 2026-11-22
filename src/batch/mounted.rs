use std::{
    env,
    path::{Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use tokio::sync::Semaphore;

use super::folder::{FolderSelection, scan_open_folder};

// A disconnected network share can stall the OS call. Keep its blocking threads bounded,
// even if the browser closes the request before the filesystem returns.
static MOUNT_IO: Semaphore = Semaphore::const_new(2);
const READ_ERROR: &str = "无法读取服务器挂载目录，请检查挂载状态和读取权限。";

fn mount_root() -> Result<PathBuf, String> {
    env::var_os("SCRAPE_MEDIA_ROOT")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or("未配置服务器挂载目录。请先挂载 SMB/NFS 共享并设置 SCRAPE_MEDIA_ROOT。".into())
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['\\', '/', ':', '\0'])
}

fn open(root: &Path, relative: &str) -> Result<Dir, String> {
    if relative.len() > 4096 || (!relative.is_empty() && !relative.split('/').all(component)) {
        return Err("请选择共享目录内的相对路径。".into());
    }
    let root = Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| READ_ERROR)?;
    if relative.is_empty() {
        Ok(root)
    } else {
        root.open_dir(relative).map_err(|_| READ_ERROR.into())
    }
}

fn browse(root: &Path, relative: &str) -> Result<Vec<String>, String> {
    let folder = open(root, relative)?;
    let mut names = Vec::new();
    for (index, entry) in folder.entries().map_err(|_| READ_ERROR)?.enumerate() {
        let entry = entry.map_err(|_| READ_ERROR)?;
        if entry.file_type().map_err(|_| READ_ERROR)?.is_dir()
            && let Ok(name) = entry.file_name().into_string()
            && component(&name)
        {
            names.push(name);
        }
        if names.len() > 2000 || index >= super::MAX_FOLDER_FILES {
            return Err("此目录的条目太多，无法在页面中列出。请缩小共享范围。".into());
        }
    }
    names.sort_unstable();
    Ok(names)
}

fn scan(root: &Path, relative: &str) -> Result<FolderSelection, String> {
    let folder = open(root, relative)?;
    let (display, name) = if relative.is_empty() {
        ("服务器共享目录".into(), "共享媒体")
    } else {
        (
            format!("服务器共享目录 / {relative}"),
            relative.rsplit('/').next().unwrap(),
        )
    };
    FolderSelection::new(display, scan_open_folder(&folder, name)?, false)
}

async fn on_mount<T: Send + 'static>(
    relative: String,
    operation: fn(&Path, &str) -> Result<T, String>,
) -> Result<T, String> {
    let root = mount_root()?;
    let permit = MOUNT_IO
        .try_acquire()
        .map_err(|_| "服务器正在读取其他共享目录，请稍后重试。")?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation(&root, &relative)
    })
    .await
    .map_err(|_| READ_ERROR)?
}

pub async fn browse_mounted_folders(relative: String) -> Result<Vec<String>, String> {
    on_mount(relative, browse).await
}

pub async fn scan_mounted_folder(relative: String) -> Result<FolderSelection, String> {
    on_mount(relative, scan).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_and_previews_only_the_mounted_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("mount");
        let movies = root.join("电影");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&movies).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(movies.join("Example (2019).mkv"), []).unwrap();
        fs::write(outside.join("Private.mkv"), []).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        assert_eq!(browse(&root, "").unwrap(), ["电影"]);
        let selection = scan(&root, "电影").unwrap();
        assert_eq!(selection.preview.works[0].query, "Example (2019)");
        assert_eq!(selection.paths, ["电影/Example (2019).mkv"]);
        assert_eq!(scan(&root, "").unwrap().preview.works.len(), 1);
        for path in [
            "../outside",
            "/outside",
            "电影/../..",
            r"电影\..",
            "C:/外部",
        ] {
            assert!(browse(&root, path).is_err(), "{path}");
            assert!(scan(&root, path).is_err(), "{path}");
        }
        #[cfg(unix)]
        assert!(scan(&root, "escape").is_err());
    }
}
