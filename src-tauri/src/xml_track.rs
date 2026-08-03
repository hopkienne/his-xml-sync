use crate::db::AppDb;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

/// Event tiến trình quét folder (UI progress bar).
pub const SCAN_PROGRESS_EVENT: &str = "kr800:scan-progress";

/// Tiến trình quét / index folder XML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    /// `disk` | `index` | `prune` | `done`
    pub phase: String,
    pub current: usize,
    /// 0 = chưa biết tổng (thanh indeterminate / chỉ hiện current).
    pub total: usize,
    pub percent: u8,
    pub message: String,
}

struct ProgressReporter<'a> {
    app: Option<&'a AppHandle>,
    last_emit: Instant,
    min_interval: Duration,
}

impl<'a> ProgressReporter<'a> {
    fn new(app: Option<&'a AppHandle>) -> Self {
        Self {
            app,
            last_emit: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            min_interval: Duration::from_millis(120),
        }
    }

    fn emit(&mut self, phase: &str, current: usize, total: usize, message: impl Into<String>, force: bool) {
        let Some(app) = self.app else {
            return;
        };
        if !force && self.last_emit.elapsed() < self.min_interval {
            return;
        }
        let percent = if total > 0 {
            (((current as f64) / (total as f64)) * 100.0).clamp(0.0, 100.0) as u8
        } else if phase == "done" {
            100
        } else {
            0
        };
        let payload = ScanProgress {
            phase: phase.to_string(),
            current,
            total,
            percent,
            message: message.into(),
        };
        let _ = app.emit(SCAN_PROGRESS_EVENT, payload);
        self.last_emit = Instant::now();
    }
}

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
    ServiceNotFound,
    XmlError,
    MappingError,
    SendError,
    Failed,
    /// Đã parse hợp lệ, đang chờ lần đo thứ hai cùng Patient.ID.
    AwaitingPair,
    /// Đang ghép cặp / chuẩn bị gửi.
    Pairing,
    /// measuredAt và Patient.No. mâu thuẫn khi ghép.
    PairingError,
    /// Lần đo thừa sau khi đã có cặp.
    ExtraMeasurement,
    /// HDR-9000: hash giống revision đã index, không gửi lại HIS.
    Duplicate,
    /// HDR-9000: XML hợp lệ nhưng không có tag payload được hỗ trợ.
    NoSupportedData,
    /// HDR-9000: mọi field đã được revision mới hơn gửi thành công.
    Superseded,
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
            "service_not_found" => Self::ServiceNotFound,
            "xml_error" => Self::XmlError,
            "mapping_error" => Self::MappingError,
            "send_error" => Self::SendError,
            "failed" => Self::Failed,
            "awaiting_pair" => Self::AwaitingPair,
            "pairing" => Self::Pairing,
            "pairing_error" => Self::PairingError,
            "extra_measurement" => Self::ExtraMeasurement,
            "duplicate" => Self::Duplicate,
            "no_supported_data" => Self::NoSupportedData,
            "superseded" => Self::Superseded,
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
    /// Khi bật: folder_watch tự xử lý file `waiting` lên HIS (không cần bấm Xử lý).
    #[serde(default)]
    pub auto_process_enabled: bool,
    pub updated_at: Option<String>,
}

/// Kết quả quét folder — **không** trả full danh sách file (tránh IPC/React treo với 15k+ bản ghi).
/// UI nên gọi `list_xml_files` theo khoảng `created_at` sau khi quét xong.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub tracking_folder: String,
    pub scanned_count: usize,
    pub inserted_count: usize,
    pub updated_count: usize,
    pub pruned_count: usize,
    /// `true` khi bỏ qua prune vì số file trên disk giảm đột biến (bảo vệ mất dữ liệu).
    pub prune_skipped: bool,
    /// Tổng số bản ghi `xml_files` của device sau quét.
    pub tracked_count: usize,
}

/// File XML mới vừa được index (background / insert-only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertedXmlFile {
    pub id: i64,
    pub file_name: String,
    pub file_path: String,
    pub created_at: String,
}

