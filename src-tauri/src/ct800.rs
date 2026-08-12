//! CT-800: each XML is an independent pressure revision.  This module is
//! deliberately separate from KR-800 pairing and from HDR-9000 revisions.

use crate::{
    app_logger,
    db::AppDb,
    his_api,
    settings::{self, AppSettings},
    xml_track::{DeviceFolderState, ScanProgress, TrackedXmlFile, XmlFileStatus},
};
use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::{Client, StatusCode};
use roxmltree::{Document, Node};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

pub const DEVICE_KEY: &str = "ct-800";
pub const SCAN_PROGRESS_EVENT: &str = "ct800:scan-progress";
pub const FILE_PROGRESS_EVENT: &str = "ct800:file-progress";
const PATIENT_PATH: &str = "/api/his/v1/nb-kham-ck-mat/nguoi-benh";
const SUMMARY_PATH: &str = "/api/his/v1/nb-dot-dieu-tri/tong-hop";
const UPDATE_PATH: &str = "/api/his/v1/nb-kham-ck-mat";
const LEASE_SECONDS: i64 = 120;
const LEASE_HEARTBEAT_SECONDS: u64 = 30;
const MIN_FILE_AGE: Duration = Duration::from_secs(2);
const HTTP_ATTEMPTS: usize = 3;

pub struct Ct800ProcessState {
    run_lock: Mutex<()>,
    token_lock: Mutex<()>,
    pub instance_id: String,
}
impl Default for Ct800ProcessState {
    fn default() -> Self {
        Self {
            run_lock: Mutex::new(()),
            token_lock: Mutex::new(()),
            instance_id: format!(
                "ct800-{}-{}",
                std::process::id(),
                Local::now().timestamp_nanos_opt().unwrap_or_default()
            ),
        }
    }
}
impl Ct800ProcessState {
    fn owner(&self) -> String {
        self.instance_id.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ct800ProcessResult {
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub files: Vec<TrackedXmlFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ct800RevisionDetail {
    pub id: i64,
    pub file_name: String,
    pub ma_ho_so: Option<String>,
    pub source_time: Option<String>,
    pub xml_time: Option<String>,
    pub machine_serial: Option<String>,
    pub xml_model: Option<String>,
    pub content_hash: String,
    pub raw_right_iop: Option<String>,
    pub raw_left_iop: Option<String>,
    pub right_iop_id: Option<i64>,
    pub left_iop_id: Option<i64>,
    pub dv_kham_id: Option<i64>,
    pub request_payload: Option<String>,
    pub response_payload: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub attempt_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ct800FileName {
    pub ma_ho_so: String,
    pub source_time: String,
    pub machine_serial: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCt800 {
    pub file: Ct800FileName,
    pub xml_time: Option<String>,
    pub right_raw: Option<String>,
    pub left_raw: Option<String>,
    pub right_id: Option<i64>,
    pub left_id: Option<i64>,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ct800ParseError {
    InvalidFilename(String),
    Xml(String),
    WrongModel(String),
    MissingTmMeasure,
    InvalidPressure(String),
    Mapping(String),
}
impl std::fmt::Display for Ct800ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFilename(v) => write!(f, "invalid_filename: {v}"),
            Self::Xml(v) => write!(f, "xml_error: {v}"),
            Self::WrongModel(v) => {
                write!(f, "xml_error: ModelName không phải CT-800 ({})", v.trim())
            }
            Self::MissingTmMeasure => write!(f, "xml_error: thiếu Measure type=TM"),
            Self::InvalidPressure(v) => write!(f, "xml_error: IOP không hợp lệ ({v})"),
            Self::Mapping(v) => write!(f, "mapping_error: không có danh mục Nhãn áp cho {v}"),
        }
    }
}

#[derive(Deserialize)]
struct PressureItem {
    id: i64,
    name: String,
    #[serde(default)]
    kind: String,
}

/// Equivalent to the required end-anchored pattern.  Splitting from the
/// suffix means underscores in `maHoSo` can never be interpreted as fields.
pub fn parse_filename(file_name: &str) -> Result<Ct800FileName, Ct800ParseError> {
    let stem = file_name
        .strip_suffix(".xml")
        .ok_or_else(|| Ct800ParseError::InvalidFilename("phần mở rộng phải là .xml".into()))?;
    let marker = "_TOPCON_CT-800_";
    let (prefix, serial) = stem
        .rsplit_once(marker)
        .ok_or_else(|| Ct800ParseError::InvalidFilename("không đúng mẫu TOPCON_CT-800".into()))?;
    if serial.trim().is_empty() || serial.contains('.') {
        return Err(Ct800ParseError::InvalidFilename(
            "machineSerial rỗng hoặc chứa dấu chấm".into(),
        ));
    }
    let mut pieces = prefix.rsplitn(3, '_');
    let time = pieces.next().unwrap_or_default();
    let date = pieces.next().unwrap_or_default();
    let ma_ho_so = pieces.next().unwrap_or_default().trim();
    if ma_ho_so.is_empty() {
        return Err(Ct800ParseError::InvalidFilename("maHoSo rỗng".into()));
    }
    let date = NaiveDate::parse_from_str(date, "%Y%m%d")
        .map_err(|_| Ct800ParseError::InvalidFilename("ngày không hợp lệ".into()))?;
    let time = NaiveTime::parse_from_str(time, "%H%M%S")
        .map_err(|_| Ct800ParseError::InvalidFilename("giờ không hợp lệ".into()))?;
    Ok(Ct800FileName {
        ma_ho_so: ma_ho_so.into(),
        source_time: NaiveDateTime::new(date, time)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        machine_serial: serial.trim().into(),
    })
}

fn decode_xml(bytes: &[u8]) -> String {
    let header = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
    if header.contains("shift_jis") || header.contains("shift-jis") {
        encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}
fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}
fn text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn common_node<'a, 'i>(doc: &'a Document<'i>) -> Option<Node<'a, 'i>> {
    doc.descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Common")
}

fn detected_model(bytes: &[u8]) -> Option<String> {
    let xml = decode_xml(bytes);
    let doc = Document::parse(&xml).ok()?;
    common_node(&doc)
        .and_then(|common| child(common, "ModelName"))
        .and_then(text)
}

fn canonical_iop(raw: &str) -> Result<String, Ct800ParseError> {
    canonical_decimal(raw, true)
}

fn canonical_catalogue_key(raw: &str) -> Result<String, Ct800ParseError> {
    canonical_decimal(raw, false)
}

fn canonical_decimal(raw: &str, enforce_device_range: bool) -> Result<String, Ct800ParseError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Ct800ParseError::InvalidPressure("rỗng".into()));
    }
    let negative = raw.starts_with('-');
    let unsigned = raw
        .strip_prefix('+')
        .or_else(|| raw.strip_prefix('-'))
        .unwrap_or(raw);
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Ct800ParseError::InvalidPressure(raw.into()));
    }
    let numeric = raw
        .parse::<f64>()
        .map_err(|_| Ct800ParseError::InvalidPressure(raw.into()))?;
    if !numeric.is_finite() || (enforce_device_range && !(1.0..=60.0).contains(&numeric)) {
        return Err(Ct800ParseError::InvalidPressure(raw.into()));
    }
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let fraction = fraction.trim_end_matches('0');
    let magnitude: String = if fraction.is_empty() {
        integer.into()
    } else {
        format!("{integer}.{fraction}")
    };
    Ok(if negative && magnitude != "0" {
        format!("-{magnitude}")
    } else {
        magnitude
    })
}
fn pressure_catalogue() -> Result<std::collections::HashMap<String, i64>, String> {
    let items: Vec<PressureItem> =
        serde_json::from_str(include_str!("../resources/dm_nhan_ap.json"))
            .map_err(|e| format!("Đọc dm_nhan_ap.json: {e}"))?;
    let mut map = std::collections::HashMap::new();
    for item in items {
        if !item.kind.is_empty() && item.kind != "Nhãn áp" {
            continue;
        }
        let key = canonical_catalogue_key(&item.name)
            .map_err(|_| format!("Danh mục Nhãn áp không hợp lệ: {}", item.name))?;
        if map.insert(key.clone(), item.id).is_some() {
            return Err(format!("Danh mục Nhãn áp trùng key {key}"));
        }
    }
    Ok(map)
}
fn average_iop(measure: Node<'_, '_>, eye: &str) -> Option<String> {
    child(measure, "TM")
        .and_then(|tm| child(tm, eye))
        .and_then(|eye| child(eye, "Average"))
        .and_then(|average| child(average, "IOP_mmHg"))
        .and_then(text)
}

