use crate::db::AppDb;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::SystemTime;

/// Trạng thái xử lý file XML trong SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum XmlFileStatus {
    Waiting,
    Processing,
    Parsed,
    PatientMatched,
    Mapped,
    Sending,
    Processed,
    PatientNotFound,
    TreatmentAmbiguous,
    XmlError,
    MappingError,
    SendError,
    Failed,
}

impl XmlFileStatus {
    pub fn parse(value: &str) -> Self {
        match value {
            "processing" => Self::Processing,
            "parsed" => Self::Parsed,
            "patient_matched" => Self::PatientMatched,
            "mapped" => Self::Mapped,
            "sending" => Self::Sending,
            "processed" => Self::Processed,
            "patient_not_found" => Self::PatientNotFound,
            "treatment_ambiguous" => Self::TreatmentAmbiguous,
            "xml_error" => Self::XmlError,
            "mapping_error" => Self::MappingError,
            "send_error" => Self::SendError,
            "failed" => Self::Failed,
            _ => Self::Waiting,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedXmlFile {
    pub id: i64,
    pub device_key: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: Option<i64>,
    pub file_modified_at: Option<String>,
    pub status: XmlFileStatus,
    pub error_message: Option<String>,
    /// Thời gian tạo file (parse từ tên file `YYYYMMDD_HHMMSS`), format `YYYY-MM-DD HH:mm:ss`.
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFolderState {
    pub device_key: String,
    pub tracking_folder: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub tracking_folder: String,
    pub scanned_count: usize,
    pub inserted_count: usize,
    pub files: Vec<TrackedXmlFile>,
}

pub fn get_device_folder(db: &AppDb, device_key: &str) -> Result<DeviceFolderState, String> {
    let conn = lock_conn(db)?;
    let row = conn
        .query_row(
            r#"
            SELECT tracking_folder, updated_at
            FROM device_config
            WHERE device_key = ?1
            "#,
            params![device_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| format!("Đọc device_config thất bại: {e}"))?;

    match row {
        Some((folder, updated_at)) => Ok(DeviceFolderState {
            device_key: device_key.to_string(),
            tracking_folder: if folder.is_empty() {
                None
            } else {
                Some(folder)
            },
            updated_at: Some(updated_at),
        }),
        None => Ok(DeviceFolderState {
            device_key: device_key.to_string(),
            tracking_folder: None,
            updated_at: None,
        }),
    }
}

pub fn set_tracking_folder_and_scan(
    db: &AppDb,
    device_key: &str,
    folder: &str,
) -> Result<ScanResult, String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("Vui lòng chọn thư mục tracking.".into());
    }

    let path = PathBuf::from(folder);
    if !path.is_dir() {
        return Err(format!("Thư mục không tồn tại: {folder}"));
    }

    let conn = lock_conn(db)?;

    conn.execute(
        r#"
        INSERT INTO device_config (device_key, tracking_folder, updated_at)
        VALUES (?1, ?2, datetime('now'))
        ON CONFLICT(device_key) DO UPDATE SET
          tracking_folder = excluded.tracking_folder,
          updated_at = datetime('now')
        "#,
        params![device_key, folder],
    )
    .map_err(|e| format!("Lưu tracking folder thất bại: {e}"))?;

    let scanned = scan_xml_files(&path)?;
    let mut inserted_count = 0usize;

    for file in &scanned {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM xml_files WHERE file_path = ?1",
                params![file.file_path],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| format!("Kiểm tra xml_files thất bại: {e}"))?
            .is_some();

        if exists {
            // Giữ nguyên status; cập nhật metadata + created_at từ tên file.
            conn.execute(
                r#"
                UPDATE xml_files
                SET
                  file_name = ?1,
                  file_size = ?2,
                  file_modified_at = ?3,
                  created_at = ?4,
                  device_key = ?5,
                  updated_at = datetime('now')
                WHERE file_path = ?6
                "#,
                params![
                    file.file_name,
                    file.file_size,
                    file.file_modified_at,
                    file.created_at,
                    device_key,
                    file.file_path,
                ],
            )
            .map_err(|e| format!("Cập nhật xml_files thất bại: {e}"))?;
        } else {
            conn.execute(
                r#"
                INSERT INTO xml_files (
                  device_key, file_name, file_path, file_size, file_modified_at,
                  status, error_message, created_at, updated_at
                ) VALUES (
                  ?1, ?2, ?3, ?4, ?5,
                  'waiting', NULL, ?6, datetime('now')
                )
                "#,
                params![
                    device_key,
                    file.file_name,
                    file.file_path,
                    file.file_size,
                    file.file_modified_at,
                    file.created_at,
                ],
            )
            .map_err(|e| format!("Thêm xml_files thất bại: {e}"))?;
            inserted_count += 1;
        }
    }

    // Xóa bản ghi thuộc folder hiện tại nhưng file không còn trên disk.
    prune_missing_files(&conn, device_key, folder, &scanned)?;

    let files = list_files_conn(&conn, device_key)?;

    Ok(ScanResult {
        tracking_folder: folder.to_string(),
        scanned_count: scanned.len(),
        inserted_count,
        files,
    })
}

pub fn rescan_tracking_folder(db: &AppDb, device_key: &str) -> Result<ScanResult, String> {
    let state = get_device_folder(db, device_key)?;
    let folder = state
        .tracking_folder
        .ok_or_else(|| "Chưa chọn thư mục tracking.".to_string())?;
    set_tracking_folder_and_scan(db, device_key, &folder)
}

pub fn list_xml_files(db: &AppDb, device_key: &str) -> Result<Vec<TrackedXmlFile>, String> {
    let conn = lock_conn(db)?;
    list_files_conn(&conn, device_key)
}

pub fn get_xml_file(db: &AppDb, id: i64) -> Result<Option<TrackedXmlFile>, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        r#"
        SELECT
          id, device_key, file_name, file_path, file_size, file_modified_at,
          status, error_message, created_at, updated_at
        FROM xml_files
        WHERE id = ?1
        "#,
        params![id],
        map_tracked_file,
    )
    .optional()
    .map_err(|error| format!("Đọc xml_files id={id} thất bại: {error}"))
}

