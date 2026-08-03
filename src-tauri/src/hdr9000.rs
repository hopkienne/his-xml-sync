use crate::{
    app_logger,
    db::AppDb,
    his_api,
    settings::{self, AppSettings},
    xml_track::{DeviceFolderState, ScanProgress, TrackedXmlFile, XmlFileStatus},
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime};
use reqwest::{Client, StatusCode};
use roxmltree::Document;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

pub const DEVICE_KEY: &str = "hdr-9000";
pub const SCAN_PROGRESS_EVENT: &str = "hdr9000:scan-progress";
pub const FILE_PROGRESS_EVENT: &str = "hdr9000:file-progress";
const PATIENT_PATH: &str = "/api/his/v1/nb-kham-ck-mat/nguoi-benh";
const SUMMARY_PATH: &str = "/api/his/v1/nb-dot-dieu-tri/tong-hop";
const UPDATE_PATH: &str = "/api/his/v1/nb-kham-ck-mat";
const LEASE_SECONDS: i64 = 120;

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
    patient_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub instance_id: String,
}

impl Default for Hdr9000ProcessState {
    fn default() -> Self {
        Self {
            run_lock: Mutex::new(()),
            token_lock: Mutex::new(()),
            patient_locks: Mutex::new(HashMap::new()),
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

fn number(document: &Document<'_>, tag: &str) -> Option<Value> {
    tag_text(document, tag).and_then(|value| value.parse::<f64>().ok()).map(Value::from)
}

fn text(document: &Document<'_>, tag: &str) -> Option<Value> {
    tag_text(document, tag).filter(|value| !value.is_empty()).map(Value::from)
}

fn put_number(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value { map.insert(key.to_string(), value); }
}

fn put_object(root: &mut Map<String, Value>, key: &str, fields: Map<String, Value>) {
    if !fields.is_empty() { root.insert(key.to_string(), Value::Object(fields)); }
}

fn sparse_payload(document: &Document<'_>) -> Value {
    let mut root = Map::new();
    let mut right_far = Map::new();
    put_number(&mut right_far, "sphId", number(document, "Final_Prescription_Data_FAR_Sph-Right"));
    put_number(&mut right_far, "cylId", number(document, "Final_Prescription_Data_FAR_Cyl-Right"));
    put_number(&mut right_far, "axId", number(document, "Final_Prescription_Data_FAR_Axis-Right"));
    put_number(&mut right_far, "thiLucId", number(document, "Final_Prescription_Data_FAR_VA-Right"));
    put_object(&mut root, "matPhaiKinhMoi", right_far);

    let mut left_far = Map::new();
    put_number(&mut left_far, "sphId", number(document, "Final_Prescription_Data_FAR_Sph-Left"));
    put_number(&mut left_far, "cylId", number(document, "Final_Prescription_Data_FAR_Cyl-Left"));
    put_number(&mut left_far, "axId", number(document, "Final_Prescription_Data_FAR_Axis-Left"));
    put_number(&mut left_far, "thiLucId", number(document, "Final_Prescription_Data_FAR_VA-Left"));
    put_object(&mut root, "matTraiKinhMoi", left_far);

    let mut right_near = Map::new();
    put_number(&mut right_near, "donViAddId", number(document, "Final_Prescription_Data_FAR_ADD-Right"));
    put_number(&mut right_near, "sphId", number(document, "Final_Prescription_Data_NEAR_Sph-Right"));
    put_number(&mut right_near, "cylId", number(document, "Final_Prescription_Data_NEAR_Cyl-Right"));
    put_number(&mut right_near, "axId", number(document, "Final_Prescription_Data_NEAR_Axis-Right"));
    put_number(&mut right_near, "thiLucId", number(document, "Final_Prescription_Data_NEAR_VA-Right"));
    put_object(&mut root, "matPhaiCapKinhNhinGan", right_near);

    let mut left_near = Map::new();
    put_number(&mut left_near, "donViAddId", number(document, "Final_Prescription_Data_FAR_ADD-Left"));
    put_number(&mut left_near, "sphId", number(document, "Final_Prescription_Data_NEAR_Sph-Left"));
    put_number(&mut left_near, "cylId", number(document, "Final_Prescription_Data_NEAR_Cyl-Left"));
    put_number(&mut left_near, "axId", number(document, "Final_Prescription_Data_NEAR_Axis-Left"));
    put_number(&mut left_near, "thiLucId", number(document, "Final_Prescription_Data_NEAR_VA-Left"));
    put_object(&mut root, "matTraiCapKinhNhinGan", left_near);

    if let Some(value) = text(document, "Far_PD_OU") { root.insert("dongTuXa".to_string(), value); }
    if let Some(value) = text(document, "Near_PD_OU") { root.insert("dongTuGan".to_string(), value); }
    Value::Object(root)
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
    let entries = fs::read_dir(folder).map_err(|e| format!("Không đọc được thư mục HDR-9000: {e}"))?;
    let paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_file()).collect();
    let total = paths.len();
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    for (index, path) in paths.iter().enumerate() {
        if let Some(app) = app {
            let percent = if total == 0 { 100 } else { (((index + 1) * 100) / total) as u8 };
            let _ = app.emit(SCAN_PROGRESS_EVENT, ScanProgress { phase: "index".into(), current: index + 1, total, percent, message: "Đang kiểm tra XML HDR-9000…".into() });
        }
        match index_path(db, path, Duration::from_secs(2))? {
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
    let metadata = fs::metadata(path).map_err(|e| format!("Không đọc metadata XML: {e}"))?;
    if metadata.modified().ok().and_then(|time| time.elapsed().ok()).map(|age| age < min_age).unwrap_or(false) {
        return Ok(IndexOutcome::Skipped);
    }
    let bytes = fs::read(path).map_err(|e| format!("Không đọc XML HDR-9000: {e}"))?;
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
    let path_text = path.to_string_lossy().to_string();
    if content_hash_seen(db, &hash)? {
        app_logger::info("hdr9000", &format!("duplicate file={file_name} model=HDR-9000 hash={}", &hash[..12]));
        return Ok(IndexOutcome::Duplicate);
    }
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let affected = conn.execute("INSERT OR IGNORE INTO hdr9000_revisions(device_key,file_name,file_path,content_hash,ma_ho_so,patient_id,filter_date,date_source,source_time,snapshot_xml,snapshot_payload,status,discovered_at,created_at,updated_at,file_size,file_modified_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,datetime('now'),datetime('now'),datetime('now'),?13,?14)",
        params![DEVICE_KEY, file_name, path_text, hash, parsed.ma_ho_so, parsed.patient_id, dates.filter_date, dates.date_source, dates.source_time, bytes, payload, status, metadata.len() as i64, metadata.modified().ok().and_then(system_time_to_local)])
        .map_err(|e| format!("Lưu revision HDR-9000: {e}"))?;
    if affected == 0 { return Ok(IndexOutcome::Duplicate); }
    app_logger::info("hdr9000", &format!("indexed model=HDR-9000 file={} maHoSo={} patientId={:?} hash={} source_time={} date_source={}", file_name, parsed.ma_ho_so, parsed.patient_id, &hash[..12], dates.source_time, dates.date_source));
    Ok(IndexOutcome::Inserted)
}

fn content_hash_seen(db: &AppDb, hash: &str) -> Result<bool, String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM hdr9000_revisions WHERE device_key=?1 AND content_hash=?2)",
            params![DEVICE_KEY, hash],
            |row| row.get(0),
        )
        .map_err(|e| format!("Kiểm tra hash HDR-9000: {e}"))
}

pub fn list_files(db: &AppDb, from: Option<&str>, to: Option<&str>) -> Result<Vec<TrackedXmlFile>, String> {
    let (Some(from), Some(to)) = (from, to) else { return Ok(Vec::new()); };
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn.prepare("SELECT id,file_name,file_path,file_size,file_modified_at,status,error_message,filter_date,updated_at FROM hdr9000_revisions WHERE device_key=?1 AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id")
        .map_err(|e| e.to_string())?;
    statement.query_map(params![DEVICE_KEY, from, to], |row| Ok(TrackedXmlFile {
        id: row.get(0)?, device_key: DEVICE_KEY.into(), file_name: row.get(1)?, file_path: row.get(2)?,
        file_size: row.get(3)?, file_modified_at: row.get(4)?, status: XmlFileStatus::parse(&row.get::<_, String>(5)?),
        error_message: row.get(6)?, created_at: row.get(7)?, updated_at: row.get(8)?,
    })).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn folder_state(db: &AppDb) -> Result<DeviceFolderState, String> {
    crate::xml_track::get_device_folder(db, DEVICE_KEY)
}

pub fn count_pending(db: &AppDb) -> Result<usize, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.query_row("SELECT COUNT(*) FROM hdr9000_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found')", params![DEVICE_KEY], |row| row.get::<_, i64>(0))
        .map(|value| value as usize).map_err(|e| e.to_string())
}

/// Poll riêng cho HDR-9000; revision hash giúp phát hiện cả file bị ghi đè cùng path.
pub fn start_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            background_tick(&app).await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

pub async fn trigger_auto_process_now(app: &AppHandle) {
    background_tick(app).await;
}

async fn background_tick(app: &AppHandle) {
    let Some(db) = app.try_state::<AppDb>() else { return; };
    let state = match folder_state(&db) { Ok(state) => state, Err(error) => { app_logger::error("hdr9000", &error); return; } };
    let Some(folder) = state.tracking_folder else {
        let _ = app.emit("hdr9000:watch-status", serde_json::json!({"active":false,"trackingFolder":null,"message":"Chưa chọn thư mục tracking HDR-9000."}));
        return;
    };
    let _ = app.emit("hdr9000:watch-status", serde_json::json!({"active":true,"trackingFolder":folder,"message":"Đang theo dõi folder HDR-9000."}));
    let before = count_pending(&db).unwrap_or(0);
    let scan = scan_folder(None, &db, &folder);
    if let Ok(result) = scan {
        if result.inserted_count > 0 {
            let _ = app.emit("hdr9000:files-indexed", serde_json::json!({"source":"poll","insertedCount":result.inserted_count,"scannedCount":result.scanned_count,"trackingFolder":folder,"inserted":[]}));
        }
    }
    if !state.auto_process_enabled { return; }
    let pending = count_pending(&db).unwrap_or(before);
    if pending == 0 { return; }
    // Recovery không bị giới hạn hôm nay: file lỗi từ ngày cũ phải được retry sau restart.
    let Some((from, to)) = pending_range(&db).unwrap_or(None) else { return; };
    let Some(process_state) = app.try_state::<Hdr9000ProcessState>() else { return; };
    match try_process(app, &db, &process_state, &from, &to).await {
        Ok(Some(result)) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":true,"message":format!("Tự xử lý: {}/{} thành công; bỏ qua {}; lỗi {}.",result.processed,result.total,result.skipped,result.failed),"fromTime":from,"toTime":to,"total":result.total,"processed":result.processed,"failed":result.failed,"skipped":result.skipped,"busy":false})); }
        Ok(None) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":true,"message":"Pipeline HDR-9000 đang bận.","fromTime":from,"toTime":to,"total":0,"processed":0,"failed":0,"skipped":0,"busy":true})); }
        Err(error) => { let _ = app.emit("hdr9000:auto-process", serde_json::json!({"ok":false,"message":error,"fromTime":from,"toTime":to,"total":0,"processed":0,"failed":0,"skipped":0,"busy":false})); }
    }
}

pub async fn process(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, from: &str, to: &str) -> Result<Hdr9000ProcessResult, String> {
    let _guard = state.run_lock.lock().await;
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
                let status = if error.starts_with("patient_not_found:") { "patient_not_found" }
                    else if error.starts_with("service_not_found:") { "service_not_found" }
                    else if error.starts_with("xml_error:") { "xml_error" } else { "send_error" };
                let _ = fail(db, id, status, &error, &state.instance_id);
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
    let guard = match state.run_lock.try_lock() { Ok(guard) => guard, Err(_) => return Ok(None) };
    drop(guard);
    process(app, db, state, from, to).await.map(Some)
}

enum ProcessOne { Processed, Skipped }

async fn process_one(app: &AppHandle, db: &AppDb, state: &Hdr9000ProcessState, client: &Client, settings: &AppSettings, id: i64) -> Result<ProcessOne, String> {
    if !claim(db, id, &state.instance_id)? { return Ok(ProcessOne::Skipped); }
    let revision = load_revision(db, id)?.ok_or_else(|| "Revision không tồn tại.".to_string())?;
    let parsed = parse_hdr9000_xml(&revision.xml, &revision.file_name).map_err(|e| {
        fail(db, id, "xml_error", &e.to_string(), &state.instance_id).ok(); e.to_string()
    })?;
    let lock = {
        let mut locks = state.patient_locks.lock().await;
        Arc::clone(locks.entry(revision.ma_ho_so.clone()).or_insert_with(|| Arc::new(Mutex::new(()))))
    };
    let _patient = lock.lock().await;
    let payload = stale_filtered_payload(db, &revision)?;
    if payload.as_object().map(|value| value.is_empty()).unwrap_or(true) {
        set_status(db, id, "superseded", None, &state.instance_id)?;
        emit_file(app, db, id);
        return Ok(ProcessOne::Skipped);
    }
    let dv_kham_id = resolve_service_id(db, state, client, settings, &revision.ma_ho_so).await?;
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    save_request(db, id, dv_kham_id, &body, &state.instance_id)?;
    let response = match send_update(db, state, client, settings, dv_kham_id, &payload, id).await {
        Ok(response) => response,
        Err(error) => {
            if is_invalid_service(&error) {
                clear_service_cache(db, &revision.ma_ho_so)?;
                let fresh_id = resolve_service_id(db, state, client, settings, &revision.ma_ho_so).await?;
                save_request(db, id, fresh_id, &body, &state.instance_id)?;
                send_update(db, state, client, settings, fresh_id, &payload, id).await?
            } else { return Err(error); }
        }
    };
    finish_success(db, &revision, &body, &response, &state.instance_id)?;
    let _ = parsed;
    emit_file(app, db, id);
    Ok(ProcessOne::Processed)
}

struct Revision { id: i64, file_name: String, ma_ho_so: String, source_time: String, snapshot_payload: String, xml: Vec<u8> }

fn load_revision(db: &AppDb, id: i64) -> Result<Option<Revision>, String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row("SELECT id,file_name,ma_ho_so,source_time,snapshot_payload,snapshot_xml FROM hdr9000_revisions WHERE id=?1 AND device_key=?2", params![id, DEVICE_KEY],
            |row| Ok(Revision { id: row.get(0)?, file_name: row.get(1)?, ma_ho_so: row.get(2)?, source_time: row.get(3)?, snapshot_payload: row.get(4)?, xml: row.get(5)? }))
        .optional().map_err(|e| e.to_string())
}