/// Kết quả quét nền: chỉ insert path chưa có trong DB (không update / không prune).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertNewResult {
    pub tracking_folder: String,
    pub scanned_count: usize,
    pub inserted_count: usize,
    pub skipped_unstable: usize,
    pub inserted: Vec<InsertedXmlFile>,
}

/// Ngưỡng prune an toàn: không xóa hàng loạt nếu “mất” quá nhiều file so với DB.
const PRUNE_MIN_DELETE_TO_GUARD: usize = 50;
const PRUNE_MAX_DROP_RATIO: f64 = 0.10;

pub fn get_device_folder(db: &AppDb, device_key: &str) -> Result<DeviceFolderState, String> {
    let conn = lock_conn(db)?;
    let row = conn
        .query_row(
            r#"
            SELECT tracking_folder, auto_process_enabled, updated_at
            FROM device_config
            WHERE device_key = ?1
            "#,
            params![device_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Đọc device_config thất bại: {e}"))?;

    match row {
        Some((folder, auto_process_enabled, updated_at)) => Ok(DeviceFolderState {
            device_key: device_key.to_string(),
            tracking_folder: if folder.is_empty() {
                None
            } else {
                Some(folder)
            },
            auto_process_enabled: auto_process_enabled != 0,
            updated_at: Some(updated_at),
        }),
        None => Ok(DeviceFolderState {
            device_key: device_key.to_string(),
            tracking_folder: None,
            auto_process_enabled: false,
            updated_at: None,
        }),
    }
}

/// Bật/tắt tự động xử lý HIS cho device (lưu SQLite).
pub fn set_auto_process_enabled(
    db: &AppDb,
    device_key: &str,
    enabled: bool,
) -> Result<DeviceFolderState, String> {
    let conn = lock_conn(db)?;
    let flag: i64 = if enabled { 1 } else { 0 };
    conn.execute(
        r#"
        INSERT INTO device_config (device_key, tracking_folder, auto_process_enabled, updated_at)
        VALUES (?1, '', ?2, datetime('now'))
        ON CONFLICT(device_key) DO UPDATE SET
          auto_process_enabled = excluded.auto_process_enabled,
          updated_at = datetime('now')
        "#,
        params![device_key, flag],
    )
    .map_err(|e| format!("Lưu auto_process_enabled thất bại: {e}"))?;
    drop(conn);
    get_device_folder(db, device_key)
}

/// Một tham số query của API danh sách người bệnh (key–value + cờ gửi).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatientQueryParam {
    pub key: String,
    pub value: String,
    /// `false` → không đưa vào query khi gọi API. JSON cũ thiếu field → true.
    #[serde(default = "default_param_enabled")]
    pub enabled: bool,
}

fn default_param_enabled() -> bool {
    true
}

/// Mặc định khớp hành vi hiện tại của pipeline KR-800.
pub fn default_patient_query_params() -> Vec<PatientQueryParam> {
    vec![
        PatientQueryParam {
            key: "page".into(),
            value: "0".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "sort".into(),
            value: "thoiGianVaoVien,asc".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "size".into(),
            value: "9999".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "tuThoiGianVaoVien".into(),
            value: "".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "denThoiGianVaoVien".into(),
            value: "".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "theoPhongKham".into(),
            value: "false".into(),
            enabled: true,
        },
        PatientQueryParam {
            key: "dsCoSoKcbId".into(),
            value: "4".into(),
            enabled: true,
        },
    ]
}

/// Đọc query params API người bệnh. NULL / rỗng / JSON lỗi → mặc định.
pub fn get_patient_query_params(
    db: &AppDb,
    device_key: &str,
) -> Result<Vec<PatientQueryParam>, String> {
    let conn = lock_conn(db)?;
    let raw: Option<Option<String>> = conn
        .query_row(
            r#"
            SELECT patient_query_params
            FROM device_config
            WHERE device_key = ?1
            "#,
            params![device_key],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Đọc patient_query_params thất bại: {e}"))?;

    match raw {
        Some(Some(json)) if !json.trim().is_empty() => {
            // Bỏ tham số đã ngừng dùng (cấu hình cũ còn lưu trong SQLite).
            Ok(strip_retired_patient_query_params(parse_patient_query_params(
                &json,
            )?))
        }
        _ => Ok(default_patient_query_params()),
    }
}

/// Các key không còn gửi lên API người bệnh.
fn strip_retired_patient_query_params(
    params: Vec<PatientQueryParam>,
) -> Vec<PatientQueryParam> {
    params
        .into_iter()
        .filter(|item| item.key.trim() != "dsTrangThai")
        .collect()
}

/// Lưu query params API người bệnh (JSON array).
pub fn save_patient_query_params(
    db: &AppDb,
    device_key: &str,
    params: Vec<PatientQueryParam>,
) -> Result<Vec<PatientQueryParam>, String> {
    let cleaned =
        sanitize_patient_query_params(strip_retired_patient_query_params(params))?;
    let json = serde_json::to_string(&cleaned)
        .map_err(|e| format!("Serialize patient_query_params thất bại: {e}"))?;
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        INSERT INTO device_config (
          device_key, tracking_folder, auto_process_enabled, patient_query_params, updated_at
        )
        VALUES (?1, '', 0, ?2, datetime('now'))
        ON CONFLICT(device_key) DO UPDATE SET
          patient_query_params = excluded.patient_query_params,
          updated_at = datetime('now')
        "#,
        params![device_key, json],
    )
    .map_err(|e| format!("Lưu patient_query_params thất bại: {e}"))?;
    Ok(cleaned)
}

fn parse_patient_query_params(json: &str) -> Result<Vec<PatientQueryParam>, String> {
    match serde_json::from_str::<Vec<PatientQueryParam>>(json) {
        Ok(parsed) => Ok(parsed),
        // JSON hỏng schema không nên phá runtime — fallback default.
        Err(_) => Ok(default_patient_query_params()),
    }
}

fn sanitize_patient_query_params(
    params: Vec<PatientQueryParam>,
) -> Result<Vec<PatientQueryParam>, String> {
    let mut cleaned = Vec::with_capacity(params.len());
    let mut seen = std::collections::HashSet::new();
    for item in params {
        let key = item.key.trim().to_string();
        if key.is_empty() {
            return Err("Tên tham số (key) không được để trống.".into());
        }
        if !seen.insert(key.clone()) {
            return Err(format!("Tham số «{key}» bị trùng."));
        }
        cleaned.push(PatientQueryParam {
            key,
            value: item.value,
            enabled: item.enabled,
        });
    }
    Ok(cleaned)
}

/// Đọc cờ auto-process (mặc định false nếu chưa có cấu hình device).
pub fn is_auto_process_enabled(db: &AppDb, device_key: &str) -> bool {
    get_device_folder(db, device_key)
        .map(|s| s.auto_process_enabled)
        .unwrap_or(false)
}

pub fn set_tracking_folder_and_scan(
    app: Option<&AppHandle>,
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

    let mut progress = ProgressReporter::new(app);
    progress.emit(
        "disk",
        0,
        0,
        "Đang đọc thư mục trên disk…",
        true,
    );

    // Scan disk **trước** khi giữ lock SQLite lâu (giảm contention).
    let scanned = scan_xml_files(&path, |xml_found, entries_seen| {
        progress.emit(
            "disk",
            xml_found,
            0,
            format!("Đang đọc disk… {xml_found} file XML (đã duyệt {entries_seen} mục)"),
            false,
        );
    })?;

    let total = scanned.len();
    progress.emit(
        "index",
        0,
        total,
        format!("Đã tìm {total} file XML — đang ghi vào cơ sở dữ liệu…"),
        true,
    );

    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("Bắt đầu transaction thất bại: {e}"))?;

    // Giữ nguyên auto_process_enabled khi chỉ đổi folder (ON CONFLICT không ghi đè cờ).
    tx.execute(
        r#"
        INSERT INTO device_config (device_key, tracking_folder, auto_process_enabled, updated_at)
        VALUES (?1, ?2, 0, datetime('now'))
        ON CONFLICT(device_key) DO UPDATE SET
          tracking_folder = excluded.tracking_folder,
          updated_at = datetime('now')
        "#,
        params![device_key, folder],
    )
    .map_err(|e| format!("Lưu tracking folder thất bại: {e}"))?;

    let existing = load_existing_files(&tx, device_key)?;
    let mut inserted_count = 0usize;
    let mut updated_count = 0usize;
    let mut present_paths = std::collections::HashSet::with_capacity(scanned.len());

    for (idx, file) in scanned.iter().enumerate() {
        present_paths.insert(file.file_path.clone());

        match existing.get(&file.file_path) {
            Some(prev) => {
                let meta_changed = prev.file_size != file.file_size
                    || prev.file_modified_at != file.file_modified_at
                    || prev.created_at != file.created_at
                    || prev.file_name != file.file_name
                    || prev.device_key != device_key;
                if meta_changed {
                    tx.execute(
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
                    updated_count += 1;
                }
            }
            None => {
                tx.execute(
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

        let current = idx + 1;
        // Emit thường xuyên hơn khi total nhỏ; throttle trong ProgressReporter.
        if current == total || current % 20 == 0 {
            progress.emit(
                "index",
                current,
                total,
                format!("Đang index {current}/{total} file XML…"),
                current == total,
            );
        }
    }

    progress.emit(
        "prune",
        total,
        total.max(1),
        "Đang dọn bản ghi file không còn trên disk…",
        true,
    );

    let (pruned_count, prune_skipped) =
        prune_missing_files_safe(&tx, device_key, folder, &present_paths, existing.len())?;

    tx.commit()
        .map_err(|e| format!("Commit transaction thất bại: {e}"))?;

    let tracked_count = count_files_conn(&conn, device_key)?;

    progress.emit(
        "done",
        total,
        total.max(1),
        format!(
            "Hoàn tất: {total} XML — thêm {inserted_count}, cập nhật {updated_count}, tổng theo dõi {tracked_count}"
        ),
        true,
    );

    Ok(ScanResult {
        tracking_folder: folder.to_string(),
        scanned_count: scanned.len(),
        inserted_count,
        updated_count,
        pruned_count,
        prune_skipped,
        tracked_count,
    })
}

pub fn rescan_tracking_folder(
    app: Option<&AppHandle>,
    db: &AppDb,
    device_key: &str,
) -> Result<ScanResult, String> {
    let state = get_device_folder(db, device_key)?;
    let folder = state
        .tracking_folder
        .ok_or_else(|| "Chưa chọn thư mục tracking.".to_string())?;
    set_tracking_folder_and_scan(app, db, device_key, &folder)
}

/// Quét nền: chỉ **INSERT** file XML chưa có trong DB.
/// Không UPDATE metadata, không prune — nhẹ hơn full rescan, dùng cho auto-watch.
///
/// Bỏ qua file còn “đang ghi” (mtime < `min_age`) để tránh đọc XML dở.
pub fn insert_new_xml_files_only(
    db: &AppDb,
    device_key: &str,
    min_age: std::time::Duration,
) -> Result<InsertNewResult, String> {
    let state = get_device_folder(db, device_key)?;
    let folder = state
        .tracking_folder
        .ok_or_else(|| "Chưa chọn thư mục tracking.".to_string())?;
    insert_new_in_folder(db, device_key, &folder, min_age)
}

/// Insert một path XML cụ thể nếu chưa có (từ FS watcher event).
pub fn insert_xml_path_if_new(
    db: &AppDb,
    device_key: &str,
    path: &Path,
    min_age: std::time::Duration,
) -> Result<Option<InsertedXmlFile>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let is_xml = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("xml"))
        .unwrap_or(false);
    if !is_xml {
        return Ok(None);
    }
    if !file_is_stable(path, min_age) {
        return Ok(None);
    }

    let scanned = scanned_file_from_path(path)?;
    let conn = lock_conn(db)?;
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM xml_files WHERE file_path = ?1",
            params![scanned.file_path],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| format!("Kiểm tra xml_files thất bại: {e}"))?
        .is_some();
    if exists {
        return Ok(None);
    }

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
            scanned.file_name,
            scanned.file_path,
            scanned.file_size,
            scanned.file_modified_at,
            scanned.created_at,
        ],
    )
    .map_err(|e| format!("Thêm xml_files thất bại: {e}"))?;

    let id = conn.last_insert_rowid();
    Ok(Some(InsertedXmlFile {
        id,
        file_name: scanned.file_name,
        file_path: scanned.file_path,
        created_at: scanned.created_at,
    }))
}

