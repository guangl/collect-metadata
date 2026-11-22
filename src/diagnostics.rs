//! Operational events. Keep user input, URLs, paths and raw dependency errors out of these logs.
use std::{
    io::{self, Write},
    time::Instant,
};

use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[cfg(feature = "desktop")]
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

tokio::task_local! {
    static REQUEST_ID: String;
    static BATCH_ROW: (i64, i64, i64);
}

pub fn request_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub async fn with_request<T>(id: String, work: impl Future<Output = T>) -> T {
    REQUEST_ID.scope(id, work).await
}

pub(crate) async fn with_batch_row<T>(
    batch: i64,
    row: i64,
    attempt: i64,
    work: impl Future<Output = T>,
) -> T {
    BATCH_ROW.scope((batch, row, attempt), work).await
}

pub(crate) fn with_reference(message: String) -> String {
    REQUEST_ID
        .try_with(|id| format!("{message}（诊断编号 {id}）"))
        .unwrap_or(message)
}

pub fn info(event: &'static str, fields: Value) {
    emit("INFO", event, fields);
}

pub fn warn(event: &'static str, fields: Value) {
    emit("WARN", event, fields);
}

pub fn error(event: &'static str, fields: Value) {
    emit("ERROR", event, fields);
}

fn emit(level: &'static str, event: &'static str, fields: Value) {
    let mut record = Map::new();
    if let Ok(timestamp) = OffsetDateTime::now_utc().format(&Rfc3339) {
        record.insert("timestamp".into(), json!(timestamp));
    }
    record.insert("level".into(), json!(level));
    record.insert("event".into(), json!(event));
    if let Ok(id) = REQUEST_ID.try_with(|id| json!(id)) {
        record.insert("request_id".into(), id);
    }
    if let Ok((batch, row, attempt)) = BATCH_ROW.try_with(|context| *context) {
        record.insert("batch_id".into(), json!(batch));
        record.insert("row_id".into(), json!(row));
        record.insert("attempt".into(), json!(attempt));
    }
    record.insert("details".into(), fields);
    let mut line = Value::Object(record).to_string();
    line.push('\n');

    #[cfg(feature = "desktop")]
    if let Some((directory, lock)) = DESKTOP_LOG.get() {
        if lock
            .lock()
            .is_ok_and(|_guard| write_desktop(directory, line.as_bytes()).is_ok())
        {
            return;
        }
        if !FILE_FAILURE.swap(true, Ordering::Relaxed) {
            let _ = io::stderr()
                .write_all(b"{\"level\":\"ERROR\",\"event\":\"desktop_log_write_failed\"}\n");
        }
    }
    let _ = io::stderr().write_all(line.as_bytes());
}

pub fn startup_step<T, E>(step: &'static str, result: Result<T, E>) -> Result<T, E> {
    result.inspect_err(|_| error("startup_step_failed", json!({"step": step})))
}

pub fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn http_reason(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else {
        "transport"
    }
}

pub(crate) fn postgres_error(event: &'static str, error: &sqlx::Error) {
    let reason = match error {
        sqlx::Error::Database(_) => "server",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::RowNotFound => "row_not_found",
        _ => "driver",
    };
    let sqlstate = error
        .as_database_error()
        .and_then(|error| error.code())
        .filter(|code| {
            code.len() == 5
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        });
    warn(event, json!({ "reason": reason, "sqlstate": sqlstate }));
}

#[cfg(feature = "desktop")]
static DESKTOP_LOG: OnceLock<(PathBuf, Mutex<()>)> = OnceLock::new();
#[cfg(feature = "desktop")]
static FILE_FAILURE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "desktop")]
const LOG_SIZE: u64 = 2 * 1024 * 1024;

#[cfg(feature = "desktop")]
pub fn init_desktop(directory: PathBuf) {
    let _ = DESKTOP_LOG.set((directory, Mutex::new(())));
}

#[cfg(feature = "desktop")]
pub fn desktop_log_directory() -> Result<&'static Path, String> {
    let (directory, lock) = DESKTOP_LOG.get().ok_or("无法确定日志目录。")?;
    let _guard = lock.lock().map_err(|_| "无法检查日志目录。")?;
    write_desktop(directory, b"")
        .map_err(|_| "无法写入日志目录。请检查当前用户的写入权限和磁盘空间。")?;
    Ok(directory)
}

#[cfg(feature = "desktop")]
fn write_desktop(directory: &Path, line: &[u8]) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    // The file lock also serializes rotation when two app processes are open.
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(directory.join(".log.lock"))?;
    lock.lock()?;
    let active = directory.join("collect-metadata.log");
    if fs::metadata(&active).is_ok_and(|metadata| metadata.len() + line.len() as u64 > LOG_SIZE) {
        let older = directory.join("collect-metadata.2.log");
        let previous = directory.join("collect-metadata.1.log");
        remove_if_present(&older)?;
        if previous.try_exists()? {
            fs::rename(previous, older)?;
        }
        fs::rename(&active, directory.join("collect-metadata.1.log"))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(active)?
        .write_all(line)
}

#[cfg(feature = "desktop")]
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(feature = "desktop")]
pub async fn desktop_command<T>(
    command: &'static str,
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let id = request_id();
    with_request(id, async {
        let start = Instant::now();
        work.await.map_err(|message| {
            warn(
                "desktop_command_failed",
                json!({ "command": command, "elapsed_ms": elapsed_ms(start) }),
            );
            with_reference(message)
        })
    })
    .await
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;

    #[test]
    fn desktop_log_rotates_and_keeps_two_previous_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("collect-metadata.2.log"), b"oldest\n").unwrap();
        fs::write(dir.path().join("collect-metadata.1.log"), b"previous\n").unwrap();
        fs::File::create(dir.path().join("collect-metadata.log"))
            .unwrap()
            .set_len(LOG_SIZE)
            .unwrap();
        write_desktop(dir.path(), b"latest\n").unwrap();
        assert_eq!(
            fs::read(dir.path().join("collect-metadata.log")).unwrap(),
            b"latest\n"
        );
        assert_eq!(
            fs::metadata(dir.path().join("collect-metadata.1.log"))
                .unwrap()
                .len(),
            LOG_SIZE
        );
        assert_eq!(
            fs::read(dir.path().join("collect-metadata.2.log")).unwrap(),
            b"previous\n"
        );
    }
}