struct ScannedFile {
    file_name: String,
    file_path: String,
    file_size: Option<i64>,
    file_modified_at: Option<String>,
    /// `YYYY-MM-DD HH:mm:ss` — ưu tiên parse từ tên file.
    created_at: String,
}

fn scan_xml_files(dir: &Path) -> Result<Vec<ScannedFile>, String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Không đọc được thư mục {}: {e}", dir.display()))?;

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Lỗi đọc entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_xml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("xml"))
            .unwrap_or(false);
        if !is_xml {
            continue;
        }

        let meta = fs::metadata(&path).ok();
        let file_size = meta.as_ref().map(|m| m.len() as i64);
        let file_modified_at = meta
            .and_then(|m| m.modified().ok())
            .and_then(system_time_to_local);

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.xml")
            .to_string();

        let file_path = path.to_string_lossy().to_string();

        // Ưu tiên timestamp trong tên file (máy KR-800), fallback mtime / now.
        let created_at = parse_created_at_from_file_name(&file_name)
            .or_else(|| file_modified_at.clone())
            .unwrap_or_else(now_local_string);

        files.push(ScannedFile {
            file_name,
            file_path,
            file_size,
            file_modified_at,
            created_at,
        });
    }

    files.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(files)
}

/// Parse `YYYYMMDD_HHMMSS` từ tên file KR-800.
///
/// Ví dụ: `HCM2607070269_20260707_145000_TOPCON_KR-800_4780634.xml`
/// → `2026-07-07 14:50:00`
pub fn parse_created_at_from_file_name(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);

    let parts: Vec<&str> = stem.split('_').collect();
    for window in parts.windows(2) {
        let date = window[0];
        let time = window[1];
        if date.len() != 8 || time.len() != 6 {
            continue;
        }
        if !date.chars().all(|c| c.is_ascii_digit()) || !time.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let y: i32 = date[0..4].parse().ok()?;
        let mo: u32 = date[4..6].parse().ok()?;
        let d: u32 = date[6..8].parse().ok()?;
        let hh: u32 = time[0..2].parse().ok()?;
        let mm: u32 = time[2..4].parse().ok()?;
        let ss: u32 = time[4..6].parse().ok()?;

        if !(1..=12).contains(&mo)
            || !(1..=31).contains(&d)
            || hh > 23
            || mm > 59
            || ss > 59
            || !(2000..=2100).contains(&y)
        {
            continue;
        }

        // Xác thực ngày hợp lệ (vd. không cho 2026-02-31).
        if chrono::NaiveDate::from_ymd_opt(y, mo, d).is_none() {
            continue;
        }
        if chrono::NaiveTime::from_hms_opt(hh, mm, ss).is_none() {
            continue;
        }

        return Some(format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}:{ss:02}"));
    }

    None
}