fn insert_new_in_folder(
    db: &AppDb,
    device_key: &str,
    folder: &str,
    min_age: std::time::Duration,
) -> Result<InsertNewResult, String> {
    let path = PathBuf::from(folder);
    if !path.is_dir() {
        return Err(format!("Thư mục không tồn tại: {folder}"));
    }

    let scanned = scan_xml_files(&path, |_, _| {})?;
    let mut conn = lock_conn(db)?;
    let existing_paths = load_existing_paths(&conn, device_key)?;

    let mut inserted = Vec::new();
    let mut skipped_unstable = 0usize;

    let tx = conn
        .transaction()
        .map_err(|e| format!("Bắt đầu transaction thất bại: {e}"))?;

    for file in &scanned {
        if existing_paths.contains(&file.file_path) {
            continue;
        }
        let file_path = PathBuf::from(&file.file_path);
        if !file_is_stable(&file_path, min_age) {
            skipped_unstable += 1;
            continue;
        }

        tx.execute(
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

        let id = tx.last_insert_rowid();
        inserted.push(InsertedXmlFile {
            id,
            file_name: file.file_name.clone(),
            file_path: file.file_path.clone(),
            created_at: file.created_at.clone(),
        });
    }

    tx.commit()
        .map_err(|e| format!("Commit transaction thất bại: {e}"))?;

    Ok(InsertNewResult {
        tracking_folder: folder.to_string(),
        scanned_count: scanned.len(),
        inserted_count: inserted.len(),
        skipped_unstable,
        inserted,
    })
}

/// Số file cần pipeline xử lý trong khoảng `created_at`.
///
/// Gồm `waiting` và các trạng thái retry; **không** đếm `awaiting_pair` như lỗi,
/// nhưng vẫn tính để periodic process biết còn cặp dở (chờ lần 2 / retry).
pub fn count_waiting_in_range(
    db: &AppDb,
    device_key: &str,
    from_time: &str,
    to_time: &str,
) -> Result<usize, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        r#"
        SELECT COUNT(*) FROM xml_files
        WHERE device_key = ?1
          AND status IN (
            'waiting',
            'awaiting_pair',
            'send_error',
            'patient_not_found',
            'treatment_ambiguous',
            'mapping_error',
            'pairing'
          )
          AND created_at >= ?2
          AND created_at <= ?3
        "#,
        params![device_key, from_time, to_time],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .map_err(|e| format!("Đếm pending thất bại: {e}"))
}