/// Parse the immutable XML observations before catalogue lookup.  Keeping this
/// separate lets an unmapped pressure remain retryable after the catalogue is
/// corrected, without rereading a mutable file from disk.
fn parse_raw_observation(
    bytes: &[u8],
) -> Result<(Option<String>, Option<String>, Option<String>), Ct800ParseError> {
    let xml = decode_xml(bytes);
    let doc = Document::parse(&xml)
        .map_err(|e| Ct800ParseError::Xml(format!("XML không hợp lệ: {e}")))?;
    let common = common_node(&doc);
    let model = common
        .and_then(|node| child(node, "ModelName"))
        .and_then(text)
        .unwrap_or_default();
    if model != "CT-800" {
        return Err(Ct800ParseError::WrongModel(model));
    }
    let measure = doc
        .descendants()
        .find(|n| {
            n.is_element() && n.tag_name().name() == "Measure" && n.attribute("type") == Some("TM")
        })
        .ok_or(Ct800ParseError::MissingTmMeasure)?;
    let xml_date = common.and_then(|node| child(node, "Date")).and_then(text);
    let xml_clock = common.and_then(|node| child(node, "Time")).and_then(text);
    let xml_time = match (xml_date, xml_clock) {
        (Some(d), Some(t)) => NaiveDateTime::parse_from_str(&(d + " " + &t), "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string()),
        _ => None,
    };
    Ok((
        xml_time,
        average_iop(measure, "R"),
        average_iop(measure, "L"),
    ))
}

pub fn parse_ct800_xml(bytes: &[u8], file_name: &str) -> Result<ParsedCt800, Ct800ParseError> {
    let file = parse_filename(file_name)?;
    let (xml_time, right_raw, left_raw) = parse_raw_observation(bytes)?;
    let catalogue = pressure_catalogue().map_err(Ct800ParseError::Mapping)?;
    let map_eye = |value: &Option<String>| -> Result<Option<i64>, Ct800ParseError> {
        match value {
            None => Ok(None),
            Some(raw) => {
                let key = canonical_iop(raw)?;
                catalogue
                    .get(&key)
                    .copied()
                    .ok_or(Ct800ParseError::Mapping(raw.clone()))
                    .map(Some)
            }
        }
    };
    let right_id = map_eye(&right_raw)?;
    let left_id = map_eye(&left_raw)?;
    let mut payload = Map::new();
    if let Some(id) = right_id {
        payload.insert("matPhaiNhanApId".into(), Value::from(id));
    }
    if let Some(id) = left_id {
        payload.insert("matTraiNhanApId".into(), Value::from(id));
    }
    Ok(ParsedCt800 {
        file,
        xml_time,
        right_raw,
        left_raw,
        right_id,
        left_id,
        payload: Value::Object(payload),
    })
}

fn parsed_status(parsed: &ParsedCt800) -> &'static str {
    if parsed
        .payload
        .as_object()
        .is_some_and(|value| value.is_empty())
    {
        "no_supported_data"
    } else {
        "waiting"
    }
}

pub fn set_tracking_folder_and_scan(
    app: Option<&AppHandle>,
    db: &AppDb,
    folder: &str,
) -> Result<crate::xml_track::ScanResult, String> {
    let folder = folder.trim();
    if folder.is_empty() || !Path::new(folder).is_dir() {
        return Err("Thư mục tracking CT-800 không tồn tại.".into());
    }
    db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?.execute("INSERT INTO device_config(device_key,tracking_folder,auto_process_enabled,updated_at) VALUES(?1,?2,0,datetime('now')) ON CONFLICT(device_key) DO UPDATE SET tracking_folder=excluded.tracking_folder,updated_at=datetime('now')", params![DEVICE_KEY, folder]).map_err(|e| e.to_string())?;
    scan_folder(app, db, folder)
}
pub fn rescan_tracking_folder(
    app: Option<&AppHandle>,
    db: &AppDb,
) -> Result<crate::xml_track::ScanResult, String> {
    let folder = folder_state(db)?
        .tracking_folder
        .ok_or_else(|| "Chưa chọn thư mục tracking CT-800.".to_string())?;
    scan_folder(app, db, &folder)
}
pub fn folder_state(db: &AppDb) -> Result<DeviceFolderState, String> {
    crate::xml_track::get_device_folder(db, DEVICE_KEY)
}

fn scan_folder(
    app: Option<&AppHandle>,
    db: &AppDb,
    folder: &str,
) -> Result<crate::xml_track::ScanResult, String> {
    let entries = fs::read_dir(folder).map_err(|e| format!("Không đọc được folder CT-800: {e}"))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                app_logger::warn(
                    "ct800",
                    &format!("Không đọc được entry trong {folder}: {error}"),
                );
                continue;
            }
        };
        let is_file = match entry.file_type() {
            Ok(file_type) => file_type.is_file(),
            Err(error) => {
                app_logger::warn(
                    "ct800",
                    &format!(
                        "Không đọc được loại file {}: {error}",
                        entry.path().display()
                    ),
                );
                continue;
            }
        };
        let path = entry.path();
        if is_file
            && path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("xml"))
                .unwrap_or(false)
        {
            paths.push(path);
        }
    }
    let known_metadata = load_known_metadata(db)?;
    let total = paths.len();
    let mut inserted = 0;
    let mut skipped = 0;
    for (index, path) in paths.iter().enumerate() {
        if let Some(app) = app.filter(|_| index + 1 == total || (index + 1) % 20 == 0) {
            let _ = app.emit(
                SCAN_PROGRESS_EVENT,
                ScanProgress {
                    phase: "index".into(),
                    current: index + 1,
                    total,
                    percent: if total == 0 {
                        100
                    } else {
                        (((index + 1) * 100) / total) as u8
                    },
                    message: "Đang kiểm tra XML CT-800…".into(),
                },
            );
        }
        match metadata_is_known(&known_metadata, path) {
            Ok(true) => {
                skipped += 1;
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                skipped += 1;
                app_logger::warn(
                    "ct800",
                    &format!("Bỏ qua file biến mất {}: {error}", path.display()),
                );
                continue;
            }
        }
        match index_path(db, path) {
            Ok(true) => inserted += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                skipped += 1;
                app_logger::warn("ct800", &format!("index file={} error={e}", path.display()));
            }
        }
    }
    let tracked_count: usize = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row(
            "SELECT COUNT(*) FROM ct800_revisions WHERE device_key=?1",
            params![DEVICE_KEY],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v as usize)
        .map_err(|e| e.to_string())?;
    if let Some(app) = app {
        let _ = app.emit(
            SCAN_PROGRESS_EVENT,
            ScanProgress {
                phase: "done".into(),
                current: total,
                total,
                percent: 100,
                message: format!("CT-800: thêm {inserted}, bỏ qua {skipped}."),
            },
        );
    }
    Ok(crate::xml_track::ScanResult {
        tracking_folder: folder.into(),
        scanned_count: total,
        inserted_count: inserted,
        updated_count: 0,
        pruned_count: 0,
        prune_skipped: false,
        tracked_count,
    })
}

fn file_modified_text(metadata: &fs::Metadata) -> Option<String> {
    metadata.modified().ok().map(|value| {
        let value: chrono::DateTime<Local> = value.into();
        value.format("%Y-%m-%d %H:%M:%S%.9f").to_string()
    })
}

fn legacy_file_modified_text(metadata: &fs::Metadata) -> Option<String> {
    metadata.modified().ok().map(|value| {
        let value: chrono::DateTime<Local> = value.into();
        value.format("%Y-%m-%d %H:%M:%S").to_string()
    })
}

