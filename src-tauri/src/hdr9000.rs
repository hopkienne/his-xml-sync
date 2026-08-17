use crate::{
    app_logger,
    db::AppDb,
    his_api,
    refraction_catalog,
    settings::{self, AppSettings},
    xml_track::{DeviceFolderState, ScanProgress, TrackedXmlFile, XmlFileStatus},
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::Client;
use roxmltree::Document;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

pub const DEVICE_KEY: &str = "hdr-9000";
pub const SCAN_PROGRESS_EVENT: &str = "hdr9000:scan-progress";
pub const FILE_PROGRESS_EVENT: &str = "hdr9000:file-progress";
const PATIENT_PATH: &str = "/api/his/v1/nb-kham-ck-mat/nguoi-benh";
const SUMMARY_PATH: &str = "/api/his/v1/nb-dot-dieu-tri/tong-hop";
const UPDATE_PATH: &str = "/api/his/v1/nb-kham-ck-mat";
const LEASE_SECONDS: i64 = 120;
const LEASE_HEARTBEAT_SECONDS: u64 = 30;
const POLL_INTERVAL_SECONDS: u64 = 20;
const WATCH_CONFIG_INTERVAL_SECONDS: u64 = 1;
const RECOVERY_INTERVAL_SECONDS: u64 = 30;
const MIN_FILE_AGE_SECONDS: u64 = 2;
const EVENT_SETTLE_MILLIS: u64 = 2_200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hdr9000ProcessResult {
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub files: Vec<TrackedXmlFile>,
}

pub struct Hdr9000ProcessState {
    run_lock: Mutex<()>,
    token_lock: Mutex<()>,
    pub instance_id: String,
}

impl Default for Hdr9000ProcessState {
    fn default() -> Self {
        Self {
            run_lock: Mutex::new(()),
            token_lock: Mutex::new(()),
            instance_id: format!("hdr9000-{}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap_or_default()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedHdr9000 {
    pub patient_id: Option<String>,
    pub ma_ho_so: String,
    pub measurement_date: Option<NaiveDate>,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hdr9000ParseError {
    Xml(String),
    WrongModel(String),
    MissingFileStem,
}

impl std::fmt::Display for Hdr9000ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(message) => write!(f, "{message}"),
            Self::WrongModel(model) => write!(f, "Product_Model không phải HDR-9000: {}", model.trim()),
            Self::MissingFileStem => write!(f, "Tên file không có mã hồ sơ."),
        }
    }
}

pub fn parse_hdr9000_xml(bytes: &[u8], file_name: &str) -> Result<ParsedHdr9000, Hdr9000ParseError> {
    let xml = decode_xml(bytes);
    let document = Document::parse(&xml).map_err(|e| Hdr9000ParseError::Xml(format!("XML không hợp lệ: {e}")))?;
    let model = tag_text(&document, "Product_Model").unwrap_or_default();
    if model.trim() != "HDR-9000" {
        return Err(Hdr9000ParseError::WrongModel(model));
    }
    let ma_ho_so = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if ma_ho_so.is_empty() {
        return Err(Hdr9000ParseError::MissingFileStem);
    }
    Ok(ParsedHdr9000 {
        patient_id: tag_text(&document, "Patient_ID").filter(|value| !value.is_empty()),
        ma_ho_so,
        measurement_date: tag_text(&document, "Measurement_Date").and_then(|value| parse_measurement_date(&value)),
        payload: sparse_payload(&document),
    })
}

fn decode_xml(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn tag_text(document: &Document<'_>, name: &str) -> Option<String> {
    document.descendants().find(|node| node.is_element() && node.tag_name().name() == name)
        .and_then(|node| node.text()).map(|value| value.trim().to_string())
}

fn parse_measurement_date(value: &str) -> Option<NaiveDate> {
    if value.len() != 8 || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y%m%d").ok()
}

fn text(document: &Document<'_>, tag: &str) -> Option<Value> {
    tag_text(document, tag).filter(|value| !value.is_empty()).map(Value::from)
}

fn put_value(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value { map.insert(key.to_string(), value); }
}

fn put_object(root: &mut Map<String, Value>, key: &str, fields: Map<String, Value>) {
    if !fields.is_empty() { root.insert(key.to_string(), Value::Object(fields)); }
}

fn sparse_payload(document: &Document<'_>) -> Value {
    let mut root = Map::new();
    let mut right_far = Map::new();
    put_value(&mut right_far, "sphId", text(document, "Final_Prescription_Data_FAR_Sph-Right"));
    put_value(&mut right_far, "cylId", text(document, "Final_Prescription_Data_FAR_Cyl-Right"));
    put_value(&mut right_far, "axId", text(document, "Final_Prescription_Data_FAR_Axis-Right"));
    put_value(&mut right_far, "thiLucId", text(document, "Final_Prescription_Data_FAR_VA-Right"));
    put_object(&mut root, "matPhaiKinhMoi", right_far);

    let mut left_far = Map::new();
    put_value(&mut left_far, "sphId", text(document, "Final_Prescription_Data_FAR_Sph-Left"));
    put_value(&mut left_far, "cylId", text(document, "Final_Prescription_Data_FAR_Cyl-Left"));
    put_value(&mut left_far, "axId", text(document, "Final_Prescription_Data_FAR_Axis-Left"));
    put_value(&mut left_far, "thiLucId", text(document, "Final_Prescription_Data_FAR_VA-Left"));
    put_object(&mut root, "matTraiKinhMoi", left_far);

    let mut right_near = Map::new();
    put_value(&mut right_near, "donViAddId", text(document, "Final_Prescription_Data_FAR_ADD-Right"));
    put_value(&mut right_near, "sphId", text(document, "Final_Prescription_Data_NEAR_Sph-Right"));
    put_value(&mut right_near, "cylId", text(document, "Final_Prescription_Data_NEAR_Cyl-Right"));
    put_value(&mut right_near, "axId", text(document, "Final_Prescription_Data_NEAR_Axis-Right"));
    put_value(&mut right_near, "thiLucId", text(document, "Final_Prescription_Data_NEAR_VA-Right"));
    put_object(&mut root, "matPhaiCapKinhNhinGan", right_near);

    let mut left_near = Map::new();
    put_value(&mut left_near, "donViAddId", text(document, "Final_Prescription_Data_FAR_ADD-Left"));
    put_value(&mut left_near, "sphId", text(document, "Final_Prescription_Data_NEAR_Sph-Left"));
    put_value(&mut left_near, "cylId", text(document, "Final_Prescription_Data_NEAR_Cyl-Left"));
    put_value(&mut left_near, "axId", text(document, "Final_Prescription_Data_NEAR_Axis-Left"));
    put_value(&mut left_near, "thiLucId", text(document, "Final_Prescription_Data_NEAR_VA-Left"));
    put_object(&mut root, "matTraiCapKinhNhinGan", left_near);

    if let Some(value) = text(document, "Far_PD_OU") { root.insert("dongTuXa".to_string(), value); }
    if let Some(value) = text(document, "Near_PD_OU") { root.insert("dongTuGan".to_string(), value); }
    Value::Object(root)
}


/// Snapshot HDR-9000 giữ chuỗi XML thô. Trước khi gửi mới tra ID để retry luôn
/// dùng danh mục hiện hành và thị lực như `20/200` không bị quy đổi.
fn mapped_payload(raw: &Value) -> Result<Value, String> {
    let catalog = refraction_catalog::catalog()?;
    let root = raw.as_object().ok_or_else(|| "Payload HDR-9000 không phải object.".to_string())?;
    let mut mapped_root = Map::new();
    for (object_key, object_value) in root {
        let Some(fields) = object_value.as_object() else {
            mapped_root.insert(object_key.clone(), object_value.clone());
            continue;
        };
        let mut mapped_fields = Map::new();
        for (field, value) in fields {
            let raw_value = value.as_str().ok_or_else(|| format!("Giá trị {object_key}.{field} không phải chuỗi XML."))?;
            let id = match field.as_str() {
                "sphId" => refraction_catalog::sph_id_from_text(catalog, raw_value)?,
                "cylId" => refraction_catalog::cyl_id_from_text(catalog, raw_value)?,
                "axId" => refraction_catalog::axis_id_from_text(catalog, raw_value)?,
                "thiLucId" => refraction_catalog::visual_acuity_id(catalog, raw_value)?,
                "donViAddId" => refraction_catalog::add_id(catalog, raw_value)?,
                _ => return Err(format!("Trường HDR-9000 chưa có quy tắc mapping: {object_key}.{field}")),
            };
            mapped_fields.insert(field.clone(), Value::from(id));
        }
        mapped_root.insert(object_key.clone(), Value::Object(mapped_fields));
    }
    Ok(Value::Object(mapped_root))
}

#[derive(Clone)]
struct Dates {
    filter_date: String,
    date_source: String,
    source_time: String,
}

fn resolve_dates(created: Option<SystemTime>, measurement: Option<NaiveDate>, modified: Option<SystemTime>, discovered: DateTime<Local>) -> Dates {
    let created_text = created.and_then(system_time_to_local);
    let modified_text = modified.and_then(system_time_to_local);
    if let Some(value) = created_text.clone() {
        return Dates { filter_date: value.clone(), date_source: "filesystem_created".into(), source_time: value };
    }
    if let Some(value) = measurement {
        return Dates {
            filter_date: NaiveDateTime::new(value, NaiveTime::MIN).format("%Y-%m-%d %H:%M:%S").to_string(),
            date_source: "xml_measurement_date".into(),
            source_time: modified_text.clone().unwrap_or_else(|| discovered.format("%Y-%m-%d %H:%M:%S").to_string()),
        };
    }
    if let Some(value) = modified_text.clone() {
        return Dates { filter_date: value.clone(), date_source: "filesystem_modified".into(), source_time: value };
    }
    let value = discovered.format("%Y-%m-%d %H:%M:%S").to_string();
    Dates { filter_date: value.clone(), date_source: "discovered_at".into(), source_time: value }
}

fn system_time_to_local(value: SystemTime) -> Option<String> {
    let time: DateTime<Local> = value.into();
    Some(time.format("%Y-%m-%d %H:%M:%S").to_string())
}

pub fn set_tracking_folder_and_scan(app: Option<&AppHandle>, db: &AppDb, folder: &str) -> Result<crate::xml_track::ScanResult, String> {
    let folder = folder.trim();
    if folder.is_empty() || !Path::new(folder).is_dir() {
        return Err("Thư mục tracking HDR-9000 không tồn tại.".into());
    }
    {
        let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
        conn.execute("INSERT INTO device_config(device_key,tracking_folder,auto_process_enabled,updated_at) VALUES(?1,?2,0,datetime('now')) ON CONFLICT(device_key) DO UPDATE SET tracking_folder=excluded.tracking_folder,updated_at=datetime('now')",
            params![DEVICE_KEY, folder]).map_err(|e| format!("Lưu folder HDR-9000: {e}"))?;
    }
    scan_folder(app, db, folder)
}

pub fn rescan_tracking_folder(app: Option<&AppHandle>, db: &AppDb) -> Result<crate::xml_track::ScanResult, String> {
    let folder = folder_state(db)?.tracking_folder.ok_or_else(|| "Chưa chọn thư mục tracking HDR-9000.".to_string())?;
    scan_folder(app, db, &folder)
}

fn scan_folder(app: Option<&AppHandle>, db: &AppDb, folder: &str) -> Result<crate::xml_track::ScanResult, String> {
    scan_folder_with_mode(app, db, folder, false)
}

fn scan_folder_with_mode(app: Option<&AppHandle>, db: &AppDb, folder: &str, incremental: bool) -> Result<crate::xml_track::ScanResult, String> {
    let entries = fs::read_dir(folder).map_err(|e| format!("Không đọc được thư mục HDR-9000: {e}"))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Không đọc được entry trong folder HDR-9000: {e}"))?;
        let file_type = entry.file_type().map_err(|e| format!("Không đọc được loại entry HDR-9000: {e}"))?;
        if file_type.is_file() { paths.push(entry.path()); }
    }
    let total = paths.len();
    let known_metadata = if incremental { Some(load_known_metadata(db)?) } else { None };
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    for (index, path) in paths.iter().enumerate() {
        let current = index + 1;
        if let Some(app) = app.filter(|_| current == total || current % 20 == 0) {
            let percent = if total == 0 { 100 } else { (((index + 1) * 100) / total) as u8 };
            let _ = app.emit(SCAN_PROGRESS_EVENT, ScanProgress { phase: "index".into(), current, total, percent, message: "Đang kiểm tra XML HDR-9000…".into() });
        }
        if known_metadata.as_ref().map(|known| metadata_is_known(known, path)).transpose()?.unwrap_or(false) {
            skipped += 1;
            continue;
        }
        match index_path(db, path, Duration::from_secs(MIN_FILE_AGE_SECONDS))? {
            IndexOutcome::Inserted => inserted += 1,
            IndexOutcome::Skipped | IndexOutcome::Duplicate => skipped += 1,
        }
    }
    let tracked_count: usize = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row("SELECT COUNT(*) FROM hdr9000_revisions", [], |row| row.get::<_, i64>(0))
        .map(|value| value as usize).map_err(|e| e.to_string())?;
    if let Some(app) = app {
        let _ = app.emit(SCAN_PROGRESS_EVENT, ScanProgress { phase: "done".into(), current: total, total, percent: 100, message: format!("HDR-9000: thêm {inserted}, bỏ qua {skipped}.") });
    }
    app_logger::info("hdr9000", &format!("scan folder={folder} scanned={total} indexed={inserted} skipped={skipped}"));
    Ok(crate::xml_track::ScanResult {
        tracking_folder: folder.to_string(), scanned_count: total, inserted_count: inserted,
        updated_count: 0, pruned_count: 0, prune_skipped: false, tracked_count,
    })
}

enum IndexOutcome { Inserted, Skipped, Duplicate }

pub fn index_path(db: &AppDb, path: &Path, min_age: Duration) -> Result<IndexOutcome, String> {
    if !path.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("xml")).unwrap_or(false) {
        return Ok(IndexOutcome::Skipped);
    }
    let metadata_before = fs::metadata(path).map_err(|e| format!("Không đọc metadata XML: {e}"))?;
    let modified_before = metadata_before.modified().ok();
    if let Some(modified) = modified_before.as_ref() {
        match modified.elapsed() {
            Ok(age) if age < min_age => return Ok(IndexOutcome::Skipped),
            Err(_) => return Ok(IndexOutcome::Skipped),
            _ => {}
        }
    }
    let bytes = fs::read(path).map_err(|e| format!("Không đọc XML HDR-9000: {e}"))?;
    let metadata = fs::metadata(path).map_err(|e| format!("Không đọc lại metadata XML: {e}"))?;
    if metadata.len() != metadata_before.len()
        || metadata.modified().ok() != modified_before
        || bytes.len() as u64 != metadata.len()
    {
        app_logger::info("hdr9000", &format!("defer unstable file={}", path.display()));
        return Ok(IndexOutcome::Skipped);
    }
    let path_text = path.to_string_lossy().to_string();
    let modified_text = metadata.modified().ok().and_then(system_time_to_local);
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("unknown.xml");
    let parsed = match parse_hdr9000_xml(&bytes, file_name) {
        Ok(parsed) => parsed,
        Err(Hdr9000ParseError::WrongModel(model)) => {
            app_logger::info("hdr9000", &format!("ignored file={} model={}", file_name, model.trim()));
            return Ok(IndexOutcome::Skipped);
        }
        Err(error) => {
            app_logger::warn("hdr9000", &format!("ignored invalid file={} error={error}", file_name));
            return Ok(IndexOutcome::Skipped);
        }
    };
    let dates = resolve_dates(metadata.created().ok(), parsed.measurement_date, metadata.modified().ok(), Local::now());
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let payload = serde_json::to_string(&parsed.payload).map_err(|e| e.to_string())?;
    let status = if parsed.payload.as_object().map(|value| value.is_empty()).unwrap_or(true) { "no_supported_data" } else { "waiting" };
    let mut conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn.transaction().map_err(|e| format!("Mở transaction HDR-9000: {e}"))?;
    let reserved = tx.execute(
        "INSERT OR IGNORE INTO hdr9000_content_hashes(device_key,content_hash,created_at) VALUES(?1,?2,datetime('now'))",
        params![DEVICE_KEY, hash],
    ).map_err(|e| format!("Đặt chỗ hash HDR-9000: {e}"))?;
    if reserved == 0 {
        tx.rollback().map_err(|e| format!("Hủy transaction hash HDR-9000: {e}"))?;
        app_logger::info("hdr9000", &format!("duplicate file={file_name} model=HDR-9000 hash={}", &hash[..12]));
        return Ok(IndexOutcome::Duplicate);
    }
    tx.execute("INSERT INTO hdr9000_revisions(device_key,file_name,file_path,content_hash,ma_ho_so,patient_id,filter_date,date_source,source_time,snapshot_xml,snapshot_payload,status,discovered_at,created_at,updated_at,file_size,file_modified_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,datetime('now'),datetime('now'),datetime('now'),?13,?14)",
        params![DEVICE_KEY, file_name, path_text, hash, parsed.ma_ho_so, parsed.patient_id, dates.filter_date, dates.date_source, dates.source_time, bytes, payload, status, metadata.len() as i64, modified_text])
        .map_err(|e| format!("Lưu revision HDR-9000: {e}"))?;
    let revision_id: i64 = tx.query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
        .map_err(|e| format!("Đọc revision HDR-9000 vừa lưu: {e}"))?;
    tx.execute("UPDATE hdr9000_content_hashes SET first_revision_id=?1 WHERE device_key=?2 AND content_hash=?3", params![revision_id, DEVICE_KEY, hash])
        .map_err(|e| format!("Cập nhật hash HDR-9000: {e}"))?;
    tx.commit().map_err(|e| format!("Lưu transaction HDR-9000: {e}"))?;
    app_logger::info("hdr9000", &format!("indexed model=HDR-9000 file={} maHoSo={} patientId={:?} hash={} source_time={} date_source={}", file_name, parsed.ma_ho_so, parsed.patient_id, &hash[..12], dates.source_time, dates.date_source));
    Ok(IndexOutcome::Inserted)
}

fn load_known_metadata(db: &AppDb) -> Result<std::collections::HashSet<(String, i64, String)>, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn.prepare("SELECT revision.file_path,revision.file_size,COALESCE(revision.file_modified_at,'') FROM hdr9000_revisions revision JOIN (SELECT file_path,MAX(id) AS id FROM hdr9000_revisions WHERE device_key=?1 GROUP BY file_path) latest ON latest.id=revision.id WHERE revision.file_size IS NOT NULL")
        .map_err(|e| format!("Chuẩn bị metadata HDR-9000: {e}"))?;
    let rows = statement.query_map(params![DEVICE_KEY], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| format!("Đọc metadata HDR-9000: {e}"))?;
    let mut metadata = std::collections::HashSet::new();
    for row in rows {
        metadata.insert(row.map_err(|e| format!("Đọc dòng metadata HDR-9000: {e}"))?);
    }
    Ok(metadata)
}