fn retryable_ids(db: &AppDb, from: &str, to: &str) -> Result<Vec<i64>, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn.prepare("SELECT id FROM hdr9000_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found') AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id").map_err(|e| e.to_string())?;
    statement.query_map(params![DEVICE_KEY, from, to], |row| row.get(0)).map_err(|e| e.to_string())?.collect::<Result<Vec<i64>, _>>().map_err(|e| e.to_string())
}

fn claim(db: &AppDb, id: i64, owner: &str) -> Result<bool, String> {
    let changed = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='processing',error_message=NULL,sending_started_at=datetime('now'),sending_owner_id=?1,sending_lease_until=datetime('now','+120 seconds'),updated_at=datetime('now') WHERE id=?2 AND device_key=?3 AND status IN ('waiting','send_error','patient_not_found','service_not_found')",
        params![owner, id, DEVICE_KEY]).map_err(|e| e.to_string())?;
    Ok(changed == 1)
}

fn save_request(db: &AppDb, id: i64, dv_kham_id: i64, body: &str, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status='sending',dv_kham_id=?1,request_payload=?2,attempt_count=attempt_count+1,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5",
        params![dv_kham_id, body, id, owner, DEVICE_KEY]).map(|_| ()).map_err(|e| e.to_string())
}

fn fail(db: &AppDb, id: i64, status: &str, message: &str, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status=?1,error_message=?2,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5",
        params![status, message, id, owner, DEVICE_KEY]).map(|_| ()).map_err(|e| e.to_string())
}