fn load_known_metadata(
    db: &AppDb,
) -> Result<std::collections::HashSet<(String, i64, String)>, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn
        .prepare(
            "SELECT revision.file_path,revision.file_size,COALESCE(revision.file_modified_at,'')
             FROM ct800_revisions revision
             JOIN (
               SELECT file_path,MAX(id) AS id
               FROM ct800_revisions
               WHERE device_key=?1
               GROUP BY file_path
             ) latest ON latest.id=revision.id
             WHERE revision.file_size IS NOT NULL",
        )
        .map_err(|e| format!("Chuẩn bị metadata CT-800: {e}"))?;
    let rows = statement
        .query_map(params![DEVICE_KEY], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("Đọc metadata CT-800: {e}"))?;
    let mut result = std::collections::HashSet::new();
    for row in rows {
        result.insert(row.map_err(|e| format!("Map metadata CT-800: {e}"))?);
    }
    Ok(result)
}

fn metadata_is_known(
    known: &std::collections::HashSet<(String, i64, String)>,
    path: &Path,
) -> Result<bool, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("Không đọc metadata {}: {e}", path.display()))?;
    let path = path.to_string_lossy().to_string();
    let size = metadata.len() as i64;
    let precise = file_modified_text(&metadata).unwrap_or_default();
    let legacy = legacy_file_modified_text(&metadata).unwrap_or_default();
    Ok(known.contains(&(path.clone(), size, precise)) || known.contains(&(path, size, legacy)))
}

pub fn index_path(db: &AppDb, path: &Path) -> Result<bool, String> {
    if !path
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.eq_ignore_ascii_case("xml"))
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let modified_before = metadata.modified().ok();
    if let Some(modified) = modified_before {
        match modified.elapsed() {
            Ok(age) if age < MIN_FILE_AGE => return Ok(false),
            Err(_) => return Ok(false),
            _ => {}
        }
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let metadata_after = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() != metadata_after.len()
        || metadata.modified().ok() != metadata_after.modified().ok()
        || bytes.len() as u64 != metadata_after.len()
    {
        app_logger::info("ct800", &format!("defer unstable file={}", path.display()));
        return Ok(false);
    }
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let file_modified_at = file_modified_text(&metadata_after);
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_string();
    let xml_model = detected_model(&bytes);
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let filename = parse_filename(&file_name).ok();
    let parsed = parse_ct800_xml(&bytes, &file_name);
    let (
        mut status,
        mut error,
        ma_ho_so,
        source_time,
        xml_time,
        serial,
        right_raw,
        left_raw,
        right_id,
        left_id,
        payload,
    ) = match parsed {
        Ok(p) => (
            parsed_status(&p),
            None,
            Some(p.file.ma_ho_so),
            Some(p.file.source_time),
            p.xml_time,
            Some(p.file.machine_serial),
            p.right_raw,
            p.left_raw,
            p.right_id,
            p.left_id,
            p.payload,
        ),
        Err(e) => {
            let status = if matches!(e, Ct800ParseError::InvalidFilename(_)) {
                "invalid_filename"
            } else if matches!(e, Ct800ParseError::Mapping(_)) {
                "mapping_error"
            } else {
                "xml_error"
            };
            let observation = parse_raw_observation(&bytes).ok();
            let (xml_time, right_raw, left_raw) = observation.unwrap_or((None, None, None));
            let (ma_ho_so, source_time, serial) = match filename {
                Some(ref v) => (
                    Some(v.ma_ho_so.clone()),
                    Some(v.source_time.clone()),
                    Some(v.machine_serial.clone()),
                ),
                None => (None, None, None),
            };
            app_logger::warn("ct800", &format!("ignored CT-800 file={file_name}: {e}"));
            (
                status,
                Some(e.to_string()),
                ma_ho_so,
                source_time,
                xml_time,
                serial,
                right_raw,
                left_raw,
                None,
                None,
                Value::Object(Map::new()),
            )
        }
    };
    if let (Some(source_time), Some(xml_time)) = (&source_time, &xml_time) {
        if source_time != xml_time {
            app_logger::warn(
                "ct800",
                &format!(
                    "file={file_name} filename_time={source_time} xml_time={xml_time} (filename is authoritative)"
                ),
            );
        }
    }
    let filter_date = source_time.clone().unwrap_or_else(|| now.clone());
    let payload = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let dedupe_eligible = !matches!(status, "invalid_filename" | "xml_error");
    let mut conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("Mở transaction index CT-800: {e}"))?;
    let reserved = if dedupe_eligible {
        tx.execute(
            "INSERT OR IGNORE INTO ct800_content_hashes(device_key,content_hash,created_at) VALUES(?1,?2,datetime('now'))",
            params![DEVICE_KEY, hash],
        )
        .map_err(|e| format!("Đặt chỗ hash CT-800: {e}"))?
    } else {
        1
    };
    if dedupe_eligible && reserved == 0 {
        status = "duplicate";
        error = Some("Nội dung XML đã được index trong phạm vi CT-800.".to_string());
    }
    let changed = tx.execute("INSERT OR IGNORE INTO ct800_revisions(device_key,file_name,file_path,content_hash,ma_ho_so,source_time,xml_time,machine_serial,xml_model,raw_right_iop,raw_left_iop,right_iop_id,left_iop_id,snapshot_xml,snapshot_payload,filter_date,file_size,file_modified_at,status,error_message,discovered_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,datetime('now'),datetime('now'),datetime('now'))", params![DEVICE_KEY,file_name,path.to_string_lossy(),hash,ma_ho_so,source_time,xml_time,serial,xml_model,right_raw,left_raw,right_id,left_id,bytes,payload,filter_date,metadata_after.len() as i64,file_modified_at,status,error]).map_err(|e| format!("Lưu revision CT-800: {e}"))?;
    if dedupe_eligible && reserved == 1 && changed == 1 {
        let revision_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE ct800_content_hashes SET first_revision_id=?1 WHERE device_key=?2 AND content_hash=?3",
            params![revision_id, DEVICE_KEY, hash],
        )
        .map_err(|e| format!("Cập nhật hash CT-800: {e}"))?;
    }
    tx.commit()
        .map_err(|e| format!("Commit index CT-800: {e}"))?;
    app_logger::info("ct800", &format!("index file={file_name} device={} maHoSo={ma_ho_so:?} sourceTime={source_time:?} serial={serial:?} model={xml_model:?} rawRight={right_raw:?} rawLeft={left_raw:?} rightId={right_id:?} leftId={left_id:?} hash={} status={status}", DEVICE_KEY, &hash[..12]));
    Ok(changed == 1)
}