fn metadata_is_known(known: &std::collections::HashSet<(String, i64, String)>, path: &Path) -> Result<bool, String> {
    if !path.extension().and_then(|value| value.to_str()).map(|value| value.eq_ignore_ascii_case("xml")).unwrap_or(false) {
        return Ok(true);
    }
    let metadata = fs::metadata(path).map_err(|e| format!("Không đọc metadata XML: {e}"))?;
    let modified = metadata.modified().ok().and_then(system_time_to_local).unwrap_or_default();
    Ok(known.contains(&(path.to_string_lossy().to_string(), metadata.len() as i64, modified)))
}

pub fn list_files(db: &AppDb, from: Option<&str>, to: Option<&str>) -> Result<Vec<TrackedXmlFile>, String> {
    let (Some(from), Some(to)) = (from, to) else { return Ok(Vec::new()); };
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn.prepare("SELECT id,file_name,file_path,file_size,file_modified_at,status,error_message,filter_date,updated_at FROM hdr9000_revisions WHERE device_key=?1 AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id")
        .map_err(|e| e.to_string())?;
    let files = statement.query_map(params![DEVICE_KEY, from, to], |row| Ok(TrackedXmlFile {
        id: row.get(0)?, device_key: DEVICE_KEY.into(), file_name: row.get(1)?, file_path: row.get(2)?,
        file_size: row.get(3)?, file_modified_at: row.get(4)?, status: XmlFileStatus::parse(&row.get::<_, String>(5)?),
        error_message: row.get(6)?, created_at: row.get(7)?, updated_at: row.get(8)?,
    })).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string());
    files
}