/// Đếm riêng file đang chờ lần đo 2 (info, không phải lỗi).
pub fn count_awaiting_pair_in_range(
    db: &AppDb,
    device_key: &str,
    from_time: &str,
    to_time: &str,
) -> Result<usize, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        r#"
        SELECT COUNT(*) FROM xml_files
        WHERE device_key = ?1
          AND status = 'awaiting_pair'
          AND created_at >= ?2
          AND created_at <= ?3
        "#,
        params![device_key, from_time, to_time],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .map_err(|e| format!("Đếm awaiting_pair thất bại: {e}"))
}

fn load_existing_paths(
    conn: &Connection,
    device_key: &str,
) -> Result<std::collections::HashSet<String>, String> {
    let mut stmt = conn
        .prepare("SELECT file_path FROM xml_files WHERE device_key = ?1")
        .map_err(|e| format!("Prepare paths thất bại: {e}"))?;
    let rows = stmt
        .query_map(params![device_key], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Query paths thất bại: {e}"))?;
    let mut set = std::collections::HashSet::new();
    for row in rows {
        set.insert(row.map_err(|e| format!("Row path thất bại: {e}"))?);
    }
    Ok(set)
}

fn file_is_stable(path: &Path, min_age: std::time::Duration) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    match modified.elapsed() {
        Ok(age) => age >= min_age,
        Err(_) => true, // clock skew: vẫn cho insert
    }
}