pub fn list_files(
    db: &AppDb,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<TrackedXmlFile>, String> {
    let (Some(from), Some(to)) = (from, to) else {
        return Ok(vec![]);
    };
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut stmt = conn.prepare("SELECT id,file_name,file_path,file_size,file_modified_at,status,error_message,filter_date,updated_at FROM ct800_revisions WHERE device_key=?1 AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id").map_err(|e| e.to_string())?;
    let files = stmt.query_map(params![DEVICE_KEY, from, to], |r| {
        Ok(TrackedXmlFile {
            id: r.get(0)?,
            device_key: DEVICE_KEY.into(),
            file_name: r.get(1)?,
            file_path: r.get(2)?,
            file_size: r.get(3)?,
            file_modified_at: r.get(4)?,
            status: XmlFileStatus::parse(&r.get::<_, String>(5)?),
            error_message: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        })
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string());
    files
}

pub fn revision_detail(db: &AppDb, id: i64) -> Result<Ct800RevisionDetail, String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row(
            "SELECT id,file_name,ma_ho_so,source_time,xml_time,machine_serial,xml_model,
                    content_hash,raw_right_iop,raw_left_iop,right_iop_id,left_iop_id,
                    dv_kham_id,request_payload,response_payload,status,error_message,attempt_count
             FROM ct800_revisions WHERE id=?1 AND device_key=?2",
            params![id, DEVICE_KEY],
            |row| {
                Ok(Ct800RevisionDetail {
                    id: row.get(0)?,
                    file_name: row.get(1)?,
                    ma_ho_so: row.get(2)?,
                    source_time: row.get(3)?,
                    xml_time: row.get(4)?,
                    machine_serial: row.get(5)?,
                    xml_model: row.get(6)?,
                    content_hash: row.get(7)?,
                    raw_right_iop: row.get(8)?,
                    raw_left_iop: row.get(9)?,
                    right_iop_id: row.get(10)?,
                    left_iop_id: row.get(11)?,
                    dv_kham_id: row.get(12)?,
                    request_payload: row.get(13)?,
                    response_payload: row.get(14)?,
                    status: row.get(15)?,
                    error_message: row.get(16)?,
                    attempt_count: row.get(17)?,
                })
            },
        )
        .map_err(|error| {
            if matches!(error, rusqlite::Error::QueryReturnedNoRows) {
                format!("Không tìm thấy revision CT-800 #{id}.")
            } else {
                format!("Đọc chi tiết revision CT-800 #{id}: {error}")
            }
        })
}

pub fn start_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
        let mut watched: Option<String> = None;
        let mut watcher: Option<RecommendedWatcher>;
        loop {
            let folder = configured_folder(&app);
            if folder != watched {
                watched = folder.clone();
                watcher = folder
                    .as_deref()
                    .and_then(|f| start_fs_watcher(f, tx.clone()));
                let _ = app.emit(
                    "ct800:watch-status",
                    serde_json::json!({
                        "active": watcher.is_some(),
                        "trackingFolder": folder,
                        "message": if watcher.is_some() {
                            "Đang theo dõi folder CT-800 (watcher + poll)."
                        } else {
                            "Chưa có folder hoặc không gắn được watcher; poll dự phòng sẽ chạy khi có folder."
                        }
                    }),
                );
            }
            recover_expired_for_app(&app);
            if let Some(folder) = folder {
                match scan_folder_background(&app, folder.clone()).await {
                    Ok(result) if result.inserted_count > 0 => {
                        let _ = app.emit(
                            "ct800:files-indexed",
                            serde_json::json!({
                                "source": "poll",
                                "insertedCount": result.inserted_count,
                                "scannedCount": result.scanned_count,
                                "trackingFolder": folder
                            }),
                        );
                        schedule_pending(&app);
                    }
                    Ok(_) => {}
                    Err(e) => app_logger::error("ct800", &format!("watch scan: {e}")),
                }
            }
            schedule_pending(&app);
            tokio::select! {
                Some(path) = rx.recv() => {
                    let mut paths = std::collections::HashSet::from([path]);
                    tokio::time::sleep(Duration::from_millis(2_200)).await;
                    while let Ok(path) = rx.try_recv() {
                        paths.insert(path);
                    }
                    let scanned = paths.len();
                    let mut inserted = 0usize;
                    for path in paths {
                        match index_path_background(&app, path.clone()).await {
                            Ok(true) => inserted += 1,
                            Ok(false) => {}
                            Err(e) => app_logger::error(
                                "ct800",
                                &format!("watch index {}: {e}", path.display()),
                            ),
                        }
                    }
                    if inserted > 0 {
                        let _ = app.emit(
                            "ct800:files-indexed",
                            serde_json::json!({
                                "source": "watcher",
                                "insertedCount": inserted,
                                "scannedCount": scanned,
                                "trackingFolder": configured_folder(&app).unwrap_or_default()
                            }),
                        );
                        schedule_pending(&app);
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            }
        }
    });
}

async fn scan_folder_background(
    app: &AppHandle,
    folder: String,
) -> Result<crate::xml_track::ScanResult, String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app
            .try_state::<AppDb>()
            .ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        scan_folder(None, &db, &folder)
    })
    .await
    .map_err(|e| format!("spawn_blocking scan CT-800: {e}"))?
}

async fn index_path_background(app: &AppHandle, path: PathBuf) -> Result<bool, String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app
            .try_state::<AppDb>()
            .ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        let folder = folder_state(&db)?.tracking_folder.unwrap_or_default();
        if folder.is_empty() || !path.starts_with(Path::new(&folder)) {
            return Ok(false);
        }
        index_path(&db, &path)
    })
    .await
    .map_err(|e| format!("spawn_blocking index CT-800: {e}"))?
}
fn start_fs_watcher(
    folder: &str,
    tx: mpsc::UnboundedSender<PathBuf>,
) -> Option<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(
        move |event: Result<notify::Event, notify::Error>| match event {
            Ok(event)
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
                ) =>
            {
                for path in event.paths {
                    if path
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("xml"))
                        .unwrap_or(false)
                    {
                        let _ = tx.send(path);
                    }
                }
            }
            Ok(_) => {}
            Err(e) => app_logger::error("ct800", &format!("filesystem watcher: {e}")),
        },
    )
    .ok()?;
    watcher
        .watch(Path::new(folder), RecursiveMode::NonRecursive)
        .map_err(|e| app_logger::error("ct800", &format!("Không watch CT-800 {folder}: {e}")))
        .ok()?;
    Some(watcher)
}
fn configured_folder(app: &AppHandle) -> Option<String> {
    app.try_state::<AppDb>()
        .and_then(|db| folder_state(&db).ok())
        .and_then(|s| s.tracking_folder)
        .filter(|s| !s.trim().is_empty())
}
fn recover_expired_for_app(app: &AppHandle) {
    if let Some(db) = app.try_state::<AppDb>() {
        if let Err(e) = recover_expired(&db) {
            app_logger::error("ct800", &format!("recovery: {e}"));
        }
    }
}
fn schedule_pending(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(db) = app.try_state::<AppDb>() else {
            return;
        };
        if !folder_state(&db)
            .map(|s| s.auto_process_enabled)
            .unwrap_or(false)
        {
            return;
        }
        let Some((from, to)) = pending_range(&db).ok().flatten() else {
            return;
        };
        let Some(state) = app.try_state::<Ct800ProcessState>() else {
            return;
        };
        match try_process(&app, &db, &state, &from, &to).await {
            Ok(Some(r)) => {
                let _=app.emit("ct800:auto-process",serde_json::json!({"ok":true,"message":format!("Tự xử lý: {}/{} thành công; bỏ qua {}; lỗi {}.",r.processed,r.total,r.skipped,r.failed)}));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = app.emit(
                    "ct800:auto-process",
                    serde_json::json!({"ok":false,"message":e}),
                );
            }
        }
    });
}
pub async fn trigger_auto_process_now(app: &AppHandle) {
    recover_expired_for_app(app);
    schedule_pending(app);
}