fn set_status(db: &AppDb, id: i64, status: &str, message: Option<&str>, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET status=?1,error_message=?2,sending_owner_id=NULL,sending_lease_until=NULL,processed_at=CASE WHEN ?1='superseded' THEN datetime('now') ELSE processed_at END,updated_at=datetime('now') WHERE id=?3 AND sending_owner_id=?4 AND device_key=?5",
        params![status, message, id, owner, DEVICE_KEY]).map(|_| ()).map_err(|e| e.to_string())
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

fn stale_filtered_payload(db: &AppDb, revision: &Revision) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(&revision.snapshot_payload).map_err(|e| format!("Snapshot JSON lỗi: {e}"))?;
    Ok(remove_stale(&payload, "", revision, db)?.unwrap_or_else(|| Value::Object(Map::new())))
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
    let changed = tx.execute("UPDATE hdr9000_revisions SET status='processed',response_payload=?1,processed_at=datetime('now'),sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?2 AND sending_owner_id=?3 AND device_key=?4",
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
    let patient_body = client.get(&patient_url).bearer_auth(&auth).query(&[("maHoSo", ma_ho_so), ("page", "0"), ("size", "50")]).send().await
        .map_err(|e| format!("patient_not_found: Gọi API người bệnh thất bại: {e}"))?.text().await.map_err(|e| e.to_string())?;
    let patients: Value = serde_json::from_str(&patient_body).map_err(|e| format!("patient_not_found: Response người bệnh không hợp lệ: {e}"))?;
    let nb_id = patients.pointer("/data").and_then(Value::as_array).and_then(|rows| rows.iter().find(|row| row.get("maHoSo").and_then(Value::as_str).map(|value| value.trim().eq_ignore_ascii_case(ma_ho_so)).unwrap_or(false)))
        .and_then(|row| row.get("nbDotDieuTriId")).and_then(Value::as_i64).ok_or_else(|| "patient_not_found: Không tìm thấy hồ sơ hoặc đợt điều trị.".to_string())?;
    let summary_url = his_api::join_url(&settings.his_api_url, SUMMARY_PATH);
    let body = client.get(&summary_url).bearer_auth(&auth).query(&[("nbThongTinId", nb_id.to_string()), ("page", "0".into()), ("size", "500".into()), ("active", "true".into()), ("dsCoSoKcbId", settings.ds_co_so_kcb_id.to_string())]).send().await
        .map_err(|e| format!("service_not_found: Gọi tổng hợp đợt điều trị thất bại: {e}"))?.text().await.map_err(|e| e.to_string())?;
    let summary: Value = serde_json::from_str(&body).map_err(|e| format!("service_not_found: Response tổng hợp không hợp lệ: {e}"))?;
    let id = summary.pointer("/data/dsDvKham").and_then(Value::as_array).and_then(|rows| rows.first()).and_then(|row| row.get("id")).and_then(Value::as_i64)
        .ok_or_else(|| "service_not_found: dsDvKham rỗng.".to_string())?;
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute("INSERT INTO hdr9000_service_cache(device_key,ma_ho_so,dv_kham_id,updated_at) VALUES(?1,?2,?3,datetime('now')) ON CONFLICT DO UPDATE SET dv_kham_id=excluded.dv_kham_id,updated_at=excluded.updated_at", params![DEVICE_KEY, ma_ho_so, id]).map_err(|e| e.to_string())?;
    Ok(id)
}