fn scanned_file_from_path(path: &Path) -> Result<ScannedFile, String> {
    let meta = fs::metadata(path).ok();
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
    let created_at = parse_created_at_from_file_name(&file_name)
        .or_else(|| file_modified_at.clone())
        .unwrap_or_else(now_local_string);
    Ok(ScannedFile {
        file_name,
        file_path,
        file_size,
        file_modified_at,
        created_at,
    })
}

/// Liệt kê file theo device. Nếu có `from_time` + `to_time` (`YYYY-MM-DD HH:mm:ss`)
/// thì lọc theo `created_at` (inclusive) — **không** load toàn bộ 15k+ file ra UI.
pub fn list_xml_files(
    db: &AppDb,
    device_key: &str,
    from_time: Option<&str>,
    to_time: Option<&str>,
) -> Result<Vec<TrackedXmlFile>, String> {
    let conn = lock_conn(db)?;
    match (from_time, to_time) {
        (Some(from), Some(to)) if !from.is_empty() && !to.is_empty() => {
            list_files_conn_in_range(&conn, device_key, from, to)
        }
        _ => {
            // Không có khoảng ngày: không trả full table (tránh treo UI/IPC).
            // Gọi có range từ frontend; process KR-800 dùng waiting_files riêng.
            Ok(Vec::new())
        }
    }
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

struct ExistingFile {
    file_name: String,
    file_size: Option<i64>,
    file_modified_at: Option<String>,
    created_at: String,
    device_key: String,
}

fn scan_xml_files(
    dir: &Path,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Vec<ScannedFile>, String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Không đọc được thư mục {}: {e}", dir.display()))?;

    let mut files = Vec::new();
    let mut entries_seen = 0usize;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Lỗi đọc entry: {e}"))?;
        entries_seen += 1;
        let path = entry.path();
        if !path.is_file() {
            if entries_seen % 200 == 0 {
                on_progress(files.len(), entries_seen);
            }
            continue;
        }
        let is_xml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("xml"))
            .unwrap_or(false);
        if !is_xml {
            if entries_seen % 200 == 0 {
                on_progress(files.len(), entries_seen);
            }
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

        if files.len() % 50 == 0 || entries_seen % 200 == 0 {
            on_progress(files.len(), entries_seen);
        }
    }

    on_progress(files.len(), entries_seen);
    // Không sort full 15k ở backend scan — UI list theo range đã ORDER BY.
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

fn load_existing_files(
    conn: &Connection,
    device_key: &str,
) -> Result<HashMap<String, ExistingFile>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT file_path, file_name, file_size, file_modified_at, created_at, device_key
            FROM xml_files
            WHERE device_key = ?1
            "#,
        )
        .map_err(|e| format!("Prepare load existing thất bại: {e}"))?;

    let rows = stmt
        .query_map(params![device_key], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ExistingFile {
                    file_name: row.get(1)?,
                    file_size: row.get(2)?,
                    file_modified_at: row.get(3)?,
                    created_at: row.get(4)?,
                    device_key: row.get(5)?,
                },
            ))
        })
        .map_err(|e| format!("Query load existing thất bại: {e}"))?;

    let mut map = HashMap::new();
    for row in rows {
        let (path, file) = row.map_err(|e| format!("Row load existing thất bại: {e}"))?;
        map.insert(path, file);
    }
    Ok(map)
}