pub async fn process(
    app: &AppHandle,
    db: &AppDb,
    state: &Ct800ProcessState,
    from: &str,
    to: &str,
) -> Result<Ct800ProcessResult, String> {
    let _guard = state.run_lock.lock().await;
    process_locked(app, db, state, from, to).await
}
pub async fn try_process(
    app: &AppHandle,
    db: &AppDb,
    state: &Ct800ProcessState,
    from: &str,
    to: &str,
) -> Result<Option<Ct800ProcessResult>, String> {
    let Ok(_guard) = state.run_lock.try_lock() else {
        return Ok(None);
    };
    process_locked(app, db, state, from, to).await.map(Some)
}
fn pending_range(db: &AppDb) -> Result<Option<(String, String)>, String> {
    db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.query_row("SELECT MIN(filter_date),MAX(filter_date) FROM ct800_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found','mapping_error')",params![DEVICE_KEY],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?))).map(|(a,b)|a.zip(b)).map_err(|e|e.to_string())
}
async fn process_locked(
    app: &AppHandle,
    db: &AppDb,
    state: &Ct800ProcessState,
    from: &str,
    to: &str,
) -> Result<Ct800ProcessResult, String> {
    recover_expired(db)?;
    let ids = retryable_ids(db, from, to)?;
    let total = ids.len();
    if ids.is_empty() {
        return Ok(Ct800ProcessResult {
            total,
            processed: 0,
            failed: 0,
            skipped: 0,
            files: list_files(db, Some(from), Some(to))?,
        });
    }
    let settings = settings::load(db)?;
    if settings.his_api_url.trim().is_empty() || settings.username.trim().is_empty() {
        return Err("Chưa cấu hình API URL hoặc tài khoản HIS.".into());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut processed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    for id in ids {
        match process_one(app, db, state, &client, &settings, id).await {
            Ok(true) => processed += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                failed += 1;
                app_logger::error("ct800", &format!("revision={id} {e}"));
            }
        }
    }
    Ok(Ct800ProcessResult {
        total,
        processed,
        failed,
        skipped,
        files: list_files(db, Some(from), Some(to))?,
    })
}
fn retryable_ids(db: &AppDb, from: &str, to: &str) -> Result<Vec<i64>, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut stmt=conn.prepare("SELECT id FROM ct800_revisions WHERE device_key=?1 AND status IN ('waiting','send_error','patient_not_found','service_not_found','mapping_error') AND filter_date BETWEEN ?2 AND ?3 ORDER BY source_time,id").map_err(|e|e.to_string())?;
    let ids = stmt.query_map(params![DEVICE_KEY, from, to], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string());
    ids
}
struct Revision {
    id: i64,
    ma: String,
    time: String,
    payload: String,
    right_raw: Option<String>,
    left_raw: Option<String>,
    file_name: String,
    xml: Vec<u8>,
}
fn load_revision(db: &AppDb, id: i64) -> Result<Option<Revision>, String> {
    db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.query_row("SELECT id,ma_ho_so,source_time,snapshot_payload,raw_right_iop,raw_left_iop,file_name,snapshot_xml FROM ct800_revisions WHERE id=?1 AND device_key=?2",params![id,DEVICE_KEY],|r|Ok(Revision{id:r.get(0)?,ma:r.get(1)?,time:r.get(2)?,payload:r.get(3)?,right_raw:r.get(4)?,left_raw:r.get(5)?,file_name:r.get(6)?,xml:r.get(7)?})).optional().map_err(|e|e.to_string())
}
fn claim(db: &AppDb, id: i64, owner: &str) -> Result<bool, String> {
    Ok(db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("UPDATE ct800_revisions SET status='processing',error_message=NULL,sending_started_at=datetime('now'),sending_owner_id=?1,sending_lease_until=datetime('now','+120 seconds'),updated_at=datetime('now') WHERE id=?2 AND device_key=?3 AND status IN ('waiting','send_error','patient_not_found','service_not_found','mapping_error')",params![owner,id,DEVICE_KEY]).map_err(|e|e.to_string())?==1)
}
fn patient_lease(db: &AppDb, ma: &str, owner: &str) -> Result<bool, String> {
    let lease = format!("+{LEASE_SECONDS} seconds");
    Ok(db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("INSERT INTO ct800_patient_leases(device_key,ma_ho_so,owner_id,lease_until,updated_at) VALUES(?1,?2,?3,datetime('now',?4),datetime('now')) ON CONFLICT(device_key,ma_ho_so) DO UPDATE SET owner_id=excluded.owner_id,lease_until=excluded.lease_until,updated_at=excluded.updated_at WHERE ct800_patient_leases.lease_until<=datetime('now') OR ct800_patient_leases.owner_id=excluded.owner_id",params![DEVICE_KEY,ma,owner,lease]).map_err(|e|e.to_string())?==1)
}
fn release_patient(db: &AppDb, ma: &str, owner: &str) -> Result<(), String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .execute(
            "DELETE FROM ct800_patient_leases WHERE device_key=?1 AND ma_ho_so=?2 AND owner_id=?3",
            params![DEVICE_KEY, ma, owner],
        )
        .map(|_| ())
        .map_err(|e| format!("Nhả patient lease CT-800: {e}"))
}
fn stale_payload(db: &AppDb, r: &Revision) -> Result<Value, String> {
    let p: Value = serde_json::from_str(&r.payload).map_err(|e| e.to_string())?;
    let mut out = Map::new();
    for (field, value) in p.as_object().into_iter().flatten() {
        let latest:Option<(String,i64)>=db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.query_row("SELECT source_time,revision_id FROM ct800_field_versions WHERE device_key=?1 AND ma_ho_so=?2 AND field_path=?3",params![DEVICE_KEY,r.ma,field],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?;
        if !latest
            .map(|(time, id)| time > r.time || (time == r.time && id > r.id))
            .unwrap_or(false)
        {
            out.insert(field.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}
fn rebuild_mapping_payload(r: &Revision) -> Result<Value, String> {
    let catalogue = pressure_catalogue().map_err(|error| format!("mapping_error: {error}"))?;
    let mut payload = Map::new();
    for (raw, field) in [
        (&r.right_raw, "matPhaiNhanApId"),
        (&r.left_raw, "matTraiNhanApId"),
    ] {
        if let Some(raw) = raw {
            let key = canonical_iop(raw).map_err(|e| e.to_string())?;
            let id = catalogue
                .get(&key)
                .copied()
                .ok_or_else(|| format!("mapping_error: không có danh mục Nhãn áp cho {raw}"))?;
            payload.insert(field.into(), Value::from(id));
        }
    }
    Ok(Value::Object(payload))
}
async fn process_one(
    app: &AppHandle,
    db: &AppDb,
    state: &Ct800ProcessState,
    client: &Client,
    settings: &AppSettings,
    id: i64,
) -> Result<bool, String> {
    let owner = state.owner();
    if !claim(db, id, &owner)? {
        return Ok(false);
    }
    emit_file(app, db, id);
    let r = load_revision(db, id)?.ok_or_else(|| "Revision CT-800 không tồn tại".to_string())?;
    if let Err(e) = parse_filename(&r.file_name) {
        let message = e.to_string();
        let _ = fail(db, id, "invalid_filename", &message, &owner);
        emit_file(app, db, id);
        return Err(message);
    }
    if let Err(e) = parse_raw_observation(&r.xml) {
        let message = e.to_string();
        let _ = fail(db, id, "xml_error", &message, &owner);
        emit_file(app, db, id);
        return Err(message);
    }
    if !patient_lease(db, &r.ma, &owner)? {
        release_claim(db, id, &owner)?;
        return Ok(false);
    }
    let result = {
        let operation = async {
            let original: Value = serde_json::from_str(&r.payload).map_err(|e| e.to_string())?;
            let candidate = if original.as_object().is_some_and(|v| v.is_empty()) {
                rebuild_mapping_payload(&r)?
            } else {
                original
            };
            save_mapped_snapshot(db, id, &candidate, &owner)?;
            let temporary = Revision {
                id: r.id,
                ma: r.ma.clone(),
                time: r.time.clone(),
                payload: serde_json::to_string(&candidate).map_err(|e| e.to_string())?,
                right_raw: r.right_raw.clone(),
                left_raw: r.left_raw.clone(),
                file_name: r.file_name.clone(),
                xml: r.xml.clone(),
            };
            let payload = stale_payload(db, &temporary)?;
            if payload.as_object().is_some_and(|v| v.is_empty()) {
                finish_status(db, id, "superseded", None, &owner)?;
                return Ok(false);
            }
            let dv = resolve_service(db, state, client, settings, &r.ma).await?;
            let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
            save_request(db, id, dv, &body, &owner)?;
            emit_file(app, db, id);
            let response =
                match send_update(db, state, client, settings, dv, &payload, id, &owner).await {
                    Ok(v) => Ok(v),
                    Err(e) if invalid_service(&e) => {
                        clear_service(db, &r.ma)?;
                        let fresh = resolve_service(db, state, client, settings, &r.ma).await?;
                        save_request(db, id, fresh, &body, &owner)?;
                        send_update(db, state, client, settings, fresh, &payload, id, &owner).await
                    }
                    Err(e) => Err(e),
                }?;
            finish_success(db, &r, &body, &response, &owner)?;
            Ok(true)
        };
        tokio::pin!(operation);
        loop {
            tokio::select! {
                value = &mut operation => break value,
                _ = tokio::time::sleep(Duration::from_secs(LEASE_HEARTBEAT_SECONDS)) => {
                    match renew_leases(db, id, &r.ma, &owner) {
                        Ok(true) => {}
                        Ok(false) => break Err("send_error: Mất ownership lease CT-800 trong khi xử lý.".to_string()),
                        Err(e) => break Err(format!("send_error: Không gia hạn được lease CT-800: {e}")),
                    }
                }
            }
        }
    };
    if let Err(e) = &result {
        if let Err(save_error) = fail(db, id, status_for_error(e), e, &owner) {
            app_logger::error(
                "ct800",
                &format!("Không lưu lỗi revision={id}: {save_error}"),
            );
        }
    }
    if let Err(e) = release_patient(db, &r.ma, &owner) {
        app_logger::error(
            "ct800",
            &format!("Không nhả được patient lease revision={id}: {e}"),
        );
    }
    emit_file(app, db, id);
    result
}

fn status_for_error(error: &str) -> &'static str {
    if error.starts_with("patient_not_found:") {
        "patient_not_found"
    } else if error.starts_with("service_not_found:") {
        "service_not_found"
    } else if error.starts_with("mapping_error:") {
        "mapping_error"
    } else if error.starts_with("invalid_filename:") {
        "invalid_filename"
    } else if error.starts_with("xml_error:") {
        "xml_error"
    } else {
        "send_error"
    }
}

fn release_claim(db: &AppDb, id: i64, owner: &str) -> Result<(), String> {
    let changed = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .execute(
            "UPDATE ct800_revisions
             SET status='waiting',error_message=NULL,sending_started_at=NULL,
                 sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now')
             WHERE id=?1 AND device_key=?2 AND status='processing' AND sending_owner_id=?3",
            params![id, DEVICE_KEY, owner],
        )
        .map_err(|e| e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("Không thể nhả claim CT-800 revision={id}"))
    }
}

fn renew_leases(db: &AppDb, id: i64, ma: &str, owner: &str) -> Result<bool, String> {
    let lease = format!("+{LEASE_SECONDS} seconds");
    let mut conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let revision = tx
        .execute(
            "UPDATE ct800_revisions
             SET sending_lease_until=datetime('now',?1),updated_at=datetime('now')
             WHERE id=?2 AND device_key=?3 AND sending_owner_id=?4
               AND status IN ('processing','sending') AND sending_lease_until>datetime('now')",
            params![lease, id, DEVICE_KEY, owner],
        )
        .map_err(|e| e.to_string())?;
    let patient = tx
        .execute(
            "UPDATE ct800_patient_leases
             SET lease_until=datetime('now',?1),updated_at=datetime('now')
             WHERE device_key=?2 AND ma_ho_so=?3 AND owner_id=?4
               AND lease_until>datetime('now')",
            params![lease, DEVICE_KEY, ma, owner],
        )
        .map_err(|e| e.to_string())?;
    if revision != 1 || patient != 1 {
        tx.rollback().map_err(|e| e.to_string())?;
        return Ok(false);
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}
fn fail(db: &AppDb, id: i64, status: &str, msg: &str, owner: &str) -> Result<(), String> {
    let changed = db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("UPDATE ct800_revisions SET status=?1,error_message=?2,sending_started_at=NULL,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?3 AND device_key=?4 AND sending_owner_id=?5",params![status,msg,id,DEVICE_KEY,owner]).map_err(|e|e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "Không lưu được lỗi CT-800 revision={id}: lease đã mất"
        ))
    }
}
fn finish_status(
    db: &AppDb,
    id: i64,
    status: &str,
    msg: Option<&str>,
    owner: &str,
) -> Result<(), String> {
    let changed = db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("UPDATE ct800_revisions SET status=?1,error_message=?2,processed_at=datetime('now'),sending_started_at=NULL,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?3 AND device_key=?4 AND sending_owner_id=?5 AND sending_lease_until>datetime('now')",params![status,msg,id,DEVICE_KEY,owner]).map_err(|e|e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "Không đổi được trạng thái CT-800 revision={id}: lease đã mất"
        ))
    }
}
fn save_request(db: &AppDb, id: i64, dv: i64, payload: &str, owner: &str) -> Result<(), String> {
    let n=db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("UPDATE ct800_revisions SET status='sending',dv_kham_id=?1,request_payload=?2,attempt_count=attempt_count+1,updated_at=datetime('now') WHERE id=?3 AND device_key=?4 AND sending_owner_id=?5 AND sending_lease_until>datetime('now')",params![dv,payload,id,DEVICE_KEY,owner]).map_err(|e|e.to_string())?;
    if n == 1 {
        Ok(())
    } else {
        Err("send_error: mất lease CT-800 trước khi gửi HIS".into())
    }
}

