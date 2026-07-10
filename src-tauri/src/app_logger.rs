use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024; // 5 MB
const LOG_FILE_NAME: &str = "app.log";
const LOG_BACKUP_NAME: &str = "app.log.1";

static LOGGER: OnceLock<AppLogger> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

pub struct AppLogger {
    log_dir: PathBuf,
    log_path: PathBuf,
    file: Mutex<File>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogInfo {
    pub log_dir: String,
    pub log_path: String,
    pub size_bytes: u64,
    pub has_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsResult {
    pub target_path: String,
    pub bytes_written: u64,
    pub source_files: usize,
}

/// Khởi tạo logger vào `{app_data_dir}/logs/app.log`.
pub fn init(app_data_dir: &Path) -> Result<(), String> {
    let log_dir = app_data_dir.join("logs");
    fs::create_dir_all(&log_dir)
        .map_err(|e| format!("Không tạo được thư mục logs: {e}"))?;

    let log_path = log_dir.join(LOG_FILE_NAME);
    let file = open_log_file(&log_path)?;

    let logger = AppLogger {
        log_dir,
        log_path,
        file: Mutex::new(file),
    };

    LOGGER
        .set(logger)
        .map_err(|_| "App logger đã được khởi tạo.".to_string())?;

    info("app", "===== HIS XML Sync started =====");
    info(
        "app",
        &format!("Log directory: {}", log_dir_path().unwrap_or_default()),
    );

    Ok(())
}

pub fn info(module: &str, message: &str) {
    write(LogLevel::Info, module, message);
}

#[allow(dead_code)]
pub fn warn(module: &str, message: &str) {
    write(LogLevel::Warn, module, message);
}

pub fn error(module: &str, message: &str) {
    write(LogLevel::Error, module, message);
}

pub fn debug(module: &str, message: &str) {
    write(LogLevel::Debug, module, message);
}

pub fn write(level: LogLevel, module: &str, message: &str) {
    let line = format!(
        "{} [{}] [{}] {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        level.as_str(),
        sanitize_module(module),
        sanitize_message(message)
    );

    // Always mirror to stderr for dev visibility.
    eprint!("{line}");

    let Some(logger) = LOGGER.get() else {
        return;
    };

    if let Err(err) = logger.append(&line) {
        eprintln!("[logger] failed to write log: {err}");
    }
}

pub fn get_info() -> Result<LogInfo, String> {
    let logger = require_logger()?;
    let size_bytes = fs::metadata(&logger.log_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let has_backup = logger.log_dir.join(LOG_BACKUP_NAME).is_file();

    Ok(LogInfo {
        log_dir: logger.log_dir.display().to_string(),
        log_path: logger.log_path.display().to_string(),
        size_bytes,
        has_backup,
    })
}

/// Sao chép app.log (+ backup nếu có) vào `target_path`.
/// Nếu có backup, gộp nội dung theo thứ tự: backup trước, log hiện tại sau.
pub fn export_to(target_path: &str) -> Result<ExportLogsResult, String> {
    let logger = require_logger()?;
    // Flush via re-open handle by reading from disk files.
    let target = PathBuf::from(target_path.trim());
    if target_path.trim().is_empty() {
        return Err("Đường dẫn xuất log không hợp lệ.".into());
    }

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Không tạo được thư mục đích: {e}"))?;
        }
    }

    let mut content = String::new();
    let mut source_files = 0usize;

    let backup = logger.log_dir.join(LOG_BACKUP_NAME);
    if backup.is_file() {
        content.push_str(&format!(
            "===== BEGIN {} =====\n",
            backup.display()
        ));
        content.push_str(&read_file_to_string(&backup)?);
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("===== END {} =====\n\n", backup.display()));
        source_files += 1;
    }