/// Xóa bản ghi thuộc folder hiện tại nhưng file không còn trên disk.
/// Bảo vệ: nếu số bản ghi “mất” quá lớn so với DB → bỏ qua prune (tránh wipe do mount lỗi / folder tạm trống).
fn prune_missing_files_safe(
    conn: &Connection,
    device_key: &str,
    folder: &str,
    present_paths: &std::collections::HashSet<String>,
    existing_count: usize,
) -> Result<(usize, bool), String> {
    // Disk trống hoàn toàn trong khi DB còn dữ liệu → rất có thể lỗi đọc folder, không prune.
    if present_paths.is_empty() && existing_count > 0 {
        return Ok((0, true));
    }

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
        let under_folder = path.starts_with(&folder_prefix);
        if under_folder && !present_paths.contains(&file_path) {
            to_delete.push(id);
        }
    }

    if to_delete.is_empty() {
        return Ok((0, false));
    }

    if should_skip_prune(existing_count, to_delete.len()) {
        return Ok((0, true));
    }

    // Xóa theo batch thay vì từng id.
    const BATCH: usize = 200;
    let mut pruned = 0usize;
    for chunk in to_delete.chunks(BATCH) {
        let placeholders = chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM xml_files WHERE id IN ({placeholders})");
        let n = conn
            .execute(&sql, rusqlite::params_from_iter(chunk.iter().copied()))
            .map_err(|e| format!("Xóa batch xml_files thất bại: {e}"))?;
        pruned += n;
    }

    Ok((pruned, false))
}