fn save_mapped_snapshot(db: &AppDb, id: i64, payload: &Value, owner: &str) -> Result<(), String> {
    let right_id = payload.get("matPhaiNhanApId").and_then(Value::as_i64);
    let left_id = payload.get("matTraiNhanApId").and_then(Value::as_i64);
    let payload = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let changed = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .execute(
            "UPDATE ct800_revisions
             SET right_iop_id=?1,left_iop_id=?2,snapshot_payload=?3,updated_at=datetime('now')
             WHERE id=?4 AND device_key=?5 AND sending_owner_id=?6
               AND sending_lease_until>datetime('now')",
            params![right_id, left_id, payload, id, DEVICE_KEY, owner],
        )
        .map_err(|e| e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "Không lưu được mapping CT-800 revision={id}: lease đã mất"
        ))
    }
}
fn finish_success(
    db: &AppDb,
    r: &Revision,
    request: &str,
    response: &str,
    owner: &str,
) -> Result<(), String> {
    let p: Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let mut conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    if let Some(payload) = p.as_object() {
        for field in payload.keys() {
            tx.execute("INSERT INTO ct800_field_versions(device_key,ma_ho_so,field_path,revision_id,source_time,created_at) VALUES(?1,?2,?3,?4,?5,datetime('now')) ON CONFLICT(device_key,ma_ho_so,field_path) DO UPDATE SET revision_id=excluded.revision_id,source_time=excluded.source_time,created_at=excluded.created_at WHERE excluded.source_time>ct800_field_versions.source_time OR (excluded.source_time=ct800_field_versions.source_time AND excluded.revision_id>ct800_field_versions.revision_id)",params![DEVICE_KEY,r.ma,field,r.id,r.time]).map_err(|e|e.to_string())?;
        }
    }
    let n=tx.execute("UPDATE ct800_revisions SET status='processed',response_payload=?1,processed_at=datetime('now'),sending_started_at=NULL,sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE id=?2 AND device_key=?3 AND sending_owner_id=?4 AND sending_lease_until>datetime('now')",params![response,r.id,DEVICE_KEY,owner]).map_err(|e|e.to_string())?;
    if n != 1 {
        return Err("mất lease CT-800 khi hoàn tất".into());
    }
    tx.commit().map_err(|e| e.to_string())
}
fn recover_expired(db: &AppDb) -> Result<(), String> {
    db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("UPDATE ct800_revisions SET status='send_error',error_message='Recovery: lease xử lý đã hết hạn.',sending_owner_id=NULL,sending_lease_until=NULL,updated_at=datetime('now') WHERE device_key=?1 AND status IN ('processing','sending') AND (sending_lease_until IS NULL OR sending_lease_until<=datetime('now'))",params![DEVICE_KEY]).map(|_|()).map_err(|e|e.to_string())
}
async fn token(db: &AppDb, state: &Ct800ProcessState) -> Result<String, String> {
    if let Some(v) = his_api::get_access_token(db)? {
        return Ok(v);
    }
    let _g = state.token_lock.lock().await;
    if let Some(v) = his_api::get_access_token(db)? {
        return Ok(v);
    }
    his_api::login_and_store(db).await?;
    his_api::get_access_token(db)?.ok_or_else(|| "Login HIS không trả access_token.".into())
}

async fn refresh_token(
    db: &AppDb,
    state: &Ct800ProcessState,
    rejected_token: &str,
) -> Result<String, String> {
    let _guard = state.token_lock.lock().await;
    if let Some(current) = his_api::get_access_token(db)? {
        if current != rejected_token {
            return Ok(current);
        }
    }
    his_api::login_and_store(db).await?;
    his_api::get_access_token(db)?.ok_or_else(|| "Login HIS không trả access_token.".into())
}