pub fn folder_state(db: &AppDb) -> Result<DeviceFolderState, String> {
    crate::xml_track::get_device_folder(db, DEVICE_KEY)
}

pub fn count_pending(db: &AppDb) -> Result<usize, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.query_row("SELECT COUNT(*) FROM hdr9000_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found')", params![DEVICE_KEY], |row| row.get::<_, i64>(0))
        .map(|value| value as usize).map_err(|e| e.to_string())
}

/// Watcher riêng cho HDR-9000: notify chụp revision mới sớm, poll metadata bù
/// event bị miss trên network share, recovery chạy độc lập với việc quét folder.
pub fn start_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        run_watch_loop(app).await;
    });
}

pub async fn trigger_auto_process_now(app: &AppHandle) {
    recover_expired_for_app(app);
    schedule_process_pending(app);
}

async fn run_watch_loop(app: AppHandle) {
    let (path_tx, mut path_rx) = mpsc::unbounded_channel::<PathBuf>();
    let (scan_tx, mut scan_rx) = mpsc::unbounded_channel::<(String, Result<crate::xml_track::ScanResult, String>)>();
    let (event_result_tx, mut event_result_rx) = mpsc::unbounded_channel::<Result<(String, usize, usize), String>>();
    let mut current_folder: Option<String> = None;
    let mut watcher: Option<RecommendedWatcher> = None;
    let mut scan_in_flight = false;
    let mut poll = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECONDS));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut config_refresh = tokio::time::interval(Duration::from_secs(WATCH_CONFIG_INTERVAL_SECONDS));
    config_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut recovery = tokio::time::interval(Duration::from_secs(RECOVERY_INTERVAL_SECONDS));
    recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    recovery.tick().await;

    recover_expired_for_app(&app);
    schedule_process_pending(&app);
    loop {
        tokio::select! {
            _ = config_refresh.tick() => {
                let folder = match configured_folder(&app) {
                    Ok(folder) => folder,
                    Err(error) => {
                        app_logger::error("hdr9000", &format!("Không đọc được cấu hình watcher: {error}"));
                        emit_watch_status(&app, false, None, &format!("Lỗi đọc cấu hình HDR-9000: {error}"));
                        continue;
                    }
                };
                if folder != current_folder {
                    current_folder = folder.clone();
                    watcher = folder.as_deref().and_then(|value| start_fs_watcher(value, path_tx.clone()));
                    match folder.as_deref() {
                        Some(folder) => {
                            emit_watch_status(&app, watcher.is_some(), Some(folder), if watcher.is_some() { "Đang theo dõi folder HDR-9000." } else { "Không gắn được filesystem watcher; vẫn đang poll dự phòng." });
                            if !scan_in_flight {
                                scan_in_flight = true;
                                schedule_incremental_scan(&app, folder.to_string(), &scan_tx);
                            }
                        }
                        None => emit_watch_status(&app, false, None, "Chưa chọn thư mục tracking HDR-9000."),
                    }
                }
            }
            _ = poll.tick() => {
                if let Some(folder) = current_folder.clone() {
                    if !scan_in_flight {
                        scan_in_flight = true;
                        if watcher.is_none() {
                            watcher = start_fs_watcher(&folder, path_tx.clone());
                            emit_watch_status(&app, watcher.is_some(), Some(&folder), if watcher.is_some() { "Đang theo dõi folder HDR-9000." } else { "Không gắn được filesystem watcher; vẫn đang poll dự phòng." });
                        }
                        schedule_incremental_scan(&app, folder, &scan_tx);
                    }
                }
            }
            Some((folder, result)) = scan_rx.recv() => {
                scan_in_flight = false;
                if current_folder.as_deref() != Some(folder.as_str()) {
                    if let Some(current) = current_folder.clone() {
                        scan_in_flight = true;
                        schedule_incremental_scan(&app, current, &scan_tx);
                    }
                    continue;
                }
                match result {
                    Ok(result) => {
                        emit_watch_status(&app, watcher.is_some(), Some(&folder), if watcher.is_some() { "Đang theo dõi folder HDR-9000." } else { "Filesystem watcher chưa sẵn sàng; poll dự phòng đang hoạt động." });
                        if result.inserted_count > 0 {
                            emit_indexed(&app, "poll", &folder, result.scanned_count, result.inserted_count);
                            schedule_process_pending(&app);
                        }
                    }
                    Err(error) => {
                        app_logger::error("hdr9000", &format!("Background scan thất bại folder={folder}: {error}"));
                        emit_watch_status(&app, false, Some(&folder), &format!("Không đọc được folder HDR-9000: {error}"));
                    }
                }
            }
            _ = recovery.tick() => {
                recover_expired_for_app(&app);
                schedule_process_pending(&app);
            },
            Some(first_path) = path_rx.recv() => {
                let mut paths = vec![first_path];
                while let Ok(path) = path_rx.try_recv() { paths.push(path); }
                // Chỉ debounce burst đã có sẵn. Event đến trong lúc chờ được giữ
                // cho lượt kế tiếp, tránh nuốt mất lần ghi đè tiếp theo cùng path.
                let event_app = app.clone();
                let result_sender = event_result_tx.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(EVENT_SETTLE_MILLIS)).await;
                    let _ = result_sender.send(index_paths_blocking(&event_app, paths).await);
                });
            }
            Some(result) = event_result_rx.recv() => {
                match result {
                    Ok((folder, scanned, inserted)) if inserted > 0 => {
                        emit_indexed(&app, "watcher", &folder, scanned, inserted);
                        schedule_process_pending(&app);
                    }
                    Ok(_) => {}
                    Err(error) => app_logger::error("hdr9000", &format!("Index event filesystem thất bại: {error}")),
                }
            }
        }
    }
}