fn clear_service_cache(db: &AppDb, ma_ho_so: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute("DELETE FROM hdr9000_service_cache WHERE device_key=?1 AND ma_ho_so=?2", params![DEVICE_KEY, ma_ho_so]).map(|_| ()).map_err(|e| e.to_string())
}

fn save_response(db: &AppDb, id: i64, response: &str, owner: &str) -> Result<(), String> {
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute(
        "UPDATE hdr9000_revisions SET response_payload=?1,updated_at=datetime('now') WHERE id=?2 AND sending_owner_id=?3 AND device_key=?4",
        params![response, id, owner, DEVICE_KEY],
    ).map(|_| ()).map_err(|e| e.to_string())
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
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiKinhMoi":{"sphId":-2.5,"cylId":-0.75,"axId":6},"matTraiKinhMoi":{"sphId":-2.25,"cylId":-0.5,"axId":19},"dongTuXa":"61.0","dongTuGan":"57.5"}));
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
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiCapKinhNhinGan":{"donViAddId":1.5},"dongTuGan":"58.0"}));
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
        test_revision(&db, 1, "same-content", "2026-07-28 09:00:00", "{}");
        assert!(content_hash_seen(&db, "same-content").unwrap());
        assert!(!content_hash_seen(&db, "different-content").unwrap());
    }

    #[test]
    fn stale_retry_keeps_only_fields_not_sent_by_newer_revision() {
        let db = open_memory_for_test().unwrap();
        test_revision(&db, 1, "old", "2026-07-28 09:00:00", r#"{"matPhaiKinhMoi":{"sphId":-2.5,"cylId":-0.75}}"#);
        test_revision(&db, 2, "new", "2026-07-28 10:00:00", r#"{"matPhaiKinhMoi":{"sphId":-2.25}}"#);
        db.conn.lock().unwrap().execute("INSERT INTO hdr9000_field_versions(ma_ho_so,field_path,revision_id,source_time,created_at) VALUES('0188','matPhaiKinhMoi.sphId',2,'2026-07-28 10:00:00',datetime('now'))", []).unwrap();
        let old = load_revision(&db, 1).unwrap().unwrap();
        assert_eq!(stale_filtered_payload(&db, &old).unwrap(), serde_json::json!({"matPhaiKinhMoi":{"cylId":-0.75}}));
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
}