async fn get_json_with_retry<F>(
    db: &AppDb,
    state: &Ct800ProcessState,
    endpoint: &str,
    build: F,
) -> Result<Value, String>
where
    F: Fn(&str) -> reqwest::RequestBuilder,
{
    let mut access_token = token(db, state).await?;
    let mut auth_retried = false;
    for attempt in 0..HTTP_ATTEMPTS {
        let response = match build(&access_token).send().await {
            Ok(response) => response,
            Err(error) if attempt + 1 < HTTP_ATTEMPTS => {
                app_logger::warn(
                    "ct800",
                    &format!(
                        "{endpoint} transport error attempt={}: {error}",
                        attempt + 1
                    ),
                );
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            Err(error) => {
                return Err(format!("send_error: Gọi {endpoint} thất bại: {error}"));
            }
        };
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| format!("send_error: Đọc {endpoint}: {e}"))?;
        if status == StatusCode::UNAUTHORIZED && !auth_retried {
            auth_retried = true;
            access_token = refresh_token(db, state, &access_token).await?;
            continue;
        }
        if (status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS)
            && attempt + 1 < HTTP_ATTEMPTS
        {
            app_logger::warn(
                "ct800",
                &format!(
                    "{endpoint} transient status={status} attempt={}",
                    attempt + 1
                ),
            );
            tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            continue;
        }
        if !status.is_success() {
            return Err(format!(
                "send_error: {endpoint} trả về {status}: {}",
                body.chars().take(500).collect::<String>()
            ));
        }
        return serde_json::from_str(&body)
            .map_err(|e| format!("send_error: JSON {endpoint} không hợp lệ: {e}"));
    }
    Err(format!("send_error: {endpoint} vượt quá số lần retry"))
}
async fn resolve_service(
    db: &AppDb,
    state: &Ct800ProcessState,
    client: &Client,
    settings: &AppSettings,
    ma: &str,
) -> Result<i64, String> {
    if let Some(v) = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .query_row(
            "SELECT dv_kham_id FROM ct800_service_cache WHERE device_key=?1 AND ma_ho_so=?2",
            params![DEVICE_KEY, ma],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return Ok(v);
    }
    let patient_url = his_api::join_url(&settings.his_api_url, PATIENT_PATH);
    let patients = get_json_with_retry(db, state, "API người bệnh", |access_token| {
        client.get(&patient_url).bearer_auth(access_token).query(&[
            ("maHoSo", ma),
            ("page", "0"),
            ("size", "50"),
        ])
    })
    .await?;
    let nb = patients
        .pointer("/data")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter().find(|x| {
                x.get("maHoSo")
                    .and_then(Value::as_str)
                    .map(|v| v.trim().eq_ignore_ascii_case(ma))
                    .unwrap_or(false)
            })
        })
        .and_then(|x| x.get("nbDotDieuTriId"))
        .and_then(Value::as_i64)
        .ok_or_else(|| "patient_not_found: Không tìm thấy hồ sơ hoặc đợt điều trị.".to_string())?;
    let summary_url = format!("{}/{}", his_api::join_url(&settings.his_api_url, SUMMARY_PATH), nb);
    let summary = get_json_with_retry(
        db,
        state,
        "API tổng hợp đợt điều trị",
        |access_token| {
            client.get(&summary_url).bearer_auth(access_token).query(&[
                ("dsCoSoKcbId", settings.ds_co_so_kcb_id.to_string()),
            ])
        },
    )
    .await?;
    let id = summary
        .pointer("/data/dsDvKham")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(|v| v.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| "service_not_found: dsDvKham rỗng.".to_string())?;
    db.conn.lock().map_err(|_|"Không khóa được SQLite.".to_string())?.execute("INSERT INTO ct800_service_cache(device_key,ma_ho_so,dv_kham_id,updated_at) VALUES(?1,?2,?3,datetime('now')) ON CONFLICT(device_key,ma_ho_so) DO UPDATE SET dv_kham_id=excluded.dv_kham_id,updated_at=excluded.updated_at",params![DEVICE_KEY,ma,id]).map_err(|e|e.to_string())?;
    Ok(id)
}
fn save_response(db: &AppDb, id: i64, body: &str, owner: &str) -> Result<(), String> {
    let changed = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .execute(
            "UPDATE ct800_revisions SET response_payload=?1,updated_at=datetime('now')
             WHERE id=?2 AND device_key=?3 AND sending_owner_id=?4
               AND sending_lease_until>datetime('now')",
            params![body, id, DEVICE_KEY, owner],
        )
        .map_err(|e| e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "Không lưu được response CT-800 revision={id}: lease đã mất"
        ))
    }
}