fn configured_folder(app: &AppHandle) -> Result<Option<String>, String> {
    let db = app.try_state::<AppDb>().ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
    Ok(folder_state(&db)?.tracking_folder.filter(|folder| !folder.trim().is_empty()))
}

fn start_fs_watcher(folder: &str, sender: mpsc::UnboundedSender<PathBuf>) -> Option<RecommendedWatcher> {
    let mut watcher = match notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
        match result {
            Ok(event) if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any) => {
                for path in event.paths {
                    if path.extension().and_then(|value| value.to_str()).map(|value| value.eq_ignore_ascii_case("xml")).unwrap_or(false) {
                        let _ = sender.send(path);
                    }
                }
            }
            Ok(_) => {}
            Err(error) => app_logger::error("hdr9000", &format!("Filesystem watcher lỗi: {error}")),
        }
    }) {
        Ok(watcher) => watcher,
        Err(error) => { app_logger::error("hdr9000", &format!("Không tạo được filesystem watcher: {error}")); return None; }
    };
    if let Err(error) = watcher.watch(Path::new(folder), RecursiveMode::NonRecursive) {
        app_logger::error("hdr9000", &format!("Không watch được folder={folder}: {error}"));
        return None;
    }
    Some(watcher)
}

async fn scan_incremental_blocking(app: &AppHandle, folder: String) -> Result<crate::xml_track::ScanResult, String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app.try_state::<AppDb>().ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        scan_folder_with_mode(None, &db, &folder, true)
    }).await.map_err(|e| format!("spawn_blocking scan HDR-9000: {e}"))?
}

fn schedule_incremental_scan(
    app: &AppHandle,
    folder: String,
    sender: &mpsc::UnboundedSender<(String, Result<crate::xml_track::ScanResult, String>)>,
) {
    let scan_app = app.clone();
    let result_sender = sender.clone();
    tauri::async_runtime::spawn(async move {
        let result = scan_incremental_blocking(&scan_app, folder.clone()).await;
        let _ = result_sender.send((folder, result));
    });
}

async fn index_paths_blocking(app: &AppHandle, paths: Vec<PathBuf>) -> Result<(String, usize, usize), String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app.try_state::<AppDb>().ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        let folder = folder_state(&db)?.tracking_folder.unwrap_or_default();
        let folder_path = PathBuf::from(&folder);
        let mut seen = std::collections::HashSet::new();
        let mut inserted = 0usize;
        for path in paths {
            if folder.is_empty() || !path.starts_with(&folder_path) { continue; }
            let key = path.to_string_lossy().to_string();
            if !seen.insert(key) { continue; }
            match index_path(&db, &path, Duration::from_secs(MIN_FILE_AGE_SECONDS)) {
                Ok(IndexOutcome::Inserted) => inserted += 1,
                Ok(IndexOutcome::Skipped | IndexOutcome::Duplicate) => {}
                Err(error) => app_logger::error("hdr9000", &format!("Không index được file={}: {error}", path.display())),
            }
        }
        Ok((folder, seen.len(), inserted))
    }).await.map_err(|e| format!("spawn_blocking index HDR-9000: {e}"))?
}

fn emit_watch_status(app: &AppHandle, active: bool, folder: Option<&str>, message: &str) {
    let _ = app.emit("hdr9000:watch-status", serde_json::json!({"active":active,"trackingFolder":folder,"message":message}));
}

fn emit_indexed(app: &AppHandle, source: &str, folder: &str, scanned: usize, inserted: usize) {
    let _ = app.emit("hdr9000:files-indexed", serde_json::json!({"source":source,"insertedCount":inserted,"scannedCount":scanned,"trackingFolder":folder,"inserted":[]}));
}

fn recover_expired_for_app(app: &AppHandle) {
    let Some(db) = app.try_state::<AppDb>() else { return; };
    if let Err(error) = recover_expired(&db) {
        app_logger::error("hdr9000", &format!("Không khôi phục lease HDR-9000 hết hạn: {error}"));
    }
}

fn schedule_process_pending(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { process_pending_if_enabled(&app).await; });
}

async fn process_pending_if_enabled(app: &AppHandle) {
    let Some(db) = app.try_state::<AppDb>() else { return; };
    let auto_enabled = match folder_state(&db) {
        Ok(state) => state.auto_process_enabled,
        Err(error) => { emit_auto_error(app, &format!("Không đọc được cấu hình HDR-9000: {error}")); return; }
    };
    if !auto_enabled { return; }
    let pending = match count_pending(&db) {
        Ok(pending) => pending,
        Err(error) => { emit_auto_error(app, &format!("Không đếm được queue HDR-9000: {error}")); return; }
    };
    if pending == 0 { return; }
    // Recovery không bị giới hạn hôm nay: file lỗi từ ngày cũ phải được retry sau restart.
    let range = match pending_range(&db) {
        Ok(range) => range,
        Err(error) => { emit_auto_error(app, &format!("Không đọc được khoảng retry HDR-9000: {error}")); return; }
    };
    let Some((from, to)) = range else { return; };
    let Some(process_state) = app.try_state::<Hdr9000ProcessState>() else { return; };
    match try_process(app, &db, &process_state, &from, &to).await {
        Ok(Some(result)) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":true,"message":format!("Tự xử lý: {}/{} thành công; bỏ qua {}; lỗi {}.",result.processed,result.total,result.skipped,result.failed),"fromTime":from,"toTime":to,"total":result.total,"processed":result.processed,"failed":result.failed,"skipped":result.skipped,"busy":false})); }
        Ok(None) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":true,"message":"Pipeline HDR-9000 đang bận.","fromTime":from,"toTime":to,"total":0,"processed":0,"failed":0,"skipped":0,"busy":true})); }
        Err(error) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":false,"message":error,"fromTime":from,"toTime":to,"total":0,"processed":0,"failed":0,"skipped":0,"busy":false})); }
    }
}

fn emit_auto_error(app: &AppHandle, message: &str) {
    app_logger::error("hdr9000", message);
    let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":false,"message":message,"fromTime":"","toTime":"","total":0,"processed":0,"failed":0,"skipped":0,"busy":false}));
}

pub async fn process(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, from: &str, to: &str) -> Result<Hdr9000ProcessResult, String> {
    let _guard = state.run_lock.lock().await;
    process_locked(app, db, state, from, to).await
}

async fn process_locked(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, from: &str, to: &str) -> Result<Hdr9000ProcessResult, String> {
    recover_expired(db)?;
    let settings = settings::load(db)?;
    if settings.his_api_url.trim().is_empty() || settings.username.trim().is_empty() {
        return Err("Chưa cấu hình API URL hoặc tài khoản HIS.".into());
    }
    let ids = retryable_ids(db, from, to)?;
    let total = ids.len();
    let client = Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let mut processed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    for id in ids {
        match process_one(app, db, state, &client, &settings, id).await {
            Ok(ProcessOne::Processed) => processed += 1,
            Ok(ProcessOne::Skipped) => skipped += 1,
            Err(error) => {
                failed += 1;
                let _ = fail(db, id, status_for_error(&error), &error, &state.instance_id);
                emit_file(app, db, id);
                app_logger::error("hdr9000", &format!("revision_id={id} {error}"));
            }
        }
    }
    Ok(Hdr9000ProcessResult { total, processed, failed, skipped, files: list_files(db, Some(from), Some(to))? })
}

fn pending_range(db: &AppDb) -> Result<Option<(String, String)>, String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row("SELECT MIN(filter_date), MAX(filter_date) FROM hdr9000_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found')", params![DEVICE_KEY],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)))
        .map(|(from, to)| from.zip(to)).map_err(|e| e.to_string())
}

pub async fn try_process(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, from: &str, to: &str) -> Result<Option<Hdr9000ProcessResult>, String> {
    let _guard = match state.run_lock.try_lock() { Ok(guard) => guard, Err(_) => return Ok(None) };
    process_locked(app, db, state, from, to).await.map(Some)
}