/// `true` khi số bản ghi sắp xóa quá lớn — khả năng cao do scan thiếu / folder lỗi.
pub fn should_skip_prune(existing_count: usize, delete_count: usize) -> bool {
    if delete_count == 0 || existing_count == 0 {
        return false;
    }
    if delete_count < PRUNE_MIN_DELETE_TO_GUARD {
        return false;
    }
    let ratio = delete_count as f64 / existing_count as f64;
    ratio > PRUNE_MAX_DROP_RATIO
}

fn list_files_conn_in_range(
    conn: &Connection,
    device_key: &str,
    from_time: &str,
    to_time: &str,
) -> Result<Vec<TrackedXmlFile>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, created_at, updated_at
            FROM xml_files
            WHERE device_key = ?1
              AND created_at >= ?2
              AND created_at <= ?3
            ORDER BY
              CASE status
                WHEN 'failed' THEN 0
                WHEN 'pairing_error' THEN 0
                WHEN 'send_error' THEN 0
                WHEN 'extra_measurement' THEN 0
                WHEN 'processing' THEN 1
                WHEN 'pairing' THEN 1
                WHEN 'sending' THEN 1
                WHEN 'waiting' THEN 2
                WHEN 'awaiting_pair' THEN 2
                WHEN 'processed' THEN 3
                ELSE 4
              END,
              created_at DESC,
              file_name ASC
            "#,
        )
        .map_err(|e| format!("Prepare list xml_files range thất bại: {e}"))?;

    let rows = stmt
        .query_map(params![device_key, from_time, to_time], map_tracked_file)
        .map_err(|e| format!("Query list xml_files range thất bại: {e}"))?;

    let mut files = Vec::new();
    for row in rows {
        files.push(row.map_err(|e| format!("Map xml_files thất bại: {e}"))?);
    }
    Ok(files)
}

fn count_files_conn(conn: &Connection, device_key: &str) -> Result<usize, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM xml_files WHERE device_key = ?1",
        params![device_key],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .map_err(|e| format!("Đếm xml_files thất bại: {e}"))
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
    use super::{parse_created_at_from_file_name, should_skip_prune};

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

    #[test]
    fn prune_guard_allows_small_deletes() {
        assert!(!should_skip_prune(1000, 10));
        assert!(!should_skip_prune(100, 49));
    }

    #[test]
    fn prune_guard_blocks_mass_delete() {
        // ~15k → 2: delete 15462
        assert!(should_skip_prune(15464, 15462));
        // 15% drop over min threshold
        assert!(should_skip_prune(1000, 150));
    }

    #[test]
    fn prune_guard_empty_delete() {
        assert!(!should_skip_prune(100, 0));
    }
}