async fn send_update(
    db: &AppDb,
    state: &Ct800ProcessState,
    client: &Client,
    settings: &AppSettings,
    dv: i64,
    payload: &Value,
    id: i64,
    owner: &str,
) -> Result<String, String> {
    let url = format!(
        "{}/{}",
        his_api::join_url(&settings.his_api_url, UPDATE_PATH),
        dv
    );
    app_logger::info(
        "ct800",
        &format!("revision={id} dvKhamId={dv} endpoint={url} payload={payload}"),
    );
    let mut access_token = token(db, state).await?;
    let mut auth_retried = false;
    for attempt in 0..HTTP_ATTEMPTS {
        let response = match client
            .put(&url)
            .bearer_auth(&access_token)
            .json(payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if attempt + 1 < HTTP_ATTEMPTS => {
                app_logger::warn(
                    "ct800",
                    &format!(
                        "PUT transport error revision={id} attempt={}: {error}",
                        attempt + 1
                    ),
                );
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }
            Err(error) => return Err(format!("send_error: Gửi HIS thất bại: {error}")),
        };
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| format!("send_error: Đọc HIS response: {e}"))?;
        app_logger::info(
            "ct800",
            &format!(
                "revision={id} HIS status={status} response={}",
                body.chars().take(16_000).collect::<String>()
            ),
        );
        save_response(db, id, &body, owner)?;
        if status == StatusCode::UNAUTHORIZED && !auth_retried {
            auth_retried = true;
            access_token = refresh_token(db, state, &access_token).await?;
            continue;
        }
        if (status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS)
            && attempt + 1 < HTTP_ATTEMPTS
        {
            tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            continue;
        }
        if status.is_success() {
            return Ok(body);
        }
        return Err(format!(
            "send_error: HIS trả về {status}: {}",
            body.chars().take(500).collect::<String>()
        ));
    }
    Err("send_error: PUT HIS vượt quá số lần retry".into())
}
fn invalid_service(error: &str) -> bool {
    error.contains("404")
        || error.to_ascii_lowercase().contains("invalid service")
        || error.to_ascii_lowercase().contains("không còn hợp lệ")
}
fn clear_service(db: &AppDb, ma: &str) -> Result<(), String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?
        .execute(
            "DELETE FROM ct800_service_cache WHERE device_key=?1 AND ma_ho_so=?2",
            params![DEVICE_KEY, ma],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn emit_file(app: &AppHandle, db: &AppDb, id: i64) {
    let item=db.conn.lock().ok().and_then(|c|c.query_row("SELECT id,file_name,file_path,file_size,file_modified_at,status,error_message,filter_date,updated_at FROM ct800_revisions WHERE id=?1 AND device_key=?2",params![id,DEVICE_KEY],|r|Ok(TrackedXmlFile{id:r.get(0)?,device_key:DEVICE_KEY.into(),file_name:r.get(1)?,file_path:r.get(2)?,file_size:r.get(3)?,file_modified_at:r.get(4)?,status:XmlFileStatus::parse(&r.get::<_,String>(5)?),error_message:r.get(6)?,created_at:r.get(7)?,updated_at:r.get(8)?})).optional().ok().flatten());
    if let Some(item) = item {
        let _ = app.emit(FILE_PROGRESS_EVENT, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory_for_test;
    const SAMPLE: &str = r#"<Ophthalmology><Common><ModelName>CT-800</ModelName><Date>2026-03-06</Date><Time>16:35:42</Time></Common><Measure type="TM"><TM><R><List><IOP_mmHg>16.0</IOP_mmHg></List><Average><IOP_mmHg>15.0</IOP_mmHg></Average></R><L><Average><IOP_mmHg>14.0</IOP_mmHg></Average></L></TM></Measure></Ophthalmology>"#;
    #[test]
    fn filename_is_end_anchored_and_validates_time() {
        let v = parse_filename("HCM_a_20260306_163542_TOPCON_CT-800_2862629.xml").unwrap();
        assert_eq!(v.ma_ho_so, "HCM_a");
        assert_eq!(v.source_time, "2026-03-06 16:35:42");
        assert_eq!(v.machine_serial, "2862629");
        assert_eq!(
            parse_filename("A_TOPCON_CT-800_B_20260306_163542_TOPCON_CT-800_x.xml")
                .unwrap()
                .ma_ho_so,
            "A_TOPCON_CT-800_B"
        );
        assert!(parse_filename("HCM_20260230_163542_TOPCON_CT-800_x.xml").is_err());
        assert!(parse_filename("HCM_20260306_256142_TOPCON_CT-800_x.xml").is_err());
    }
    #[test]
    fn parser_uses_average_maps_catalogue_and_requires_tm_model() {
        let p = parse_ct800_xml(
            SAMPLE.as_bytes(),
            "HCM_20260306_163542_TOPCON_CT-800_2862629.xml",
        )
        .unwrap();
        assert_eq!(p.right_raw.as_deref(), Some("15.0"));
        assert_eq!(p.left_raw.as_deref(), Some("14.0"));
        let mismatched = parse_ct800_xml(
            SAMPLE.as_bytes(),
            "HCM_20260306_164000_TOPCON_CT-800_2862629.xml",
        )
        .unwrap();
        assert_eq!(mismatched.file.source_time, "2026-03-06 16:40:00");
        assert_eq!(mismatched.xml_time.as_deref(), Some("2026-03-06 16:35:42"));
        assert_eq!(
            p.payload,
            serde_json::json!({"matPhaiNhanApId":403,"matTraiNhanApId":402})
        );
        assert!(matches!(
            parse_ct800_xml(
                b"<x><ModelName>KR-800</ModelName></x>",
                "HCM_20260306_163542_TOPCON_CT-800_x.xml"
            ),
            Err(Ct800ParseError::WrongModel(_))
        ));
        assert!(matches!(
            parse_ct800_xml(
                b"<x><ModelName>CT-800</ModelName><Measure type='TM'><TM/></Measure></x>",
                "HCM_20260306_163542_TOPCON_CT-800_x.xml"
            ),
            Err(Ct800ParseError::WrongModel(_))
        ));
        assert!(matches!(
            parse_ct800_xml(
                b"<x><Common><ModelName>CT-800</ModelName></Common></x>",
                "HCM_20260306_163542_TOPCON_CT-800_x.xml"
            ),
            Err(Ct800ParseError::MissingTmMeasure)
        ));
    }
    #[test]
    fn blank_eyes_are_sparse_and_no_supported_data() {
        let xml = SAMPLE
            .replace(
                "<Average><IOP_mmHg>15.0</IOP_mmHg></Average>",
                "<Average><IOP_mmHg></IOP_mmHg></Average>",
            )
            .replace(
                "<Average><IOP_mmHg>14.0</IOP_mmHg></Average>",
                "<Average><IOP_mmHg></IOP_mmHg></Average>",
            );
        let p = parse_ct800_xml(xml.as_bytes(), "HCM_20260306_163542_TOPCON_CT-800_x.xml").unwrap();
        assert_eq!(p.payload, serde_json::json!({}));
        assert_eq!(parsed_status(&p), "no_supported_data");
    }
    #[test]
    fn one_blank_eye_is_omitted_instead_of_serialized_as_null() {
        let xml = SAMPLE.replace(
            "<Average><IOP_mmHg>14.0</IOP_mmHg></Average>",
            "<Average><IOP_mmHg> </IOP_mmHg></Average>",
        );
        let parsed =
            parse_ct800_xml(xml.as_bytes(), "HCM_20260306_163542_TOPCON_CT-800_x.xml").unwrap();
        assert_eq!(parsed.left_raw, None);
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiNhanApId":403}));
        assert!(parsed.payload.get("matTraiNhanApId").is_none());
    }
    #[test]
    fn canonical_decimal_does_not_round() {
        assert_eq!(canonical_iop("15").unwrap(), "15");
        assert_eq!(canonical_iop("15.0").unwrap(), "15");
        assert_eq!(canonical_iop("15.00").unwrap(), "15");
        assert_eq!(canonical_iop("14.3").unwrap(), "14.3");
        assert!(pressure_catalogue().unwrap().get("14") == Some(&402));
        assert!(pressure_catalogue().unwrap().get("15") == Some(&403));
    }
    #[test]
    fn official_catalogue_loads_all_rows_without_relaxing_xml_range() {
        let catalogue = pressure_catalogue().unwrap();
        assert_eq!(catalogue.len(), 66);
        assert_eq!(catalogue.get("70"), Some(&1411));
        assert!(canonical_iop("61").is_err());
    }
    #[test]
    fn missing_decimal_catalogue_value_is_not_rounded() {
        let xml = SAMPLE.replace(
            "<Average><IOP_mmHg>15.0</IOP_mmHg></Average>",
            "<Average><IOP_mmHg>14.3</IOP_mmHg></Average>",
        );
        assert!(matches!(
            parse_ct800_xml(
                xml.as_bytes(),
                "HCM_20260306_163542_TOPCON_CT-800_x.xml"
            ),
            Err(Ct800ParseError::Mapping(value)) if value == "14.3"
        ));
    }
    #[test]
    fn namespaces_are_matched_by_local_name() {
        let xml = r#"<root xmlns:c="urn:common" xmlns:m="urn:tm"><c:Common><c:ModelName>CT-800</c:ModelName></c:Common><m:Measure type="TM"><m:TM><m:R><m:List><m:IOP_mmHg>60</m:IOP_mmHg></m:List><m:Average><m:IOP_mmHg>15.00</m:IOP_mmHg></m:Average></m:R></m:TM></m:Measure></root>"#;
        let parsed =
            parse_ct800_xml(xml.as_bytes(), "HCM_20260306_163542_TOPCON_CT-800_x.xml").unwrap();
        assert_eq!(parsed.right_raw.as_deref(), Some("15.00"));
        assert_eq!(parsed.payload, serde_json::json!({"matPhaiNhanApId":403}));
    }
    #[test]
    fn newer_field_excludes_stale_retry() {
        let db = open_memory_for_test().unwrap();
        let r = Revision {
            id: 1,
            ma: "HCM1".into(),
            time: "2026-03-06 16:35:42".into(),
            payload: r#"{"matPhaiNhanApId":403,"matTraiNhanApId":402}"#.into(),
            right_raw: Some("15.0".into()),
            left_raw: Some("14.0".into()),
            file_name: "HCM1_20260306_163542_TOPCON_CT-800_x.xml".into(),
            xml: SAMPLE.as_bytes().to_vec(),
        };
        db.conn.lock().unwrap().execute("INSERT INTO ct800_field_versions(device_key,ma_ho_so,field_path,revision_id,source_time,created_at) VALUES(?1,?2,?3,2,?4,datetime('now'))",params![DEVICE_KEY,"HCM1","matPhaiNhanApId","2026-03-06 16:40:00"]).unwrap();
        assert_eq!(
            stale_payload(&db, &r).unwrap(),
            serde_json::json!({"matTraiNhanApId":402})
        );
    }
    #[test]
    fn content_hash_and_service_cache_are_scoped_by_device() {
        let db = open_memory_for_test().unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.execute(
                "INSERT OR IGNORE INTO ct800_content_hashes(device_key,content_hash,created_at) VALUES(?1,'same-hash',datetime('now'))",
                params![DEVICE_KEY],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.execute(
                "INSERT OR IGNORE INTO ct800_content_hashes(device_key,content_hash,created_at) VALUES('hdr-9000','same-hash',datetime('now'))",
                [],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.execute(
                "INSERT OR IGNORE INTO ct800_content_hashes(device_key,content_hash,created_at) VALUES(?1,'same-hash',datetime('now'))",
                params![DEVICE_KEY],
            )
            .unwrap(),
            0
        );
        conn.execute(
            "INSERT INTO ct800_service_cache(device_key,ma_ho_so,dv_kham_id,updated_at) VALUES(?1,'HCM1',10,datetime('now')),('hdr-9000','HCM1',20,datetime('now'))",
            params![DEVICE_KEY],
        )
        .unwrap();
        let ct: i64 = conn
            .query_row(
                "SELECT dv_kham_id FROM ct800_service_cache WHERE device_key=?1 AND ma_ho_so='HCM1'",
                params![DEVICE_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ct, 10);
    }
}