fn prune_missing_files(
    conn: &Connection,
    device_key: &str,
    folder: &str,
    scanned: &[ScannedFile],
) -> Result<(), String> {
    let present: std::collections::HashSet<&str> =
        scanned.iter().map(|f| f.file_path.as_str()).collect();

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, file_path FROM xml_files
            WHERE device_key = ?1
            "#,
        )
        .map_err(|e| format!("Prepare prune thất bại: {e}"))?;

    let rows = stmt
        .query_map(params![device_key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("Query prune thất bại: {e}"))?;

    let folder_prefix = PathBuf::from(folder);
    let mut to_delete = Vec::new();

    for row in rows {
        let (id, file_path) = row.map_err(|e| format!("Row prune thất bại: {e}"))?;
        let path = PathBuf::from(&file_path);
        // Only prune files that belong under the current tracking folder.
        let under_folder = path.starts_with(&folder_prefix);
        if under_folder && !present.contains(file_path.as_str()) {
            to_delete.push(id);
        }
    }

    for id in to_delete {
        conn.execute("DELETE FROM xml_files WHERE id = ?1", params![id])
            .map_err(|e| format!("Xóa xml_files id={id} thất bại: {e}"))?;
    }

    Ok(())
}

fn list_files_conn(conn: &Connection, device_key: &str) -> Result<Vec<TrackedXmlFile>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, created_at, updated_at
            FROM xml_files
            WHERE device_key = ?1
            ORDER BY
              CASE status
                WHEN 'failed' THEN 0
                WHEN 'processing' THEN 1
                WHEN 'waiting' THEN 2
                WHEN 'processed' THEN 3
                ELSE 4
              END,
              created_at DESC,
              file_name ASC
            "#,
        )
        .map_err(|e| format!("Prepare list xml_files thất bại: {e}"))?;

    let rows = stmt
        .query_map(params![device_key], map_tracked_file)
        .map_err(|e| format!("Query list xml_files thất bại: {e}"))?;

    let mut files = Vec::new();
    for row in rows {
        files.push(row.map_err(|e| format!("Map xml_files thất bại: {e}"))?);
    }
    Ok(files)
}

fn map_tracked_file(row: &Row<'_>) -> rusqlite::Result<TrackedXmlFile> {
    let status_raw: String = row.get(6)?;
    Ok(TrackedXmlFile {
        id: row.get(0)?,
        device_key: row.get(1)?,
        file_name: row.get(2)?,
        file_path: row.get(3)?,
        file_size: row.get(4)?,
        file_modified_at: row.get(5)?,
        status: XmlFileStatus::parse(&status_raw),
        error_message: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn system_time_to_local(time: SystemTime) -> Option<String> {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn now_local_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn lock_conn(db: &AppDb) -> Result<MutexGuard<'_, Connection>, String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được kết nối SQLite.".to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_created_at_from_file_name;

    #[test]
    fn parses_kr800_filename() {
        let name = "HCM2607070269_20260707_145000_TOPCON_KR-800_4780634.xml";
        assert_eq!(
            parse_created_at_from_file_name(name).as_deref(),
            Some("2026-07-07 14:50:00")
        );
    }

    #[test]
    fn parses_without_extension() {
        let name = "ABC_20260101_000000_TOPCON";
        assert_eq!(
            parse_created_at_from_file_name(name).as_deref(),
            Some("2026-01-01 00:00:00")
        );
    }

    #[test]
    fn rejects_invalid_date() {
        assert_eq!(
            parse_created_at_from_file_name("X_20260231_120000_Y.xml"),
            None
        );
    }

    #[test]
    fn returns_none_when_no_timestamp() {
        assert_eq!(parse_created_at_from_file_name("plain.xml"), None);
        assert_eq!(parse_created_at_from_file_name("only_20260707.xml"), None);
    }
}