enum ProcessOne { Processed, Skipped }

async fn process_one(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, client: &Client, settings: &AppSettings, id: i64) -> Result<ProcessOne, String> {
    if !claim(db, id, &state.instance_id)? { return Ok(ProcessOne::Skipped); }
    let revision = load_revision(db, id)?.ok_or_else(|| "Revision không tồn tại.".to_string())?;
    let parsed = parse_hdr9000_xml(&revision.xml, &revision.file_name).map_err(|e| {
        fail(db, id, "xml_error", &e.to_string(), &state.instance_id).ok(); e.to_string()
    })?;
    let mapped = mapped_payload(&parsed.payload).map_err(|error| {
        let message = format!("mapping_error: {error}");
        fail(db, id, "mapping_error", &message, &state.instance_id).ok();
        emit_file(app, db, id);
        message
    })?;
    if !claim_patient_lease(db, &revision.ma_ho_so, &state.instance_id)? {
        release_claim(db, id, &state.instance_id)?;
        app_logger::info("hdr9000", &format!("revision_id={id} maHoSo={} đang được instance khác xử lý", revision.ma_ho_so));
        emit_file(app, db, id);
        return Ok(ProcessOne::Skipped);
    }

    let operation = async {
        let payload = stale_filtered_payload(db, &revision, &mapped)?;
        if payload.as_object().map(|value| value.is_empty()).unwrap_or(true) {
            set_status(db, id, "superseded", None, &state.instance_id)?;
            emit_file(app, db, id);
            Ok(ProcessOne::Skipped)
        } else {
            let dv_kham_id = resolve_service_id(db, state, client, settings, &revision.ma_ho_so).await?;
            let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
            save_request(db, id, dv_kham_id, &body, &state.instance_id)?;
            let response = match send_update(db, state, client, settings, dv_kham_id, &payload, id).await {
                Ok(response) => Ok(response),
                Err(error) if is_invalid_service(&error) => {
                    clear_service_cache(db, &revision.ma_ho_so)?;
                    let fresh_id = resolve_service_id(db, state, client, settings, &revision.ma_ho_so).await?;
                    save_request(db, id, fresh_id, &body, &state.instance_id)?;
                    send_update(db, state, client, settings, fresh_id, &payload, id).await
                }
                Err(error) => Err(error),
            }?;
            finish_success(db, &revision, &body, &response, &state.instance_id)?;
            emit_file(app, db, id);
            Ok(ProcessOne::Processed)
        }
    };
    tokio::pin!(operation);
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            _ = tokio::time::sleep(Duration::from_secs(LEASE_HEARTBEAT_SECONDS)) => {
                match renew_leases(db, id, &revision.ma_ho_so, &state.instance_id) {
                    Ok(true) => {}
                    Ok(false) => break Err("send_error: Mất ownership lease HDR-9000 trong khi đang xử lý.".to_string()),
                    Err(error) => break Err(format!("send_error: Không gia hạn được lease HDR-9000: {error}")),
                }
            }
        }
    };

    // Lưu trạng thái revision trước khi nhả khóa liên tiến trình, tránh một
    // instance khác gửi revision cùng hồ sơ trong khoảng rất ngắn khi vừa lỗi.
    if let Err(error) = &result {
        if let Err(fail_error) = fail(db, id, status_for_error(error), error, &state.instance_id) {
            app_logger::error("hdr9000", &format!("Không lưu lỗi revision_id={id} trước khi nhả lease: {fail_error}"));
        }
    }
    if let Err(error) = release_patient_lease(db, &revision.ma_ho_so, &state.instance_id) {
        app_logger::error("hdr9000", &format!("Không nhả lease maHoSo={} revision_id={id}: {error}", revision.ma_ho_so));
    }
    result
}

fn status_for_error(error: &str) -> &'static str {
    if error.starts_with("patient_not_found:") { "patient_not_found" }
    else if error.starts_with("service_not_found:") { "service_not_found" }
    else if error.starts_with("xml_error:") { "xml_error" }
    else { "send_error" }
}

struct Revision { id: i64, file_name: String, ma_ho_so: String, source_time: String, xml: Vec<u8> }

fn load_revision(db: &AppDb, id: i64) -> Result<Option<Revision>, String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row("SELECT id,file_name,ma_ho_so,source_time,snapshot_xml FROM hdr9000_revisions WHERE id=?1 AND device_key=?2", params![id, DEVICE_KEY],
            |row| Ok(Revision { id: row.get(0)?, file_name: row.get(1)?, ma_ho_so: row.get(2)?, source_time: row.get(3)?, xml: row.get(4)? }))
        .optional().map_err(|e| e.to_string())
}

fn retryable_ids(db: &AppDb, from: &str, to: &str) -> Result<Vec<i64>, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn.prepare("SELECT id FROM hdr9000_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found') AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id").map_err(|e| e.to_string())?;
    let ids = statement.query_map(params![DEVICE_KEY, from, to], |row| row.get(0)).map_err(|e| e.to_string())?.collect::<Result<Vec<i64>, _>>().map_err(|e| e.to_string());
    ids
}

fn claim(db: &AppDb, id: i64, owner: &str) -> Result<bool, String> {
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='processing',error_message=NULL,sending_started_at=datetime('now'),sending_owner_id=?1,sending_lease_until=datetime('now','+120 seconds'),updated_at=datetime('now') WHERE id=?2 AND device_key=?3 AND status IN ('waiting','send_error','patient_not_found','service_not_found')",
        params![owner, id, DEVICE_KEY]).map_err(|e| e.to_string())?;
    Ok(changed == 1)
}

fn release_claim(db: &AppDb, id: i64, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='waiting',error_message=NULL,sending_started_at=NULL,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?1 AND device_key=?2 AND status='processing' AND sending_owner_id=?3",
        params![id, DEVICE_KEY, owner],
    ).map(|_| ()).map_err(|e| e.to_string())
}

fn claim_patient_lease(db: &AppDb, ma_ho_so: &str, owner: &str) -> Result<bool, String> {
    let lease_modifier = format!("+{LEASE_SECONDS} seconds");
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "INSERT INTO hdr9000_patient_leases(device_key,ma_ho_so,owner_id,lease_until,updated_at) VALUES(?1,?2,?3,datetime('now',?4),datetime('now')) ON CONFLICT(device_key,ma_ho_so) DO UPDATE SET owner_id=excluded.owner_id,lease_until=excluded.lease_until,updated_at=excluded.updated_at WHERE hdr9000_patient_leases.lease_until <= datetime('now') OR hdr9000_patient_leases.owner_id=excluded.owner_id",
        params![DEVICE_KEY, ma_ho_so, owner, lease_modifier],
    ).map_err(|e| format!("Đặt lease maHoSo HDR-9000: {e}"))?;
    Ok(changed == 1)
}

fn renew_leases(db: &AppDb, id: i64, ma_ho_so: &str, owner: &str) -> Result<bool, String> {
    let lease_modifier = format!("+{LEASE_SECONDS} seconds");
    let mut conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn.transaction().map_err(|e| format!("Mở transaction gia hạn lease: {e}"))?;
    let revision_changed = tx.execute(
        "UPDATE hdr9000_revisions SET sending_lease_until=datetime('now',?1),updated_at=datetime('now') WHERE id=?2 AND device_key=?3 AND sending_owner_id=?4 AND status IN ('processing','sending') AND sending_lease_until > datetime('now')",
        params![lease_modifier, id, DEVICE_KEY, owner],
    ).map_err(|e| format!("Gia hạn lease revision HDR-9000: {e}"))?;
    let patient_changed = tx.execute(
        "UPDATE hdr9000_patient_leases SET lease_until=datetime('now',?1),updated_at=datetime('now') WHERE device_key=?2 AND ma_ho_so=?3 AND owner_id=?4 AND lease_until > datetime('now')",
        params![lease_modifier, DEVICE_KEY, ma_ho_so, owner],
    ).map_err(|e| format!("Gia hạn lease maHoSo HDR-9000: {e}"))?;
    if revision_changed != 1 || patient_changed != 1 {
        tx.rollback().map_err(|e| format!("Rollback gia hạn lease HDR-9000: {e}"))?;
        return Ok(false);
    }
    tx.commit().map_err(|e| format!("Commit gia hạn lease HDR-9000: {e}"))?;
    Ok(true)
}

fn release_patient_lease(db: &AppDb, ma_ho_so: &str, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "DELETE FROM hdr9000_patient_leases WHERE device_key=?1 AND ma_ho_so=?2 AND owner_id=?3",
        params![DEVICE_KEY, ma_ho_so, owner],
    ).map(|_| ()).map_err(|e| format!("Nhả lease maHoSo HDR-9000: {e}"))
}