    content.push_str(&format!(
        "===== BEGIN {} =====\n",
        logger.log_path.display()
    ));
    content.push_str(&read_file_to_string(&logger.log_path)?);
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("===== END {} =====\n", logger.log_path.display()));
    source_files += 1;

    let header = format!(
        "# HIS XML Sync log export\n# Exported at: {}\n# Sources: {source_files}\n\n",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );

    let full = format!("{header}{content}");
    fs::write(&target, full.as_bytes())
        .map_err(|e| format!("Không ghi được file log: {e}"))?;

    info(
        "app",
        &format!(
            "Exported logs to {} ({} bytes, {source_files} source file(s))",
            target.display(),
            full.len()
        ),
    );

    Ok(ExportLogsResult {
        target_path: target.display().to_string(),
        bytes_written: full.len() as u64,
        source_files,
    })
}

/// Ghi log từ frontend (UI / client-side errors).
pub fn log_from_frontend(level: &str, module: &str, message: &str) {
    let lvl = match level.to_ascii_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "warn" | "warning" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    let module = if module.trim().is_empty() {
        "frontend"
    } else {
        module
    };
    write(lvl, module, message);
}

fn require_logger() -> Result<&'static AppLogger, String> {
    LOGGER
        .get()
        .ok_or_else(|| "Logger chưa được khởi tạo.".to_string())
}

fn log_dir_path() -> Option<String> {
    LOGGER.get().map(|l| l.log_dir.display().to_string())
}

impl AppLogger {
    fn append(&self, line: &str) -> Result<(), String> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| "Không khóa được file log.".to_string())?;

        // Rotate if current file grew too large.
        let size = file
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0);
        if size >= MAX_LOG_BYTES {
            drop(file);
            self.rotate()?;
            file = self
                .file
                .lock()
                .map_err(|_| "Không khóa được file log sau rotate.".to_string())?;
        }

        file.write_all(line.as_bytes())
            .map_err(|e| format!("Ghi log thất bại: {e}"))?;
        file.flush().map_err(|e| format!("Flush log thất bại: {e}"))?;
        Ok(())
    }

    fn rotate(&self) -> Result<(), String> {
        let backup = self.log_dir.join(LOG_BACKUP_NAME);
        let _ = fs::remove_file(&backup);
        fs::rename(&self.log_path, &backup)
            .map_err(|e| format!("Rotate log thất bại: {e}"))?;

        let new_file = open_log_file(&self.log_path)?;
        let mut guard = self
            .file
            .lock()
            .map_err(|_| "Không khóa được file log khi rotate.".to_string())?;
        *guard = new_file;

        // Write rotation notice without recursive rotate risk (new empty file).
        let notice = format!(
            "{} [INFO] [app] Log rotated (previous -> {})\n",
            Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            backup.display()
        );
        guard
            .write_all(notice.as_bytes())
            .map_err(|e| format!("Ghi notice rotate thất bại: {e}"))?;
        guard.flush().ok();
        Ok(())
    }
}

fn open_log_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .map_err(|e| format!("Không mở được file log {}: {e}", path.display()))
}

fn read_file_to_string(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|e| format!("Không đọc được {}: {e}", path.display()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|e| format!("Không đọc nội dung {}: {e}", path.display()))?;
    Ok(buf)
}

fn sanitize_module(module: &str) -> String {
    module
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(64)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Loại bỏ ký tự control; không cố parse password nhưng chặn pattern password=...
fn sanitize_message(message: &str) -> String {
    let mut out = message
        .chars()
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
        .collect::<String>();

    // Redact common secret patterns.
    out = redact_key(&out, "password");
    out = redact_key(&out, "Password");
    out = redact_key(&out, "token");
    out = redact_key(&out, "Token");
    out
}

fn redact_key(input: &str, key: &str) -> String {
    // password=secret / "password":"secret"
    let patterns = [
        format!("{key}="),
        format!("{key}:"),
        format!("\"{key}\":"),
        format!("'{key}':"),
    ];

    let mut result = input.to_string();
    for pattern in patterns {
        if let Some(idx) = result.find(&pattern) {
            let start = idx + pattern.len();
            let rest = &result[start..];
            let end_rel = rest
                .find(|c: char| c == ' ' || c == ',' || c == ';' || c == '"' || c == '\'')
                .unwrap_or(rest.len());
            let end = start + end_rel;
            result.replace_range(start..end, "***");
        }
    }
    result
}