fn save_request(db: &AppDb, id: i64, dv_kham_id: i64, body: &str, owner: &str) -> Result<(), String> {
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='sending',dv_kham_id=?1,request_payload=?2,attempt_count=attempt_count+1,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5 AND status IN ('processing','sending') AND sending_lease_until > datetime('now')",
        params![dv_kham_id, body, id, owner, DEVICE_KEY]).map_err(|e| e.to_string())?;
    if changed == 1 { Ok(()) } else { Err("Revision HDR-9000 không còn lease hợp lệ trước khi gửi HIS.".into()) }
}

fn fail(db: &AppDb, id: i64, status: &str, message: &str, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status=?1,error_message=?2,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5",
        params![status, message, id, owner, DEVICE_KEY]).map(|_| ()).map_err(|e| e.to_string())
}

fn set_status(db: &AppDb, id: i64, status: &str, message: Option<&str>, owner: &str) -> Result<(), String> {
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status=?1,error_message=?2,sending_owner_id=NULL,sending_lease_until=NULL,processed_at=CASE WHEN ?1='superseded' THEN datetime('now') ELSE processed_at END,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5 AND sending_lease_until > datetime('now')",
        params![status, message, id, owner, DEVICE_KEY]).map_err(|e| e.to_string())?;
    if changed == 1 { Ok(()) } else { Err("Revision HDR-9000 không còn lease hợp lệ khi đổi trạng thái.".into()) }
}

fn recover_expired(db: &AppDb) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='send_error',error_message='Recovery: lease xử lý đã hết hạn.',sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE device_key=?1 AND status IN ('processing','sending') AND sending_lease_until < datetime('now')", params![DEVICE_KEY]).map(|_| ()).map_err(|e| e.to_string())
}

fn leaf_fields(value: &Value, prefix: &str, result: &mut Vec<String>) {
    if let Value::Object(values) = value {
        for (key, child) in values {
            let path = if prefix.is_empty() { key.to_string() } else { format!("{prefix}.{key}") };
            leaf_fields(child, &path, result);
        }
    } else { result.push(prefix.to_string()); }
}

fn remove_stale(value: &Value, prefix: &str, revision: &Revision, db: &AppDb) -> Result<Option<Value>, String> {
    match value {
        Value::Object(values) => {
            let mut kept = Map::new();
            for (key, child) in values {
                let path = if prefix.is_empty() { key.to_string() } else { format!("{prefix}.{key}") };
                if let Some(value) = remove_stale(child, &path, revision, db)? { kept.insert(key.clone(), value); }
            }
            Ok(if kept.is_empty() { None } else { Some(Value::Object(kept)) })
        }
        _ => {
            let latest: Option<(String, i64)> = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
                .query_row("SELECT source_time,revision_id FROM hdr9000_field_versions WHERE device_key=?1 AND ma_ho_so=?2 AND field_path=?3",
                    params![DEVICE_KEY, revision.ma_ho_so, prefix], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|e| e.to_string())?;
            let stale = latest.map(|(time, id)| time > revision.source_time || (time == revision.source_time && id > revision.id)).unwrap_or(false);
            Ok(if stale { None } else { Some(value.clone()) })
        }
    }
}

fn stale_filtered_payload(db: &AppDb, revision: &Revision, payload: &Value) -> Result<Value, String> {
    Ok(remove_stale(payload, "", revision, db)?.unwrap_or_else(|| Value::Object(Map::new())))
}

fn finish_success(db: &AppDb, revision: &Revision, request: &str, response: &str, owner: &str) -> Result<(), String> {
    let payload: Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let mut fields = Vec::new();
    leaf_fields(&payload, "", &mut fields);
    let mut conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for field in fields {
        tx.execute("INSERT INTO hdr9000_field_versions(device_key,ma_ho_so,field_path,revision_id,source_time,created_at) VALUES(?1,?2,?3,?4,?5,datetime('now')) ON CONFLICT DO UPDATE SET revision_id=excluded.revision_id,source_time=excluded.source_time,created_at=excluded.created_at",
            params![DEVICE_KEY, revision.ma_ho_so, field, revision.id, revision.source_time]).map_err(|e| e.to_string())?;
    }
    let changed = tx.execute("UPDATE hdr9000_revisions SET status='processed',response_payload=?1,processed_at=datetime('now'),sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?2 AND sending_owner_id=?3 AND device_key=?4 AND sending_lease_until > datetime('now')",
        params![response, revision.id, owner, DEVICE_KEY]).map_err(|e| e.to_string())?;
    if changed != 1 { return Err("Không thể hoàn tất revision HDR-9000: lease không còn hợp lệ.".into()); }
    tx.commit().map_err(|e| e.to_string())
}

async fn token(db: &AppDb, state: &Hdr9000ProcessState) -> Result<String, String> {
    if let Some(token) = his_api::get_access_token(db)? { return Ok(token); }
    let _guard = state.token_lock.lock().await;
    if let Some(token) = his_api::get_access_token(db)? { return Ok(token); }
    his_api::login_and_store(db).await?;
    his_api::get_access_token(db)?.ok_or_else(|| "Login HIS không trả access_token.".to_string())
}

async fn resolve_service_id(db: &AppDb, state: &Hdr9000ProcessState, client: &Client, settings: &AppSettings, ma_ho_so: &str) -> Result<i64, String> {
    if let Some(id) = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row("SELECT dv_kham_id FROM hdr9000_service_cache WHERE device_key=?1 AND ma_ho_so=?2", params![DEVICE_KEY, ma_ho_so], |row| row.get(0)).optional().map_err(|e| e.to_string())? { return Ok(id); }
    let auth = token(db, state).await?;
    let patient_url = his_api::join_url(&settings.his_api_url, PATIENT_PATH);
    let patient_response = client.get(&patient_url).bearer_auth(&auth).query(&[("maHoSo", ma_ho_so), ("page", "0"), ("size", "50")]).send().await
        .map_err(|e| format!("send_error: Gọi API người bệnh thất bại: {e}"))?;
    let patient_body = read_success_body(patient_response, "API người bệnh").await?;
    let patients: Value = serde_json::from_str(&patient_body).map_err(|e| format!("patient_not_found: Response người bệnh không hợp lệ: {e}"))?;
    let nb_id = patients.pointer("/data").and_then(Value::as_array).and_then(|rows| rows.iter().find(|row| row.get("maHoSo").and_then(Value::as_str).map(|value| value.trim().eq_ignore_ascii_case(ma_ho_so)).unwrap_or(false)))
        .and_then(|row| row.get("nbDotDieuTriId")).and_then(Value::as_i64).ok_or_else(|| "patient_not_found: Không tìm thấy hồ sơ hoặc đợt điều trị.".to_string())?;
    let summary_url = format!("{}/{}", his_api::join_url(&settings.his_api_url, SUMMARY_PATH), nb_id);
    let summary_response = client.get(&summary_url).bearer_auth(&auth).query(&[("dsCoSoKcbId", settings.ds_co_so_kcb_id.to_string())]).send().await
        .map_err(|e| format!("send_error: Gọi tổng hợp đợt điều trị thất bại: {e}"))?;
    let body = read_success_body(summary_response, "API tổng hợp đợt điều trị").await?;
    let summary: Value = serde_json::from_str(&body).map_err(|e| format!("service_not_found: Response tổng hợp không hợp lệ: {e}"))?;
    let id = summary.pointer("/data/dsDvKham").and_then(Value::as_array).and_then(|rows| rows.first()).and_then(|row| row.get("id")).and_then(Value::as_i64)
        .ok_or_else(|| "service_not_found: dsDvKham rỗng.".to_string())?;
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute("INSERT INTO hdr9000_service_cache(device_key,ma_ho_so,dv_kham_id,updated_at) VALUES(?1,?2,?3,datetime('now')) ON CONFLICT DO UPDATE SET dv_kham_id=excluded.dv_kham_id,updated_at=excluded.updated_at", params![DEVICE_KEY, ma_ho_so, id]).map_err(|e| e.to_string())?;
    Ok(id)
}

async fn read_success_body(response: reqwest::Response, endpoint: &str) -> Result<String, String> {
    let status = response.status();
    let body = response.text().await.map_err(|e| format!("send_error: Đọc response {endpoint}: {e}"))?;
    if status.is_success() {
        return Ok(body);
    }
    let preview: String = body.chars().take(500).collect();
    app_logger::error("hdr9000", &format!("{endpoint} trả về HTTP {status}: {preview}"));
    Err(format!("send_error: {endpoint} trả về {status}: {preview}"))
}

fn clear_service_cache(db: &AppDb, ma_ho_so: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute("DELETE FROM hdr9000_service_cache WHERE device_key=?1 AND ma_ho_so=?2", params![DEVICE_KEY, ma_ho_so]).map(|_| ()).map_err(|e| e.to_string())
}

fn save_response(db: &AppDb, id: i64, response: &str, owner: &str) -> Result<(), String> {
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET response_payload=?1,updated_at=datetime('now') WHERE id=?2 AND sending_owner_id=?3 AND device_key=?4 AND sending_lease_until > datetime('now')",
        params![response, id, owner, DEVICE_KEY],
    ).map_err(|e| e.to_string())?;
    if changed == 1 { Ok(()) } else { Err("Revision HDR-9000 không còn lease hợp lệ khi lưu response HIS.".into()) }
}

async fn send_update(db: &AppDb, state: &Hdr9000ProcessState, client: &Client, settings: &AppSettings, dv_kham_id: i64, payload: &Value, id: i64) -> Result<String, String> {
    let url = format!("{}/{}", his_api::join_url(&settings.his_api_url, UPDATE_PATH), dv_kham_id);
    app_logger::info("hdr9000", &format!("revision_id={id} dv_kham_id={dv_kham_id} payload={payload}"));
    let auth = token(db, state).await?;
    let response = client.put(&url).bearer_auth(auth).json(payload).send().await.map_err(|e| format!("send_error: Gửi HIS thất bại: {e}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| format!("send_error: Đọc HIS response: {e}"))?;
    app_logger::info("hdr9000", &format!("revision_id={id} HIS status={status} response={}", body.chars().take(16000).collect::<String>()));
    if status.is_success() { return Ok(body); }
    // Giữ lease để nhánh invalid-service có thể resolve lại một lần và persist kết quả cuối.
    save_response(db, id, &body, &state.instance_id)?;
    Err(format!("send_error: HIS trả về {status}: {}", body.chars().take(500).collect::<String>()))
}

fn is_invalid_service(error: &str) -> bool {
    error.contains("404") || error.to_ascii_lowercase().contains("không còn hợp lệ") || error.to_ascii_lowercase().contains("invalid service")
}

fn emit_file(app: &AppHandle, db: &AppDb, id: i64) {
    let record = db.conn.lock().ok().and_then(|conn| conn.query_row("SELECT id,file_name,file_path,file_size,file_modified_at,status,error_message,filter_date,updated_at FROM hdr9000_revisions WHERE id=?1 AND device_key=?2", params![id, DEVICE_KEY], |row| Ok(TrackedXmlFile {
        id: row.get(0)?, device_key: DEVICE_KEY.into(), file_name: row.get(1)?, file_path: row.get(2)?, file_size: row.get(3)?, file_modified_at: row.get(4)?,
        status: XmlFileStatus::parse(&row.get::<_, String>(5)?), error_message: row.get(6)?, created_at: row.get(7)?, updated_at: row.get(8)?,
    })).optional().ok().flatten());
    if let Some(record) = record { let _ = app.emit(FILE_PROGRESS_EVENT, record); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory_for_test;

    const SAMPLE: &str = "<Root><Product_Model> HDR-9000 </Product_Model><Patient_ID>P260728-0188</Patient_ID><Measurement_Date>20260728</Measurement_Date><Final_Prescription_Data_FAR_Sph-Right>-2.50</Final_Prescription_Data_FAR_Sph-Right><Final_Prescription_Data_FAR_Cyl-Right>-0.75</Final_Prescription_Data_FAR_Cyl-Right><Final_Prescription_Data_FAR_Axis-Right>6</Final_Prescription_Data_FAR_Axis-Right><Final_Prescription_Data_FAR_Sph-Left>-2.25</Final_Prescription_Data_FAR_Sph-Left><Final_Prescription_Data_FAR_Cyl-Left>-0.50</Final_Prescription_Data_FAR_Cyl-Left><Final_Prescription_Data_FAR_Axis-Left>19</Final_Prescription_Data_FAR_Axis-Left><Far_PD_OU>61.0</Far_PD_OU><Near_PD_OU>57.5</Near_PD_OU></Root>";

    #[test]
    fn parses_expected_sparse_payload() {
        let parsed = parse_hdr9000_xml(SAMPLE.as_bytes(), "0188.xml").unwrap();
        assert_eq!(parsed.ma_ho_so, "0188");
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiKinhMoi":{"sphId":"-2.50","cylId":"-0.75","axId":"6"},"matTraiKinhMoi":{"sphId":"-2.25","cylId":"-0.50","axId":"19"},"dongTuXa":"61.0","dongTuGan":"57.5"}));
        assert_eq!(mapped_payload(&parsed.payload).unwrap(), serde_json::json!({"matPhaiKinhMoi":{"sphId":1100,"cylId":1335,"axId":1540},"matTraiKinhMoi":{"sphId":1099,"cylId":1334,"axId":1553},"dongTuXa":"61.0","dongTuGan":"57.5"}));
    }

    #[test]
    fn visual_acuity_keeps_the_exact_xml_text() {
        let xml = b"<x><Product_Model>HDR-9000</Product_Model><Final_Prescription_Data_FAR_VA-Right>20/200</Final_Prescription_Data_FAR_VA-Right><Final_Prescription_Data_FAR_VA-Left>1/10</Final_Prescription_Data_FAR_VA-Left><Final_Prescription_Data_NEAR_VA-Right>ST(+)</Final_Prescription_Data_NEAR_VA-Right></x>";
        let parsed = parse_hdr9000_xml(xml, "0188.xml").unwrap();
        assert_eq!(mapped_payload(&parsed.payload).unwrap(), serde_json::json!({"matPhaiKinhMoi":{"thiLucId":852},"matTraiKinhMoi":{"thiLucId":1902},"matPhaiCapKinhNhinGan":{"thiLucId":893}}));
    }

    #[test]
    fn rejects_other_model_and_omits_empty_objects() {
        assert!(matches!(parse_hdr9000_xml(b"<x><Product_Model>KR-800</Product_Model></x>", "a.xml"), Err(Hdr9000ParseError::WrongModel(_))));
        let parsed = parse_hdr9000_xml(b"<x><Product_Model>HDR-9000</Product_Model></x>", "a.xml").unwrap();
        assert_eq!(parsed.payload, serde_json::json!({}));
    }

    #[test]
    fn later_file_serializes_only_its_populated_fields() {
        let xml = b"<x><Product_Model>HDR-9000</Product_Model><Final_Prescription_Data_FAR_ADD-Right>1.50</Final_Prescription_Data_FAR_ADD-Right><Near_PD_OU>58.0</Near_PD_OU></x>";
        let parsed = parse_hdr9000_xml(xml, "0188.xml").unwrap();
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiCapKinhNhinGan":{"donViAddId":"1.50"},"dongTuGan":"58.0"}));
        assert_eq!(mapped_payload(&parsed.payload).unwrap(), serde_json::json!({"matPhaiCapKinhNhinGan":{"donViAddId":6},"dongTuGan":"58.0"}));
        assert!(parsed.payload.get("matPhaiKinhMoi").is_none());
        assert!(!parsed.payload.to_string().contains("null"));
    }

    #[test]
    fn measurement_date_falls_back_correctly() {
        let date = resolve_dates(None, Some(NaiveDate::from_ymd_opt(2026, 7, 28).unwrap()), None, Local::now());
        assert_eq!(date.date_source, "xml_measurement_date");
        let date = resolve_dates(None, None, None, Local::now());
        assert_eq!(date.date_source, "discovered_at");
    }

    #[test]
    fn invalid_measurement_date_uses_modified_time() {
        let date = resolve_dates(None, None, Some(SystemTime::now()), Local::now());
        assert_eq!(date.date_source, "filesystem_modified");
    }

    fn test_revision(db: &AppDb, id: i64, hash: &str, source_time: &str, payload: &str) {
        db.conn.lock().unwrap().execute("INSERT INTO hdr9000_revisions(id,file_name,file_path,content_hash,ma_ho_so,filter_date,date_source,source_time,snapshot_xml,snapshot_payload,status,discovered_at,created_at,updated_at) VALUES(?1,'0188.xml',?2,?3,'0188',?4,'discovered_at',?4,x'3c783e',?5,'waiting',datetime('now'),datetime('now'),datetime('now'))",
            params![id, "C:/in/0188.xml", hash, source_time, payload]).unwrap();
    }

    #[test]
    fn same_path_new_hash_is_revision_and_same_hash_is_idempotent() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "a", "2026-07-28 09:00:00", "{}");
        test_revision(&db, 2, "b", "2026-07-28 09:01:00", "{}");
        let duplicate = db.conn.lock().unwrap().execute("INSERT OR IGNORE INTO hdr9000_revisions(file_name,file_path,content_hash,ma_ho_so,filter_date,date_source,source_time,snapshot_xml,snapshot_payload,status,discovered_at,created_at,updated_at) VALUES('0188.xml','C:/in/0188.xml','a','0188','2026-07-28 09:00:00','discovered_at','2026-07-28 09:00:00',x'3c783e','{}','waiting',datetime('now'),datetime('now'),datetime('now'))", []).unwrap();
        assert_eq!(duplicate, 0);
    }

    #[test]
    fn same_hash_at_another_path_is_not_eligible_for_a_second_send() {
        let db = open_memory_for_test().unwrap();
        let first = db.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO hdr9000_content_hashes(device_key,content_hash,created_at) VALUES(?1,?2,datetime('now'))",
            params![DEVICE_KEY, "same-content"],
        ).unwrap();
        let duplicate = db.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO hdr9000_content_hashes(device_key,content_hash,created_at) VALUES(?1,?2,datetime('now'))",
            params![DEVICE_KEY, "same-content"],
        ).unwrap();
        assert_eq!(first, 1);
        assert_eq!(duplicate, 0);
    }

    #[test]
    fn stale_retry_keeps_only_fields_not_sent_by_newer_revision() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "old", "2026-07-28 09:00:00", r#"{"matPhaiKinhMoi":{"sphId":-2.5,"cylId":-0.75}}"#);
        test_revision(&db, 2, "new", "2026-07-28 10:00:00", r#"{"matPhaiKinhMoi":{"sphId":-2.25}}"#);
        db.conn.lock().unwrap().execute("INSERT INTO hdr9000_field_versions(ma_ho_so,field_path,revision_id,source_time,created_at) VALUES('0188','matPhaiKinhMoi.sphId',2,'2026-07-28 10:00:00',datetime('now'))", []).unwrap();
        let old = load_revision(&db, 1).unwrap().unwrap();
        let payload = serde_json::json!({"matPhaiKinhMoi":{"sphId":1100,"cylId":1335}});
        assert_eq!(stale_filtered_payload(&db, &old, &payload).unwrap(), serde_json::json!({"matPhaiKinhMoi":{"cylId":1335}}));
    }

    #[test]
    fn service_cache_is_keyed_by_ma_ho_so() {
        let db = open_memory_for_test().unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO hdr9000_service_cache(ma_ho_so,dv_kham_id,updated_at) VALUES('0188',3462,datetime('now'))", []).unwrap();
        let value: i64 = db.conn.lock().unwrap().query_row("SELECT dv_kham_id FROM hdr9000_service_cache WHERE ma_ho_so='0188'", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 3462);
    }

    #[test]
    fn failed_response_keeps_lease_for_single_service_retry_and_counts_attempts() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "retry", "2026-07-28 09:00:00", r#"{"dongTuXa":"61.0"}"#);
        assert!(claim(&db, 1, "owner").unwrap());
        save_request(&db, 1, 3462, r#"{"dongTuXa":"61.0"}"#, "owner").unwrap();
        save_response(&db, 1, "service no longer valid", "owner").unwrap();
        let row: (String, i64) = db.conn.lock().unwrap().query_row("SELECT sending_owner_id,attempt_count FROM hdr9000_revisions WHERE id=1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(row.0, "owner");
        assert_eq!(row.1, 1);
        let revision = load_revision(&db, 1).unwrap().unwrap();
        finish_success(&db, &revision, r#"{"dongTuXa":"61.0"}"#, "{}", "owner").unwrap();
        let status: String = db.conn.lock().unwrap().query_row("SELECT status FROM hdr9000_revisions WHERE id=1", [], |row| row.get(0)).unwrap();
        assert_eq!(status, "processed");
    }

    #[test]
    fn expired_processing_or_sending_revision_is_recovered_without_a_pending_row() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "stale", "2026-07-28 09:00:00", "{}");
        db.conn.lock().unwrap().execute(
            "UPDATE hdr9000_revisions SET status='sending',sending_owner_id='old-instance',sending_lease_until=datetime('now','-1 second') WHERE id=1",
            [],
        ).unwrap();
        assert_eq!(count_pending(&db).unwrap(), 0);
        recover_expired(&db).unwrap();
        let status: String = db.conn.lock().unwrap().query_row(
            "SELECT status FROM hdr9000_revisions WHERE id=1", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(status, "send_error");
        assert_eq!(count_pending(&db).unwrap(), 1);
    }

    #[test]
    fn patient_lease_excludes_another_instance_until_released() {
        let db = open_memory_for_test().unwrap();
        assert!(claim_patient_lease(&db, "0188", "instance-a").unwrap());
        assert!(!claim_patient_lease(&db, "0188", "instance-b").unwrap());
        release_patient_lease(&db, "0188", "instance-a").unwrap();
        assert!(claim_patient_lease(&db, "0188", "instance-b").unwrap());
    }

    #[test]
    fn heartbeat_renews_both_revision_and_patient_leases() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "heartbeat", "2026-07-28 09:00:00", r#"{"dongTuXa":"61.0"}"#);
        assert!(claim(&db, 1, "instance-a").unwrap());
        assert!(claim_patient_lease(&db, "0188", "instance-a").unwrap());
        assert!(renew_leases(&db, 1, "0188", "instance-a").unwrap());
        let leases: (String, String) = db.conn.lock().unwrap().query_row(
            "SELECT r.sending_lease_until,p.lease_until FROM hdr9000_revisions r JOIN hdr9000_patient_leases p ON p.device_key=r.device_key AND p.ma_ho_so=r.ma_ho_so WHERE r.id=1",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(leases.0, leases.1);
    }

    #[test]
    fn lost_or_expired_lease_cannot_transition_to_sending() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "lost", "2026-07-28 09:00:00", r#"{"dongTuXa":"61.0"}"#);
        assert!(claim(&db, 1, "instance-a").unwrap());
        assert!(claim_patient_lease(&db, "0188", "instance-a").unwrap());
        db.conn.lock().unwrap().execute_batch(
            "UPDATE hdr9000_revisions SET sending_lease_until=datetime('now','-1 second') WHERE id=1; UPDATE hdr9000_patient_leases SET lease_until=datetime('now','-1 second') WHERE ma_ho_so='0188';",
        ).unwrap();
        assert!(!renew_leases(&db, 1, "0188", "instance-a").unwrap());
        assert!(save_request(&db, 1, 3462, r#"{"dongTuXa":"61.0"}"#, "instance-a").is_err());
        let attempts: i64 = db.conn.lock().unwrap().query_row(
            "SELECT attempt_count FROM hdr9000_revisions WHERE id=1", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn index_path_snapshots_each_stable_hash_at_the_same_path() {
        let db = open_memory_for_test().unwrap();
        let path = std::env::temp_dir().join(format!(
            "hdr9000-{}-{}.xml",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        fs::write(&path, SAMPLE.as_bytes()).unwrap();
        assert!(matches!(index_path(&db, &path, Duration::ZERO).unwrap(), IndexOutcome::Inserted));
        let later = SAMPLE.replace("<Far_PD_OU>61.0</Far_PD_OU>", "<Far_PD_OU>62.0</Far_PD_OU>");
        fs::write(&path, later.as_bytes()).unwrap();
        assert!(matches!(index_path(&db, &path, Duration::ZERO).unwrap(), IndexOutcome::Inserted));
        let path_text = path.to_string_lossy().to_string();
        let revisions: i64 = db.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM hdr9000_revisions WHERE file_path=?1", params![path_text], |row| row.get(0),
        ).unwrap();
        assert_eq!(revisions, 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn index_path_rejects_the_same_hash_at_another_path() {
        let db = open_memory_for_test().unwrap();
        let suffix = format!("{}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap_or_default());
        let first = std::env::temp_dir().join(format!("0188-{suffix}.xml"));
        let second = std::env::temp_dir().join(format!("0199-{suffix}.xml"));
        fs::write(&first, SAMPLE.as_bytes()).unwrap();
        fs::write(&second, SAMPLE.as_bytes()).unwrap();
        assert!(matches!(index_path(&db, &first, Duration::ZERO).unwrap(), IndexOutcome::Inserted));
        assert!(matches!(index_path(&db, &second, Duration::ZERO).unwrap(), IndexOutcome::Duplicate));
        let revisions: i64 = db.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM hdr9000_revisions", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(revisions, 1);
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
    }
}
