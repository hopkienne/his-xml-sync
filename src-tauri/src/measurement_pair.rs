//! Ghép cặp hai lần đo KR-800 theo Patient.ID + measuredAt + Patient.No.
//!
//! Invariant:
//! - Một XML thuộc tối đa một cặp.
//! - Một `patient_code_norm` chỉ có tối đa một cặp đang mở (chưa processed).
//! - Một cặp có đúng hai content_hash khác nhau khi đầy đủ.
//! - Claim/cập nhật hai file trong transaction để tránh double PUT.

use crate::app_logger;
use crate::db::AppDb;
use crate::xml_parser::{format_measured_at, ParsedEye, ParsedMeasurement};
use chrono::NaiveDateTime;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::sync::MutexGuard;

pub const DEVICE_KEY: &str = "kr-800";

/// Thời hạn lease mỗi lần claim/gia hạn.
/// Phải dài hơn HTTP timeout (30s) + backoff transient; được renew mỗi attempt.
pub const SENDING_LEASE_SECS: i64 = 120;

/// Tạo instance_id duy nhất cho một process app (PID + nanos).
pub fn generate_instance_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("kr800-{}-{}", std::process::id(), nanos)
}

/// Snapshot đã parse — nguồn duy nhất cho payload PUT (không đọc lại file mutable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementSnapshot {
    pub version: u32,
    /// Optional XML metadata, retained only for audit/logging.
    #[serde(default, alias = "patientId")]
    pub xml_patient_id: Option<String>,
    pub patient_no: i64,
    pub measured_at: String,
    pub content_hash: String,
    pub right: EyeSnapshot,
    pub left: EyeSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EyeSnapshot {
    pub sphere: f64,
    pub cylinder: f64,
    pub axis: i64,
}

impl MeasurementSnapshot {
    pub const VERSION: u32 = 2;

    pub fn from_parsed(parsed: &ParsedMeasurement, content_hash: &str) -> Self {
        Self {
            version: Self::VERSION,
            xml_patient_id: parsed.xml_patient_id.clone(),
            patient_no: parsed.patient_no,
            measured_at: format_measured_at(parsed.measured_at),
            content_hash: content_hash.to_string(),
            right: EyeSnapshot::from_eye(&parsed.right),
            left: EyeSnapshot::from_eye(&parsed.left),
        }
    }

    pub fn right_eye(&self) -> ParsedEye {
        ParsedEye {
            sphere: self.right.sphere,
            cylinder: self.right.cylinder,
            axis: self.right.axis,
        }
    }

    pub fn left_eye(&self) -> ParsedEye {
        ParsedEye {
            sphere: self.left.sphere,
            cylinder: self.left.cylinder,
            axis: self.left.axis,
        }
    }
}

impl EyeSnapshot {
    fn from_eye(eye: &ParsedEye) -> Self {
        Self {
            sphere: eye.sphere,
            cylinder: eye.cylinder,
            axis: eye.axis,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MeasurementMeta {
    pub file_id: i64,
    pub patient_code: String,
    pub patient_code_norm: String,
    pub patient_no: i64,
    pub measured_at: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct OrderedPair {
    pub first: MeasurementMeta,
    pub second: MeasurementMeta,
}

#[derive(Debug, Clone)]
pub struct PairRecord {
    pub id: i64,
    pub patient_code: String,
    pub patient_code_norm: String,
    pub file_id_1: Option<i64>,
    pub file_id_2: Option<i64>,
    pub content_hash_1: Option<String>,
    pub content_hash_2: Option<String>,
    pub patient_no_1: Option<i64>,
    pub patient_no_2: Option<i64>,
    pub measured_at_1: Option<String>,
    pub measured_at_2: Option<String>,
    pub status: String,
    pub request_payload: Option<String>,
    pub nb_dot_dieu_tri_id: Option<i64>,
    pub dv_kham_id: Option<i64>,
}

/// Kết quả resolve sau khi parse một XML mới.
#[derive(Debug)]
pub enum PairResolve {
    /// Chỉ có lần đo 1 — chờ file thứ hai, không gọi HIS.
    AwaitingSecond {
        pair_id: i64,
        patient_code: String,
    },
    /// Đã có đủ hai lần đo, sẵn sàng map + PUT.
    Ready {
        pair_id: i64,
        ordered: OrderedPair,
    },
    /// measuredAt/Patient.No. mâu thuẫn.
    PairingError {
        pair_id: i64,
        message: String,
    },
    /// Đã có cặp (hoặc đang chờ) khác cho hồ sơ này.
    ExtraMeasurement {
        message: String,
    },
}

pub fn normalize_patient_code(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileResult { pub reconciled_files: usize, pub invalid_files: usize }

/// Repair pending records created while XML `Patient.ID` was incorrectly used
/// as the HIS code. This is transactional, idempotent, and leaves every
/// successfully processed XML untouched.
pub fn reconcile_pending_patient_codes(db: &AppDb) -> Result<ReconcileResult, String> {
    let mut conn = lock_conn(db)?;
    let tx = conn.transaction().map_err(|e| format!("Bắt đầu reconcile KR-800 thất bại: {e}"))?;
    let rows = {
        let mut stmt = tx.prepare("SELECT id, file_name, pair_id FROM xml_files WHERE device_key=?1 AND status NOT IN ('processed', 'invalid_filename')")
            .map_err(|e| format!("Đọc file KR-800 cần reconcile thất bại: {e}"))?;
        let rows = stmt.query_map(params![DEVICE_KEY], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<i64>>(2)?)))
            .map_err(|e| format!("Query file KR-800 cần reconcile thất bại: {e}"))?
            .collect::<Result<Vec<_>, _>>().map_err(|e| format!("Map file KR-800 cần reconcile thất bại: {e}"))?;
        rows
    };
    let mut result = ReconcileResult::default();
    let mut pair_ids = std::collections::BTreeSet::new();
    for (id, name, pair_id) in &rows {
        if let Some(pair_id) = pair_id { pair_ids.insert(*pair_id); }
        match crate::xml_track::parse_kr800_filename(name) {
            Ok(meta) => {
                let patient_code_norm = normalize_patient_code(&meta.ma_ho_so);
                let changed = tx.execute("UPDATE xml_files SET patient_code=?1, updated_at=datetime('now') WHERE id=?2 AND status <> 'processed' AND (patient_code IS NULL OR lower(trim(patient_code)) <> ?3)", params![meta.ma_ho_so, id, patient_code_norm])
                    .map_err(|e| format!("Cập nhật mã hồ sơ KR-800 id={id} thất bại: {e}"))?;
                result.reconciled_files += changed;
            }
            Err(error) => {
                tx.execute("UPDATE xml_files SET status='invalid_filename', error_message=?1, updated_at=datetime('now') WHERE id=?2 AND status <> 'processed'", params![error, id])
                    .map_err(|e| format!("Đánh dấu filename KR-800 không hợp lệ id={id}: {e}"))?;
                result.invalid_files += 1;
            }
        }
    }
    for pair_id in pair_ids {
        let members = {
            let mut stmt = tx.prepare("SELECT file_name, status FROM xml_files WHERE pair_id=?1")
                .map_err(|e| format!("Đọc pair KR-800 id={pair_id} thất bại: {e}"))?;
            let rows = stmt.query_map(params![pair_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(|e| format!("Query pair KR-800 id={pair_id} thất bại: {e}"))?
                .collect::<Result<Vec<_>, _>>().map_err(|e| format!("Map pair KR-800 id={pair_id} thất bại: {e}"))?;
            rows
        };
        let codes: Result<Vec<String>, String> = members.iter().map(|(name, _)| crate::xml_track::parse_kr800_filename(name).map(|meta| meta.ma_ho_so)).collect();
        let code = codes.ok().and_then(|codes| codes.first().and_then(|first| codes.iter().all(|code| normalize_patient_code(code) == normalize_patient_code(first)).then_some(first.clone())));
        if let Some(code) = code {
            let code_norm = normalize_patient_code(&code);
            tx.execute("UPDATE measurement_pairs SET patient_code=?1, patient_code_norm=?2, updated_at=datetime('now') WHERE id=?3 AND status <> 'processed'", params![code, code_norm, pair_id])
                .map_err(|e| format!("Cập nhật pair KR-800 id={pair_id} thất bại: {e}"))?;
        } else {
            let message = format!("Pair KR-800 có filename không hợp lệ hoặc mã hồ sơ không nhất quán; không gọi HIS (pair_id={pair_id}).");
            tx.execute("UPDATE xml_files SET status='invalid_filename', error_message=?1, updated_at=datetime('now') WHERE pair_id=?2 AND status <> 'processed'", params![message, pair_id])
                .map_err(|e| format!("Khoá pair filename không hợp lệ id={pair_id} thất bại: {e}"))?;
        }
    }
    tx.commit().map_err(|e| format!("Commit reconcile KR-800 thất bại: {e}"))?;
    Ok(result)
}

pub fn meta_from_parsed(file_id: i64, parsed: &ParsedMeasurement, content_hash: &str, patient_code: String) -> MeasurementMeta {
    MeasurementMeta {
        file_id,
        patient_code_norm: normalize_patient_code(&patient_code),
        patient_code,
        patient_no: parsed.patient_no,
        measured_at: format_measured_at(parsed.measured_at),
        content_hash: content_hash.to_string(),
    }
}

/// Xác định thứ tự lần 1 / lần 2 chỉ từ metadata XML (không dùng arrival/DB id).
///
/// Yêu cầu đồng thời: measured_at tăng **và** patient_no tăng.
pub fn order_measurements(
    a: MeasurementMeta,
    b: MeasurementMeta,
) -> Result<OrderedPair, String> {
    if a.content_hash == b.content_hash {
        return Err("Hai lần đo có cùng content hash — không thể ghép cặp.".into());
    }
    let at_a = parse_measured_at_str(&a.measured_at)?;
    let at_b = parse_measured_at_str(&b.measured_at)?;

    let a_before_b = at_a < at_b && a.patient_no < b.patient_no;
    let b_before_a = at_b < at_a && b.patient_no < a.patient_no;

    if a_before_b {
        Ok(OrderedPair {
            first: a,
            second: b,
        })
    } else if b_before_a {
        Ok(OrderedPair {
            first: b,
            second: a,
        })
    } else {
        Err(format!(
            "Không xác định được thứ tự lần đo: measuredAt ({}, {}) và Patient.No. ({}, {}) mâu thuẫn hoặc bằng nhau.",
            a.measured_at, b.measured_at, a.patient_no, b.patient_no
        ))
    }
}

fn parse_measured_at_str(value: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| format!("measured_at không hợp lệ: {value}"))
}

/// Hash đã từng là một lần đo hợp lệ (kể cả awaiting) → bản sao không được tính lần 2.
pub fn content_hash_already_measured(
    db: &AppDb,
    content_hash: &str,
    exclude_file_id: i64,
) -> Result<bool, String> {
    let conn = lock_conn(db)?;
    let found = conn
        .query_row(
            r#"
            SELECT 1 FROM xml_files
            WHERE content_hash = ?1
              AND id <> ?2
              AND status IN (
                'awaiting_pair', 'pairing', 'sending', 'processed',
                'pairing_error', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error', 'extra_measurement',
                'parsed', 'patient_matched', 'mapped'
              )
            LIMIT 1
            "#,
            params![content_hash, exclude_file_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| format!("Kiểm tra content_hash thất bại: {e}"))?;
    Ok(found.is_some())
}

/// Lưu metadata + snapshot parse atomic (một UPDATE).
pub fn save_measurement_meta(
    db: &AppDb,
    meta: &MeasurementMeta,
    parsed: &ParsedMeasurement,
) -> Result<(), String> {
    let snapshot = MeasurementSnapshot::from_parsed(parsed, &meta.content_hash);
    let snapshot_json = serde_json::to_string(&snapshot)
        .map_err(|e| format!("Serialize measurement_snapshot thất bại: {e}"))?;
    let conn = lock_conn(db)?;
    let n = conn
        .execute(
            r#"
            UPDATE xml_files SET
              content_hash = ?1,
              patient_code = ?2,
              patient_no = ?3,
              measured_at = ?4,
              measurement_snapshot = ?5,
              updated_at = datetime('now')
            WHERE id = ?6
            "#,
            params![
                meta.content_hash,
                meta.patient_code,
                meta.patient_no,
                meta.measured_at,
                snapshot_json,
                meta.file_id
            ],
        )
        .map_err(|e| format!("Lưu metadata+snapshot đo thất bại: {e}"))?;
    if n != 1 {
        return Err(format!(
            "Lưu snapshot file_id={} thất bại: cập nhật {n} dòng (kỳ vọng 1).",
            meta.file_id
        ));
    }
    Ok(())
}

/// Đọc snapshot đã lưu; None nếu legacy chưa có.
pub fn load_snapshot(db: &AppDb, file_id: i64) -> Result<Option<MeasurementSnapshot>, String> {
    let conn = lock_conn(db)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT measurement_snapshot FROM xml_files WHERE id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Đọc measurement_snapshot id={file_id}: {e}"))?;
    match raw {
        Some(json) if !json.trim().is_empty() => {
            let snap: MeasurementSnapshot = serde_json::from_str(&json).map_err(|e| {
                format!("measurement_snapshot id={file_id} JSON hỏng: {e}")
            })?;
            Ok(Some(snap))
        }
        _ => Ok(None),
    }
}

/// Meta đã lưu trên pair cho một file (dùng kiểm tra legacy rehydrate).
#[derive(Debug, Clone)]
pub struct ExpectedFileMeta {
    pub content_hash: String,
    pub patient_code: String,
    pub patient_no: i64,
    pub measured_at: String,
}

/// Lấy snapshot; nếu thiếu thì rehydrate từ disk **chỉ khi** hash + meta khớp pair.
/// Không bao giờ dùng nội dung file đã đổi để PUT cho pair cũ.
pub fn load_or_rehydrate_snapshot(
    db: &AppDb,
    file_id: i64,
    expected: &ExpectedFileMeta,
) -> Result<MeasurementSnapshot, String> {
    if let Some(snap) = load_snapshot(db, file_id)? {
        verify_snapshot_matches_expected(&snap, expected, file_id)?;
        return Ok(snap);
    }

    // Legacy: đọc file, verify hash + meta, rồi persist snapshot.
    let path = file_path_by_id(db, file_id)?;
    let bytes = std::fs::read(&path).map_err(|e| {
        format!("Legacy rehydrate: không đọc được file id={file_id} path={path}: {e}")
    })?;
    let hash = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    if hash != expected.content_hash {
        return Err(format!(
            "File id={file_id} đã thay đổi trên disk: content_hash DB={} ≠ file hiện tại={hash}. Không PUT.",
            expected.content_hash
        ));
    }
    let parsed = crate::xml_parser::parse_measurement(&bytes).map_err(|e| {
        format!("Legacy rehydrate: parse XML id={file_id} thất bại: {e}")
    })?;
    let measured_at = format_measured_at(parsed.measured_at);
    let mut mismatches = Vec::new();
    let file_name = std::path::Path::new(&path).file_name().and_then(|name| name.to_str())
        .ok_or_else(|| format!("Legacy rehydrate: path không có filename id={file_id}: {path}"))?;
    let filename_meta = crate::xml_track::parse_kr800_filename(file_name)
        .map_err(|error| format!("Legacy rehydrate: {error}"))?;
    if normalize_patient_code(&filename_meta.ma_ho_so) != normalize_patient_code(&expected.patient_code) {
        mismatches.push(format!(
            "maHoSo filename DB={} file={}",
            expected.patient_code, filename_meta.ma_ho_so
        ));
    }
    if parsed.patient_no != expected.patient_no {
        mismatches.push(format!(
            "Patient.No. DB={} file={}",
            expected.patient_no, parsed.patient_no
        ));
    }
    if measured_at != expected.measured_at {
        mismatches.push(format!(
            "measuredAt DB={} file={}",
            expected.measured_at, measured_at
        ));
    }
    if !mismatches.is_empty() {
        return Err(format!(
            "File id={file_id} metadata không khớp pair (hash OK nhưng meta lệch): {}",
            mismatches.join("; ")
        ));
    }

    let meta = MeasurementMeta {
        file_id,
        patient_code: filename_meta.ma_ho_so.clone(),
        patient_code_norm: normalize_patient_code(&filename_meta.ma_ho_so),
        patient_no: parsed.patient_no,
        measured_at,
        content_hash: hash,
    };
    save_measurement_meta(db, &meta, &parsed)?;
    load_snapshot(db, file_id)?.ok_or_else(|| {
        format!("Legacy rehydrate: lưu snapshot id={file_id} xong nhưng không đọc lại được.")
    })
}

fn verify_snapshot_matches_expected(
    snap: &MeasurementSnapshot,
    expected: &ExpectedFileMeta,
    file_id: i64,
) -> Result<(), String> {
    let mut mismatches = Vec::new();
    if snap.content_hash != expected.content_hash {
        mismatches.push(format!(
            "content_hash snap={} expected={}",
            snap.content_hash, expected.content_hash
        ));
    }
    if snap.patient_no != expected.patient_no {
        mismatches.push(format!(
            "Patient.No. snap={} expected={}",
            snap.patient_no, expected.patient_no
        ));
    }
    if snap.measured_at != expected.measured_at {
        mismatches.push(format!(
            "measuredAt snap={} expected={}",
            snap.measured_at, expected.measured_at
        ));
    }
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Snapshot file id={file_id} không khớp pair: {}",
            mismatches.join("; ")
        ))
    }
}

fn file_path_by_id(db: &AppDb, file_id: i64) -> Result<String, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        "SELECT file_path FROM xml_files WHERE id = ?1",
        params![file_id],
        |row| row.get(0),
    )
    .map_err(|e| format!("Đọc file_path id={file_id}: {e}"))
}

fn transition_log(
    pair_id: i64,
    transition: &str,
    from: &str,
    to: &str,
    cause: &str,
) -> String {
    format!(
        "pair_id={pair_id} transition={transition} from={from} to={to} rollback: {cause}"
    )
}

/// Transaction: gắn file vào cặp chờ hoặc hoàn tất cặp khi đủ hai lần đo.
///
/// Không dùng created_at/range để tìm partner — chỉ patient_code_norm (vắt qua nửa đêm vẫn ghép).
pub fn resolve_pair_for_measurement(
    db: &AppDb,
    meta: &MeasurementMeta,
) -> Result<PairResolve, String> {
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("Bắt đầu transaction pair thất bại: {e}"))?;

    // Đã thuộc cặp rồi (retry path) → không tạo cặp mới.
    if let Some(existing_pair_id) = file_pair_id(&tx, meta.file_id)? {
        if let Some(pair) = load_pair(&tx, existing_pair_id)? {
            if pair.file_id_1.is_some() && pair.file_id_2.is_some() {
                if let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) {
                    let first = load_meta(&tx, id1)?;
                    let second = load_meta(&tx, id2)?;
                    let ordered = order_measurements(first, second).map_err(|message| message)?;
                    tx.commit()
                        .map_err(|e| format!("Commit pair resolve thất bại: {e}"))?;
                    return Ok(PairResolve::Ready {
                        pair_id: pair.id,
                        ordered,
                    });
                }
            }
        }
    }

    // Cặp đã processed / pairing_error đầy đủ → file thứ ba.
    if patient_has_terminal_pair(&tx, &meta.patient_code_norm)? {
        mark_extra(&tx, meta.file_id, "Phát hiện lần đo thừa — hồ sơ đã có cặp đo.")?;
        tx.commit()
            .map_err(|e| format!("Commit extra_measurement thất bại: {e}"))?;
        return Ok(PairResolve::ExtraMeasurement {
            message: format!(
                "Hồ sơ {} đã có cặp đo — file này là lần đo thừa, không ghi đè HIS.",
                meta.patient_code
            ),
        });
    }

    // Cặp đang mở (awaiting / retry send_error với 1 file, v.v.)
    if let Some(open) = find_open_pair(&tx, &meta.patient_code_norm)? {
        if open.file_id_2.is_some() {
            // Đã đủ 2 file trong cặp mở (đang pairing/send_error) — file mới là thừa.
            mark_extra(
                &tx,
                meta.file_id,
                "Phát hiện lần đo thừa — cặp đang xử lý đã đủ hai file.",
            )?;
            tx.commit()
                .map_err(|e| format!("Commit extra_measurement thất bại: {e}"))?;
            return Ok(PairResolve::ExtraMeasurement {
                message: format!(
                    "Hồ sơ {} đã có đủ hai lần đo trong cặp — không ghi đè.",
                    meta.patient_code
                ),
            });
        }

        let partner_id = open
            .file_id_1
            .ok_or_else(|| "Cặp awaiting_pair thiếu file_id_1.".to_string())?;
        if partner_id == meta.file_id {
            tx.commit()
                .map_err(|e| format!("Commit pair resolve thất bại: {e}"))?;
            return Ok(PairResolve::AwaitingSecond {
                pair_id: open.id,
                patient_code: meta.patient_code.clone(),
            });
        }

        // Không ghép nếu hash trùng partner (bản sao lần 1).
        if open.content_hash_1.as_deref() == Some(meta.content_hash.as_str()) {
            mark_duplicate_file(&tx, meta.file_id, &meta.content_hash)?;
            tx.commit()
                .map_err(|e| format!("Commit duplicate pair thất bại: {e}"))?;
            return Ok(PairResolve::ExtraMeasurement {
                message: "Bản sao nội dung của lần đo đang chờ — không tính là lần đo 2.".into(),
            });
        }

        let partner = load_meta(&tx, partner_id)?;
        match order_measurements(partner, meta.clone()) {
            Ok(ordered) => {
                complete_pair_rows(&tx, open.id, &ordered)?;
                tx.commit()
                    .map_err(|e| format!("Commit pair ready thất bại: {e}"))?;
                Ok(PairResolve::Ready {
                    pair_id: open.id,
                    ordered,
                })
            }
            Err(message) => {
                mark_pairing_error(&tx, open.id, partner_id, meta.file_id, &message)?;
                tx.commit()
                    .map_err(|e| format!("Commit pairing_error thất bại: {e}"))?;
                Ok(PairResolve::PairingError {
                    pair_id: open.id,
                    message,
                })
            }
        }
    } else {
        // Chưa có cặp — tạo awaiting_pair với file hiện tại (chưa gán order 1/2).
        let pair_id = insert_awaiting_pair(&tx, meta)?;
        // Arrival defines measurement #1.  It is intentionally never reordered
        // after its PUT has completed.
        attach_file_to_pair(&tx, meta.file_id, pair_id, Some(1), "awaiting_pair")?;
        tx.commit()
            .map_err(|e| format!("Commit awaiting_pair thất bại: {e}"))?;
        Ok(PairResolve::AwaitingSecond {
            pair_id,
            patient_code: meta.patient_code.clone(),
        })
    }
}

fn complete_pair_rows(tx: &Transaction<'_>, pair_id: i64, ordered: &OrderedPair) -> Result<(), String> {
    let n = tx
        .execute(
            r#"
            UPDATE measurement_pairs SET
              file_id_1 = ?1,
              file_id_2 = ?2,
              content_hash_1 = ?3,
              content_hash_2 = ?4,
              patient_no_1 = ?5,
              patient_no_2 = ?6,
              measured_at_1 = ?7,
              measured_at_2 = ?8,
              status = 'pairing',
              error_message = NULL,
              updated_at = datetime('now')
            WHERE id = ?9 AND status = 'awaiting_pair'
            "#,
            params![
                ordered.first.file_id,
                ordered.second.file_id,
                ordered.first.content_hash,
                ordered.second.content_hash,
                ordered.first.patient_no,
                ordered.second.patient_no,
                ordered.first.measured_at,
                ordered.second.measured_at,
                pair_id
            ],
        )
        .map_err(|e| format!("Cập nhật measurement_pairs sẵn sàng thất bại: {e}"))?;
    if n != 1 {
        return Err(format!(
            "complete_pair_rows pair_id={pair_id}: UPDATE rows={n} (cần status=awaiting_pair)"
        ));
    }

    // File #1 may already be processed (or waiting for retry); never overwrite
    // its per-file outcome when file #2 arrives.
    tx.execute(
        "UPDATE xml_files SET pair_id=?1, pair_order=1, status=CASE WHEN status='awaiting_pair' THEN 'pairing' ELSE status END, updated_at=datetime('now') WHERE id=?2",
        params![pair_id, ordered.first.file_id],
    ).map_err(|e| format!("Giữ trạng thái file 1 khi ghép pair thất bại: {e}"))?;
    attach_file_to_pair(tx, ordered.second.file_id, pair_id, Some(2), "pairing")?;
    Ok(())
}

fn insert_awaiting_pair(tx: &Transaction<'_>, meta: &MeasurementMeta) -> Result<i64, String> {
    tx.execute(
        r#"
        INSERT INTO measurement_pairs (
          device_key, patient_code, patient_code_norm,
          file_id_1, content_hash_1, patient_no_1, measured_at_1,
          status, created_at, updated_at
        ) VALUES (
          ?1, ?2, ?3,
          ?4, ?5, ?6, ?7,
          'awaiting_pair', datetime('now'), datetime('now')
        )
        "#,
        params![
            DEVICE_KEY,
            meta.patient_code,
            meta.patient_code_norm,
            meta.file_id,
            meta.content_hash,
            meta.patient_no,
            meta.measured_at
        ],
    )
    .map_err(|e| format!("Tạo measurement_pairs thất bại: {e}"))?;
    Ok(tx.last_insert_rowid())
}

fn attach_file_to_pair(
    tx: &Transaction<'_>,
    file_id: i64,
    pair_id: i64,
    pair_order: Option<i64>,
    status: &str,
) -> Result<(), String> {
    tx.execute(
        r#"
        UPDATE xml_files SET
          pair_id = ?1,
          pair_order = ?2,
          status = ?3,
          error_message = NULL,
          updated_at = datetime('now')
        WHERE id = ?4
        "#,
        params![pair_id, pair_order, status, file_id],
    )
    .map_err(|e| format!("Gắn file vào pair thất bại: {e}"))?;
    Ok(())
}

fn mark_pairing_error(
    tx: &Transaction<'_>,
    pair_id: i64,
    file_a: i64,
    file_b: i64,
    message: &str,
) -> Result<(), String> {
    tx.execute(
        r#"
        UPDATE measurement_pairs SET
          file_id_2 = ?1,
          status = 'pairing_error',
          error_message = ?2,
          updated_at = datetime('now')
        WHERE id = ?3 AND status = 'awaiting_pair'
        "#,
        params![file_b, message, pair_id],
    )
    .map_err(|e| format!("Lưu pairing_error pair thất bại: {e}"))?;
    for id in [file_a, file_b] {
        tx.execute(
            r#"
            UPDATE xml_files SET
              pair_id = ?1,
              status = 'pairing_error',
              error_message = ?2,
              updated_at = datetime('now')
            WHERE id = ?3
            "#,
            params![pair_id, message, id],
        )
        .map_err(|e| format!("Lưu pairing_error file thất bại: {e}"))?;
    }
    Ok(())
}

fn mark_extra(tx: &Transaction<'_>, file_id: i64, message: &str) -> Result<(), String> {
    tx.execute(
        r#"
        UPDATE xml_files SET
          status = 'extra_measurement',
          error_message = ?1,
          updated_at = datetime('now')
        WHERE id = ?2
        "#,
        params![message, file_id],
    )
    .map_err(|e| format!("Đánh extra_measurement thất bại: {e}"))?;
    Ok(())
}

fn mark_duplicate_file(tx: &Transaction<'_>, file_id: i64, hash: &str) -> Result<(), String> {
    tx.execute(
        r#"
        UPDATE xml_files SET
          status = 'processed',
          error_message = NULL,
          response_payload = ?1,
          processed_at = datetime('now'),
          updated_at = datetime('now')
        WHERE id = ?2
        "#,
        params![format!("duplicate_skipped:{hash}"), file_id],
    )
    .map_err(|e| format!("Lưu duplicate file thất bại: {e}"))?;
    Ok(())
}

fn patient_has_terminal_pair(tx: &Transaction<'_>, patient_norm: &str) -> Result<bool, String> {
    let found = tx
        .query_row(
            r#"
            SELECT 1 FROM measurement_pairs
            WHERE device_key = ?1
              AND patient_code_norm = ?2
              AND status IN ('processed', 'pairing_error')
            LIMIT 1
            "#,
            params![DEVICE_KEY, patient_norm],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| format!("Kiểm tra cặp terminal thất bại: {e}"))?;
    Ok(found.is_some())
}

fn find_open_pair(tx: &Transaction<'_>, patient_norm: &str) -> Result<Option<PairRecord>, String> {
    tx.query_row(
        r#"
        SELECT id, patient_code, patient_code_norm,
               file_id_1, file_id_2, content_hash_1, content_hash_2,
               patient_no_1, patient_no_2, measured_at_1, measured_at_2,
               status, request_payload, nb_dot_dieu_tri_id, dv_kham_id
        FROM measurement_pairs
        WHERE device_key = ?1
          AND patient_code_norm = ?2
          AND status IN (
            'awaiting_pair', 'pairing', 'sending', 'send_error',
            'patient_not_found', 'treatment_ambiguous', 'mapping_error'
          )
        ORDER BY id ASC
        LIMIT 1
        "#,
        params![DEVICE_KEY, patient_norm],
        map_pair_row,
    )
    .optional()
    .map_err(|e| format!("Tìm open pair thất bại: {e}"))
}

pub fn load_pair_by_id(db: &AppDb, pair_id: i64) -> Result<Option<PairRecord>, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        r#"
        SELECT id, patient_code, patient_code_norm,
               file_id_1, file_id_2, content_hash_1, content_hash_2,
               patient_no_1, patient_no_2, measured_at_1, measured_at_2,
               status, request_payload, nb_dot_dieu_tri_id, dv_kham_id
        FROM measurement_pairs WHERE id = ?1
        "#,
        params![pair_id],
        map_pair_row,
    )
    .optional()
    .map_err(|e| format!("Đọc pair id={pair_id} thất bại: {e}"))
}

fn load_pair(tx: &Transaction<'_>, pair_id: i64) -> Result<Option<PairRecord>, String> {
    tx.query_row(
        r#"
        SELECT id, patient_code, patient_code_norm,
               file_id_1, file_id_2, content_hash_1, content_hash_2,
               patient_no_1, patient_no_2, measured_at_1, measured_at_2,
               status, request_payload, nb_dot_dieu_tri_id
        FROM measurement_pairs WHERE id = ?1
        "#,
        params![pair_id],
        map_pair_row,
    )
    .optional()
    .map_err(|e| format!("Đọc pair id={pair_id} thất bại: {e}"))
}

fn map_pair_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairRecord> {
    Ok(PairRecord {
        id: row.get(0)?,
        patient_code: row.get(1)?,
        patient_code_norm: row.get(2)?,
        file_id_1: row.get(3)?,
        file_id_2: row.get(4)?,
        content_hash_1: row.get(5)?,
        content_hash_2: row.get(6)?,
        patient_no_1: row.get(7)?,
        patient_no_2: row.get(8)?,
        measured_at_1: row.get(9)?,
        measured_at_2: row.get(10)?,
        status: row.get(11)?,
        request_payload: row.get(12)?,
        nb_dot_dieu_tri_id: row.get(13)?,
        dv_kham_id: row.get(14)?,
    })
}

fn file_pair_id(tx: &Transaction<'_>, file_id: i64) -> Result<Option<i64>, String> {
    tx.query_row(
        "SELECT pair_id FROM xml_files WHERE id = ?1",
        params![file_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(|e| format!("Đọc pair_id file thất bại: {e}"))
}

fn load_meta(tx: &Transaction<'_>, file_id: i64) -> Result<MeasurementMeta, String> {
    tx.query_row(
        r#"
        SELECT id, patient_code, patient_no, measured_at, content_hash
        FROM xml_files WHERE id = ?1
        "#,
        params![file_id],
        |row| {
            let patient_code: String = row.get(1)?;
            Ok(MeasurementMeta {
                file_id: row.get(0)?,
                patient_code_norm: normalize_patient_code(&patient_code),
                patient_code,
                patient_no: row.get(2)?,
                measured_at: row.get(3)?,
                content_hash: row.get(4)?,
            })
        },
    )
    .map_err(|e| format!("Đọc metadata file id={file_id} thất bại: {e}"))
}

/// Recovery pair `sending` **hết lease** (hoặc legacy lease NULL) → `send_error`.
/// Không reclaim pair còn lease hiệu lực (instance khác đang gửi).
///
/// Gọi được khi startup / poll — **không** cần XML `waiting`.
pub fn recover_expired_sending_pairs(db: &AppDb) -> Result<usize, String> {
    let candidates: Vec<(i64, Option<i64>, Option<i64>)> = {
        let conn = lock_conn(db)?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, file_id_1, file_id_2
                FROM measurement_pairs
                WHERE device_key = ?1
                  AND status = 'sending'
                  AND (
                    sending_lease_until IS NULL
                    OR sending_lease_until <= datetime('now')
                  )
                "#,
            )
            .map_err(|e| format!("recover_expired_sending prepare: {e}"))?;
        let rows = stmt
            .query_map(params![DEVICE_KEY], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(|e| format!("recover_expired_sending query: {e}"))?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| format!("recover_expired_sending row: {e}"))?);
        }
        list
    };

    let mut recovered = 0usize;
    let message =
        "Phục hồi pair sending hết lease (orphan/crash) — sẽ retry cùng pair/payload.";
    for (pair_id, f1, f2) in candidates {
        let mut conn = lock_conn(db)?;
        let tx = conn.transaction().map_err(|e| {
            format!("recover_expired_sending pair_id={pair_id} begin tx: {e}")
        })?;
        match transition_sending_to_error(
            &tx,
            pair_id,
            f1,
            f2,
            None, // lease reclaim — không cần owner
            true, // require lease expired
            "send_error",
            message,
            "recover_expired_sending",
        ) {
            Ok(()) => {
                tx.commit().map_err(|e| {
                    format!("recover_expired_sending pair_id={pair_id} commit: {e}")
                })?;
                recovered += 1;
                app_logger::warn(
                    "kr800",
                    &format!(
                        "pair_id={pair_id} recovered expired sending → send_error (file1={:?} file2={:?})",
                        f1, f2
                    ),
                );
            }
            Err(error) => {
                drop(tx);
                // Lease còn sống hoặc đã chuyển trạng thái — bỏ qua im lặng ở mức debug.
                app_logger::info(
                    "kr800",
                    &format!("pair_id={pair_id} recover skip: {error}"),
                );
            }
        }
    }
    Ok(recovered)
}

/// @deprecated alias — dùng `recover_expired_sending_pairs`.
pub fn recover_orphaned_sending_pairs(db: &AppDb) -> Result<usize, String> {
    recover_expired_sending_pairs(db)
}

/// Per-file state used by the new sender.  The pair remains the shared cache
/// and ordering record; the file is the unit that is leased, retried and
/// audited.
#[derive(Debug, Clone)]
pub struct FileSendRecord {
    pub id: i64,
    pub pair_id: i64,
    pub pair_order: u8,
    pub status: String,
    pub request_payload: Option<String>,
    pub nb_dot_dieu_tri_id: Option<i64>,
    pub dv_kham_id: Option<i64>,
    pub attempt_count: u32,
}

pub fn load_file_send_record(db: &AppDb, file_id: i64) -> Result<Option<FileSendRecord>, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        "SELECT id, pair_id, pair_order, status, request_payload, nb_dot_dieu_tri_id, dv_kham_id, attempt_count FROM xml_files WHERE id = ?1",
        params![file_id],
        |r| Ok(FileSendRecord { id: r.get(0)?, pair_id: r.get(1)?, pair_order: r.get::<_, i64>(2)? as u8,
            status: r.get(3)?, request_payload: r.get(4)?, nb_dot_dieu_tri_id: r.get(5)?, dv_kham_id: r.get(6)?, attempt_count: r.get::<_, i64>(7)? as u32 }),
    ).optional().map_err(|e| format!("Đọc file send id={file_id} thất bại: {e}"))
}

/// CAS claim.  File 2 is eligible only after the persisted first file is
/// processed; therefore separate app instances cannot reverse the two PUTs.
pub fn claim_file_for_send(db: &AppDb, file_id: i64, owner_id: &str) -> Result<bool, String> {
    let mut conn = lock_conn(db)?;
    let tx = conn.transaction().map_err(|e| format!("claim_file_send begin: {e}"))?;
    let file = tx.query_row(
        "SELECT pair_id, pair_order, status FROM xml_files WHERE id=?1", params![file_id],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?)),
    ).optional().map_err(|e| format!("claim_file_send read: {e}"))?
        .ok_or_else(|| format!("claim_file_send: không có file_id={file_id}"))?;
    let (Some(pair_id), Some(order)) = (file.0, file.1) else { return Ok(false); };
    if order == 2 {
        let first_status: String = tx.query_row("SELECT status FROM xml_files WHERE pair_id=?1 AND pair_order=1", params![pair_id], |r| r.get(0))
            .map_err(|e| format!("claim file 2 đọc file 1: {e}"))?;
        if first_status != "processed" { tx.commit().map_err(|e| e.to_string())?; return Ok(false); }
    }
    let lease = format!("+{SENDING_LEASE_SECS} seconds");
    let changed = tx.execute(r#"
        UPDATE xml_files SET status='sending', error_message=NULL,
          sending_started_at=datetime('now'), sending_owner_id=?2,
          sending_lease_until=datetime('now', ?3), updated_at=datetime('now')
        WHERE id=?1 AND status IN ('awaiting_pair','pairing','send_error','patient_not_found',
          'treatment_ambiguous','mapping_error','service_not_found')
    "#, params![file_id, owner_id, lease]).map_err(|e| format!("claim_file_send update: {e}"))?;
    tx.commit().map_err(|e| format!("claim_file_send commit: {e}"))?;
    Ok(changed == 1)
}

pub fn save_file_request(db: &AppDb, file_id: i64, pair_id: i64, nb_id: i64, dv_id: i64, payload: &str, owner_id: &str) -> Result<(), String> {
    let mut conn = lock_conn(db)?;
    let tx = conn.transaction().map_err(|e| format!("save_file_request begin: {e}"))?;
    let file_changed = tx.execute(r#"
        UPDATE xml_files SET nb_dot_dieu_tri_id=?1, dv_kham_id=?2, request_payload=?3, updated_at=datetime('now')
        WHERE id=?4 AND pair_id=?5 AND status='sending' AND sending_owner_id=?6
    "#, params![nb_id, dv_id, payload, file_id, pair_id, owner_id]).map_err(|e| format!("save_file_request file: {e}"))?;
    if file_changed != 1 { return Err(format!("save_file_request lost lease for file_id={file_id}")); }
    let pair_changed = tx.execute(r#"
        UPDATE measurement_pairs SET nb_dot_dieu_tri_id=?1, dv_kham_id=?2, updated_at=datetime('now')
        WHERE id=?3
    "#, params![nb_id, dv_id, pair_id]).map_err(|e| format!("save_file_request pair: {e}"))?;
    if pair_changed != 1 { return Err(format!("save_file_request missing pair_id={pair_id}")); }
    tx.commit().map_err(|e| format!("save_file_request commit: {e}"))
}

pub fn increment_file_attempt(db: &AppDb, file_id: i64, owner_id: &str) -> Result<(), String> {
    let conn = lock_conn(db)?;
    let lease = format!("+{SENDING_LEASE_SECS} seconds");
    let n = conn.execute(r#"
        UPDATE xml_files SET attempt_count=attempt_count+1, sending_lease_until=datetime('now', ?2), updated_at=datetime('now')
        WHERE id=?1 AND status='sending' AND sending_owner_id=?3
    "#, params![file_id, lease, owner_id]).map_err(|e| format!("increment file attempt: {e}"))?;
    if n == 1 { Ok(()) } else { Err(format!("increment file attempt lost lease file_id={file_id}")) }
}

pub fn finish_file_success(db: &AppDb, file_id: i64, response: &str, owner_id: &str) -> Result<(), String> {
    let mut conn = lock_conn(db)?;
    let tx = conn.transaction().map_err(|e| format!("finish_file begin: {e}"))?;
    let pair_id: i64 = tx.query_row("SELECT pair_id FROM xml_files WHERE id=?1", params![file_id], |r| r.get(0))
        .map_err(|e| format!("finish_file read pair: {e}"))?;
    let n = tx.execute(r#"
        UPDATE xml_files SET status='processed', error_message=NULL, response_payload=?1,
          processed_at=datetime('now'), sending_started_at=NULL, sending_owner_id=NULL,
          sending_lease_until=NULL, updated_at=datetime('now')
        WHERE id=?2 AND status='sending' AND sending_owner_id=?3
    "#, params![response, file_id, owner_id]).map_err(|e| format!("finish_file update: {e}"))?;
    if n != 1 { return Err(format!("finish_file lost lease file_id={file_id}")); }
    let second_exists: i64 = tx.query_row("SELECT COUNT(*) FROM xml_files WHERE pair_id=?1 AND pair_order=2", params![pair_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let all_done: i64 = tx.query_row("SELECT COUNT(*) FROM xml_files WHERE pair_id=?1 AND pair_order IN (1,2) AND status='processed'", params![pair_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let status = if second_exists == 1 && all_done == 2 { "processed" } else if second_exists == 1 { "pairing" } else { "awaiting_pair" };
    tx.execute("UPDATE measurement_pairs SET status=?1, error_message=NULL, processed_at=CASE WHEN ?1='processed' THEN datetime('now') ELSE processed_at END, updated_at=datetime('now') WHERE id=?2", params![status, pair_id])
        .map_err(|e| format!("finish_file pair: {e}"))?;
    tx.commit().map_err(|e| format!("finish_file commit: {e}"))
}

pub fn fail_file_send(db: &AppDb, file_id: i64, status: &str, message: &str, owner_id: &str) -> Result<(), String> {
    if !matches!(status, "send_error" | "mapping_error" | "patient_not_found" | "treatment_ambiguous" | "service_not_found") {
        return Err(format!("Trạng thái retry file không hợp lệ: {status}"));
    }
    let conn = lock_conn(db)?;
    let n = conn.execute(r#"
        UPDATE xml_files SET status=?1, error_message=?2, sending_started_at=NULL,
          sending_owner_id=NULL, sending_lease_until=NULL, updated_at=datetime('now')
        WHERE id=?3 AND status='sending' AND sending_owner_id=?4
    "#, params![status, message, file_id, owner_id]).map_err(|e| format!("fail_file_send: {e}"))?;
    if n == 1 { Ok(()) } else { Err(format!("fail_file_send lost lease file_id={file_id}")) }
}

pub fn recover_expired_sending_files(db: &AppDb) -> Result<usize, String> {
    let conn = lock_conn(db)?;
    conn.execute(r#"
      UPDATE xml_files SET status='send_error', error_message='Phục hồi file sending hết lease (crash/orphan) — sẽ retry cùng payload.',
        sending_started_at=NULL, sending_owner_id=NULL, sending_lease_until=NULL, updated_at=datetime('now')
      WHERE device_key=?1 AND status='sending' AND (sending_lease_until IS NULL OR sending_lease_until <= datetime('now'))
    "#, params![DEVICE_KEY]).map_err(|e| format!("recover expired file sends: {e}"))
      .map(|n| n as usize)
}

/// Includes old awaiting pairs with only file #1 and never filters by current
/// day, so recovery works after midnight and while the folder is unavailable.
pub fn retryable_files(db: &AppDb) -> Result<Vec<i64>, String> {
    let conn = lock_conn(db)?;
    let mut stmt = conn.prepare(r#"
      SELECT f.id FROM xml_files f
      WHERE f.device_key=?1
        AND f.status IN ('awaiting_pair','pairing','send_error','patient_not_found','treatment_ambiguous','mapping_error','service_not_found')
        AND f.pair_id IS NOT NULL AND f.pair_order IN (1,2)
        AND (f.pair_order=1 OR EXISTS (SELECT 1 FROM xml_files first WHERE first.pair_id=f.pair_id AND first.pair_order=1 AND first.status='processed'))
      ORDER BY f.pair_id, f.pair_order
    "#).map_err(|e| format!("retryable files prepare: {e}"))?;
    let rows = stmt.query_map(params![DEVICE_KEY], |r| r.get::<_, i64>(0)).map_err(|e| format!("retryable files query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("retryable files row: {e}"))
}

/// Informational counter: an open pair is not an outcome mutually exclusive
/// with a successfully processed first file.
pub fn count_open_pairs(db: &AppDb) -> Result<usize, String> {
    let conn = lock_conn(db)?;
    conn.query_row(
        "SELECT COUNT(*) FROM measurement_pairs WHERE device_key=?1 AND status IN ('awaiting_pair','pairing','send_error','patient_not_found','treatment_ambiguous','mapping_error')",
        params![DEVICE_KEY], |r| r.get::<_, i64>(0),
    ).map(|n| n as usize).map_err(|e| format!("Đếm pair đang mở: {e}"))
}

/// Claim cặp để gửi HIS — CAS status + gắn owner/lease.
pub fn claim_pair_for_send(db: &AppDb, pair_id: i64, owner_id: &str) -> Result<bool, String> {
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("claim_pair begin tx thất bại: {e}"))?;

    let pair = load_pair(&tx, pair_id)?
        .ok_or_else(|| format!("claim_pair: không tìm thấy pair_id={pair_id}"))?;
    let from = pair.status.clone();
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        return Err(transition_log(
            pair_id,
            "claim_pair",
            &from,
            "sending",
            "thiếu file_id_1 hoặc file_id_2",
        ));
    };

    if !matches!(
        from.as_str(),
        "pairing" | "send_error" | "patient_not_found" | "treatment_ambiguous" | "mapping_error"
    ) {
        tx.commit()
            .map_err(|e| format!("claim_pair commit (skip): {e}"))?;
        return Ok(false);
    }

    let lease_mod = format!("+{SENDING_LEASE_SECS} seconds");
    let changed = tx
        .execute(
            r#"
            UPDATE measurement_pairs SET
              status = 'sending',
              error_message = NULL,
              sending_started_at = datetime('now'),
              sending_owner_id = ?2,
              sending_lease_until = datetime('now', ?3),
              updated_at = datetime('now')
            WHERE id = ?1
              AND status IN (
                'pairing', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error'
              )
            "#,
            params![pair_id, owner_id, lease_mod],
        )
        .map_err(|e| {
            transition_log(pair_id, "claim_pair", &from, "sending", &format!("SQL pair: {e}"))
        })?;

    if changed == 0 {
        tx.commit()
            .map_err(|e| format!("claim_pair commit (no-op) thất bại: {e}"))?;
        return Ok(false);
    }
    if changed != 1 {
        return Err(transition_log(
            pair_id,
            "claim_pair",
            &from,
            "sending",
            &format!("pair UPDATE rows={changed}"),
        ));
    }

    let files = tx
        .execute(
            r#"
            UPDATE xml_files SET
              status = 'sending',
              error_message = NULL,
              updated_at = datetime('now')
            WHERE pair_id = ?1 AND id IN (?2, ?3)
              AND status IN (
                'pairing', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error', 'sending'
              )
            "#,
            params![pair_id, id1, id2],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "claim_pair",
                &from,
                "sending",
                &format!("SQL xml_files: {e}"),
            )
        })?;
    if files != 2 {
        return Err(transition_log(
            pair_id,
            "claim_pair",
            &from,
            "sending",
            &format!("xml_files UPDATE rows={files} (kỳ vọng 2; id1={id1} id2={id2})"),
        ));
    }
    assert_exactly_two_files_for_pair(&tx, pair_id, id1, id2, "claim_pair", &from, "sending")?;

    tx.commit()
        .map_err(|e| transition_log(pair_id, "claim_pair", &from, "sending", &format!("commit: {e}")))?;
    Ok(true)
}

pub fn save_pair_request(
    db: &AppDb,
    pair_id: i64,
    treatment_id: i64,
    payload_json: &str,
    owner_id: &str,
) -> Result<(), String> {
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("save_pair_request begin tx: {e}"))?;
    let pair = load_pair(&tx, pair_id)?
        .ok_or_else(|| format!("save_pair_request: thiếu pair_id={pair_id}"))?;
    let from = pair.status.clone();
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        return Err(transition_log(
            pair_id,
            "save_pair_request",
            &from,
            "sending",
            "thiếu file_id_1/2",
        ));
    };

    let n = tx
        .execute(
            r#"
            UPDATE measurement_pairs SET
              nb_dot_dieu_tri_id = ?1,
              request_payload = ?2,
              status = 'sending',
              updated_at = datetime('now')
            WHERE id = ?3
              AND status = 'sending'
              AND sending_owner_id = ?4
            "#,
            params![treatment_id, payload_json, pair_id, owner_id],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "save_pair_request",
                &from,
                "sending",
                &format!("SQL pair: {e}"),
            )
        })?;
    if n != 1 {
        return Err(transition_log(
            pair_id,
            "save_pair_request",
            &from,
            "sending",
            &format!(
                "pair UPDATE rows={n} (cần status=sending và owner={owner_id})"
            ),
        ));
    }

    let files = tx
        .execute(
            r#"
            UPDATE xml_files SET
              nb_dot_dieu_tri_id = ?1,
              request_payload = ?2,
              status = 'sending',
              updated_at = datetime('now')
            WHERE pair_id = ?3 AND id IN (?4, ?5)
            "#,
            params![treatment_id, payload_json, pair_id, id1, id2],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "save_pair_request",
                &from,
                "sending",
                &format!("SQL xml: {e}"),
            )
        })?;
    if files != 2 {
        return Err(transition_log(
            pair_id,
            "save_pair_request",
            &from,
            "sending",
            &format!("xml_files UPDATE rows={files}"),
        ));
    }
    assert_exactly_two_files_for_pair(&tx, pair_id, id1, id2, "save_pair_request", &from, "sending")?;
    tx.commit().map_err(|e| {
        transition_log(
            pair_id,
            "save_pair_request",
            &from,
            "sending",
            &format!("commit: {e}"),
        )
    })?;
    Ok(())
}

/// Chỉ Ok khi pair + đúng hai XML đã `processed` (cùng transaction, đúng owner).
pub fn finish_pair_success(
    db: &AppDb,
    pair_id: i64,
    response: &str,
    owner_id: &str,
) -> Result<(), String> {
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("finish_pair_success begin tx: {e}"))?;
    let pair = load_pair(&tx, pair_id)?
        .ok_or_else(|| format!("finish_pair_success: thiếu pair_id={pair_id}"))?;
    let from = pair.status.clone();
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        return Err(transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            "thiếu file_id_1/2",
        ));
    };

    let n = tx
        .execute(
            r#"
            UPDATE measurement_pairs SET
              status = 'processed',
              error_message = NULL,
              response_payload = ?1,
              sending_started_at = NULL,
              sending_owner_id = NULL,
              sending_lease_until = NULL,
              processed_at = datetime('now'),
              updated_at = datetime('now')
            WHERE id = ?2
              AND status = 'sending'
              AND sending_owner_id = ?3
            "#,
            params![response, pair_id, owner_id],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "finish_pair_success",
                &from,
                "processed",
                &format!("SQL pair: {e}"),
            )
        })?;
    if n != 1 {
        return Err(transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            &format!(
                "pair UPDATE rows={n} (cần status=sending và owner={owner_id}; không ghi đè processed)"
            ),
        ));
    }

    let files = tx
        .execute(
            r#"
            UPDATE xml_files SET
              status = 'processed',
              error_message = NULL,
              response_payload = ?1,
              processed_at = datetime('now'),
              updated_at = datetime('now')
            WHERE pair_id = ?2 AND id IN (?3, ?4) AND status = 'sending'
            "#,
            params![response, pair_id, id1, id2],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "finish_pair_success",
                &from,
                "processed",
                &format!("SQL xml: {e}"),
            )
        })?;
    if files != 2 {
        return Err(transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            &format!("xml_files UPDATE rows={files}"),
        ));
    }
    assert_exactly_two_files_for_pair(
        &tx,
        pair_id,
        id1,
        id2,
        "finish_pair_success",
        &from,
        "processed",
    )?;

    let pair_status: String = tx
        .query_row(
            "SELECT status FROM measurement_pairs WHERE id = ?1",
            params![pair_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("finish verify pair: {e}"))?;
    if pair_status != "processed" {
        return Err(transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            &format!("pair status sau update = {pair_status}"),
        ));
    }
    let processed_files: i64 = tx
        .query_row(
            r#"
            SELECT COUNT(*) FROM xml_files
            WHERE pair_id = ?1 AND id IN (?2, ?3) AND status = 'processed'
            "#,
            params![pair_id, id1, id2],
            |r| r.get(0),
        )
        .map_err(|e| format!("finish verify files: {e}"))?;
    if processed_files != 2 {
        return Err(transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            &format!("chỉ {processed_files}/2 file processed"),
        ));
    }

    tx.commit().map_err(|e| {
        transition_log(
            pair_id,
            "finish_pair_success",
            &from,
            "processed",
            &format!("commit: {e}"),
        )
    })?;
    Ok(())
}

/// Error path sau claim: chỉ owner hiện tại, chỉ từ `sending`, không đụng `processed`/`pairing_error`.
pub fn fail_pair(
    db: &AppDb,
    pair_id: i64,
    status: &str,
    message: &str,
    owner_id: &str,
) -> Result<(), String> {
    if !is_allowed_error_status(status) {
        return Err(format!(
            "fail_pair: status đích không hợp lệ từ sending: {status}"
        ));
    }
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("fail_pair begin tx: {e}"))?;
    let pair = load_pair(&tx, pair_id)?
        .ok_or_else(|| format!("fail_pair: thiếu pair_id={pair_id}"))?;
    let from = pair.status.clone();
    transition_sending_to_error(
        &tx,
        pair_id,
        pair.file_id_1,
        pair.file_id_2,
        Some(owner_id),
        false,
        status,
        message,
        "fail_pair",
    )?;
    tx.commit().map_err(|e| {
        transition_log(
            pair_id,
            "fail_pair",
            &from,
            status,
            &format!("commit: {e}"),
        )
    })?;
    Ok(())
}

/// Tăng attempt + gia hạn lease — chỉ owner.
pub fn increment_pair_attempt(db: &AppDb, pair_id: i64, owner_id: &str) -> Result<(), String> {
    let mut conn = lock_conn(db)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("increment_pair_attempt begin tx: {e}"))?;
    let pair = load_pair(&tx, pair_id)?
        .ok_or_else(|| format!("increment_pair_attempt: thiếu pair_id={pair_id}"))?;
    let from = pair.status.clone();
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        return Err(transition_log(
            pair_id,
            "increment_pair_attempt",
            &from,
            &from,
            "thiếu file_id_1/2",
        ));
    };

    let lease_mod = format!("+{SENDING_LEASE_SECS} seconds");
    let n = tx
        .execute(
            r#"
            UPDATE measurement_pairs SET
              attempt_count = attempt_count + 1,
              sending_lease_until = datetime('now', ?2),
              updated_at = datetime('now')
            WHERE id = ?1
              AND status = 'sending'
              AND sending_owner_id = ?3
            "#,
            params![pair_id, lease_mod, owner_id],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "increment_pair_attempt",
                &from,
                "sending",
                &format!("SQL pair: {e}"),
            )
        })?;
    if n != 1 {
        return Err(transition_log(
            pair_id,
            "increment_pair_attempt",
            &from,
            "sending",
            &format!("pair UPDATE rows={n} (cần owner={owner_id})"),
        ));
    }
    let files = tx
        .execute(
            r#"
            UPDATE xml_files SET
              attempt_count = attempt_count + 1,
              updated_at = datetime('now')
            WHERE pair_id = ?1 AND id IN (?2, ?3)
            "#,
            params![pair_id, id1, id2],
        )
        .map_err(|e| {
            transition_log(
                pair_id,
                "increment_pair_attempt",
                &from,
                "sending",
                &format!("SQL xml: {e}"),
            )
        })?;
    if files != 2 {
        return Err(transition_log(
            pair_id,
            "increment_pair_attempt",
            &from,
            "sending",
            &format!("xml_files UPDATE rows={files}"),
        ));
    }
    tx.commit().map_err(|e| {
        transition_log(
            pair_id,
            "increment_pair_attempt",
            &from,
            "sending",
            &format!("commit: {e}"),
        )
    })?;
    Ok(())
}

/// Cặp cần retry (không gồm `sending` live; expired sending phải recover trước).
pub fn retryable_pairs(db: &AppDb) -> Result<Vec<i64>, String> {
    let conn = lock_conn(db)?;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id FROM measurement_pairs
            WHERE device_key = ?1
              AND status IN (
                'pairing', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error'
              )
              AND file_id_1 IS NOT NULL
              AND file_id_2 IS NOT NULL
            ORDER BY id
            "#,
        )
        .map_err(|e| format!("Prepare retryable pairs thất bại: {e}"))?;
    let rows = stmt
        .query_map(params![DEVICE_KEY], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Query retryable pairs thất bại: {e}"))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| format!("Map pair id thất bại: {e}"))?);
    }
    Ok(ids)
}

/// Đếm pair chờ retry + sending đã hết lease (watcher, không phụ thuộc waiting XML).
pub fn count_pending_pair_work(db: &AppDb) -> Result<PendingPairWork, String> {
    let conn = lock_conn(db)?;
    let retryable: i64 = conn
        .query_row(
            r#"
            SELECT COUNT(*) FROM measurement_pairs
            WHERE device_key = ?1
              AND status IN (
                'pairing', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error'
              )
              AND file_id_1 IS NOT NULL AND file_id_2 IS NOT NULL
            "#,
            params![DEVICE_KEY],
            |r| r.get(0),
        )
        .map_err(|e| format!("count retryable pairs: {e}"))?;
    let expired_sending: i64 = conn
        .query_row(
            r#"
            SELECT COUNT(*) FROM measurement_pairs
            WHERE device_key = ?1
              AND status = 'sending'
              AND (
                sending_lease_until IS NULL
                OR sending_lease_until <= datetime('now')
              )
            "#,
            params![DEVICE_KEY],
            |r| r.get(0),
        )
        .map_err(|e| format!("count expired sending: {e}"))?;
    Ok(PendingPairWork {
        retryable: retryable as usize,
        expired_sending: expired_sending as usize,
    })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PendingPairWork {
    pub retryable: usize,
    pub expired_sending: usize,
}

impl PendingPairWork {
    pub fn total(self) -> usize {
        self.retryable + self.expired_sending
    }
}

fn is_allowed_error_status(status: &str) -> bool {
    matches!(
        status,
        "send_error" | "mapping_error" | "patient_not_found" | "treatment_ambiguous"
    )
}

/// CAS: `sending` → error status. Owner path hoặc lease-expired reclaim.
fn transition_sending_to_error(
    tx: &Transaction<'_>,
    pair_id: i64,
    file_id_1: Option<i64>,
    file_id_2: Option<i64>,
    owner_id: Option<&str>,
    require_lease_expired: bool,
    to_status: &str,
    message: &str,
    transition: &str,
) -> Result<(), String> {
    if !is_allowed_error_status(to_status) {
        return Err(transition_log(
            pair_id,
            transition,
            "sending",
            to_status,
            "status đích không thuộc error cho phép",
        ));
    }

    let n = if require_lease_expired {
        // Reclaim: không cần owner; chỉ khi lease hết / NULL.
        tx.execute(
            r#"
            UPDATE measurement_pairs SET
              status = ?1,
              error_message = ?2,
              sending_started_at = NULL,
              sending_owner_id = NULL,
              sending_lease_until = NULL,
              updated_at = datetime('now')
            WHERE id = ?3
              AND status = 'sending'
              AND (
                sending_lease_until IS NULL
                OR sending_lease_until <= datetime('now')
              )
            "#,
            params![to_status, message, pair_id],
        )
    } else {
        let owner = owner_id.ok_or_else(|| {
            transition_log(
                pair_id,
                transition,
                "sending",
                to_status,
                "thiếu owner_id cho fail path",
            )
        })?;
        tx.execute(
            r#"
            UPDATE measurement_pairs SET
              status = ?1,
              error_message = ?2,
              sending_started_at = NULL,
              sending_owner_id = NULL,
              sending_lease_until = NULL,
              updated_at = datetime('now')
            WHERE id = ?3
              AND status = 'sending'
              AND sending_owner_id = ?4
            "#,
            params![to_status, message, pair_id, owner],
        )
    }
    .map_err(|e| {
        transition_log(
            pair_id,
            transition,
            "sending",
            to_status,
            &format!("SQL pair: {e}"),
        )
    })?;

    if n != 1 {
        // Đọc status hiện tại để log (có thể đã processed).
        let current: String = tx
            .query_row(
                "SELECT status FROM measurement_pairs WHERE id = ?1",
                params![pair_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "missing".into());
        return Err(transition_log(
            pair_id,
            transition,
            &current,
            to_status,
            &format!(
                "CAS failed rows={n} owner={:?} lease_expired_required={require_lease_expired} (không ghi đè processed/pairing_error)",
                owner_id
            ),
        ));
    }

    match (file_id_1, file_id_2) {
        (Some(id1), Some(id2)) => {
            let files = tx
                .execute(
                    r#"
                    UPDATE xml_files SET
                      status = ?1,
                      error_message = ?2,
                      updated_at = datetime('now')
                    WHERE pair_id = ?3 AND id IN (?4, ?5)
                      AND status = 'sending'
                    "#,
                    params![to_status, message, pair_id, id1, id2],
                )
                .map_err(|e| {
                    transition_log(
                        pair_id,
                        transition,
                        "sending",
                        to_status,
                        &format!("SQL xml: {e}"),
                    )
                })?;
            if files != 2 {
                return Err(transition_log(
                    pair_id,
                    transition,
                    "sending",
                    to_status,
                    &format!("xml_files UPDATE rows={files} (kỳ vọng 2, status=sending)"),
                ));
            }
            assert_exactly_two_files_for_pair(
                tx, pair_id, id1, id2, transition, "sending", to_status,
            )?;
        }
        _ => {
            return Err(transition_log(
                pair_id,
                transition,
                "sending",
                to_status,
                "thiếu file_id_1/2 khi fail pair đầy đủ",
            ));
        }
    }
    Ok(())
}

fn assert_exactly_two_files_for_pair(
    tx: &Transaction<'_>,
    pair_id: i64,
    id1: i64,
    id2: i64,
    transition: &str,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM xml_files WHERE pair_id = ?1",
            params![pair_id],
            |r| r.get(0),
        )
        .map_err(|e| {
            transition_log(pair_id, transition, from, to, &format!("count pair files: {e}"))
        })?;
    if count != 2 {
        return Err(transition_log(
            pair_id,
            transition,
            from,
            to,
            &format!("invariant: {count} XML gắn pair (kỳ vọng 2)"),
        ));
    }
    // Không được trùng pair_id trên file khác id1/id2 đã được count; kiểm tra hai id thuộc pair.
    for id in [id1, id2] {
        let ok: Option<i64> = tx
            .query_row(
                "SELECT id FROM xml_files WHERE id = ?1 AND pair_id = ?2",
                params![id, pair_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| transition_log(pair_id, transition, from, to, &format!("check id={id}: {e}")))?;
        if ok.is_none() {
            return Err(transition_log(
                pair_id,
                transition,
                from,
                to,
                &format!("file id={id} không thuộc pair"),
            ));
        }
    }
    Ok(())
}

/// Expected meta từ pair record cho file theo order 1/2.
pub fn expected_meta_for_order(pair: &PairRecord, order: u8) -> Result<ExpectedFileMeta, String> {
    match order {
        1 => Ok(ExpectedFileMeta {
            content_hash: pair
                .content_hash_1
                .clone()
                .ok_or_else(|| format!("pair {} thiếu content_hash_1", pair.id))?,
            patient_code: pair.patient_code.clone(),
            patient_no: pair
                .patient_no_1
                .ok_or_else(|| format!("pair {} thiếu patient_no_1", pair.id))?,
            measured_at: pair
                .measured_at_1
                .clone()
                .ok_or_else(|| format!("pair {} thiếu measured_at_1", pair.id))?,
        }),
        2 => Ok(ExpectedFileMeta {
            content_hash: pair
                .content_hash_2
                .clone()
                .ok_or_else(|| format!("pair {} thiếu content_hash_2", pair.id))?,
            patient_code: pair.patient_code.clone(),
            patient_no: pair
                .patient_no_2
                .ok_or_else(|| format!("pair {} thiếu patient_no_2", pair.id))?,
            measured_at: pair
                .measured_at_2
                .clone()
                .ok_or_else(|| format!("pair {} thiếu measured_at_2", pair.id))?,
        }),
        _ => Err(format!("order không hợp lệ: {order}")),
    }
}

fn lock_conn(db: &AppDb) -> Result<MutexGuard<'_, Connection>, String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn insert_waiting(conn: &Connection, path: &str, created_at: &str) -> i64 {
        conn.execute(
            r#"
            INSERT INTO xml_files (
              device_key, file_name, file_path, status, created_at, updated_at
            ) VALUES ('kr-800', ?1, ?2, 'waiting', ?3, datetime('now'))
            "#,
            params![format!("{path}.xml"), path, created_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn meta(file_id: i64, code: &str, no: i64, at: &str, hash: &str) -> MeasurementMeta {
        MeasurementMeta {
            file_id,
            patient_code: code.into(),
            patient_code_norm: normalize_patient_code(code),
            patient_no: no,
            measured_at: at.into(),
            content_hash: hash.into(),
        }
    }

    fn dummy_parsed(code: &str, no: i64, at: &str) -> ParsedMeasurement {
        let measured_at =
            NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M:%S").expect("test datetime");
        ParsedMeasurement {
            xml_patient_id: Some(code.into()),
            patient_no: no,
            measured_at,
            right: ParsedEye {
                sphere: 0.25,
                cylinder: -1.0,
                axis: 165,
            },
            left: ParsedEye {
                sphere: 1.25,
                cylinder: -1.75,
                axis: 176,
            },
            machine_no: None,
        }
    }

    fn save_meta(db: &AppDb, m: &MeasurementMeta) {
        let parsed = dummy_parsed(&m.patient_code, m.patient_no, &m.measured_at);
        save_measurement_meta(db, m, &parsed).unwrap();
    }

    #[test]
    fn orders_by_measured_at_and_patient_no_not_arrival() {
        let early = meta(2, "HCM1", 10, "2026-07-15 10:00:00", "h1");
        let late = meta(1, "HCM1", 20, "2026-07-15 11:00:00", "h2");
        // file id đảo nhưng order vẫn đúng metadata
        let ordered = order_measurements(late.clone(), early.clone()).expect("order");
        assert_eq!(ordered.first.file_id, 2);
        assert_eq!(ordered.second.file_id, 1);
    }

    #[test]
    fn pairing_error_when_timestamp_up_but_patient_no_down() {
        let a = meta(1, "HCM1", 20, "2026-07-15 10:00:00", "h1");
        let b = meta(2, "HCM1", 10, "2026-07-15 11:00:00", "h2");
        let err = order_measurements(a, b).unwrap_err();
        assert!(err.contains("mâu thuẫn") || err.contains("bằng nhau"), "{err}");
    }

    #[test]
    fn first_file_awaits_pair_no_partner() {
        let db = db::open_memory_for_test().unwrap();
        let id = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/a", "2026-07-15 15:12:40")
        };
        let m = meta(id, "HCM2607150275", 1694, "2026-07-15 15:12:40", "hash1");
        save_meta(&db, &m);
        match resolve_pair_for_measurement(&db, &m).unwrap() {
            PairResolve::AwaitingSecond { patient_code, .. } => {
                assert_eq!(patient_code, "HCM2607150275");
            }
            other => panic!("expected awaiting, got {other:?}"),
        }
        let status: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row("SELECT status FROM xml_files WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(status, "awaiting_pair");
    }

    #[test]
    fn second_file_completes_pair_even_if_arrived_first_in_db() {
        let db = db::open_memory_for_test().unwrap();
        // Lưu file muộn trước (id nhỏ hơn) — vẫn order theo metadata.
        let (id_late, id_early) = {
            let conn = db.conn.lock().unwrap();
            let late = insert_waiting(&conn, "/tmp/late", "2026-07-15 16:00:00");
            let early = insert_waiting(&conn, "/tmp/early", "2026-07-15 15:00:00");
            (late, early)
        };
        let late = meta(id_late, "HCM2607150275", 1700, "2026-07-15 16:00:00", "hash_late");
        let early = meta(id_early, "HCM2607150275", 1694, "2026-07-15 15:00:00", "hash_early");
        save_meta(&db, &late);
        save_meta(&db, &early);

        assert!(matches!(
            resolve_pair_for_measurement(&db, &late).unwrap(),
            PairResolve::AwaitingSecond { .. }
        ));
        match resolve_pair_for_measurement(&db, &early).unwrap() {
            PairResolve::Ready { ordered, .. } => {
                assert_eq!(ordered.first.file_id, id_early);
                assert_eq!(ordered.second.file_id, id_late);
                assert_eq!(ordered.first.patient_no, 1694);
                assert_eq!(ordered.second.patient_no, 1700);
            }
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_content_of_first_is_not_second_measurement() {
        let db = db::open_memory_for_test().unwrap();
        let (id1, id_dup) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/m1", "2026-07-15 15:00:00"),
                insert_waiting(&conn, "/tmp/m1-copy", "2026-07-15 15:01:00"),
            )
        };
        let m1 = meta(id1, "HCM1", 1, "2026-07-15 15:00:00", "samehash");
        let dup = meta(id_dup, "HCM1", 2, "2026-07-15 16:00:00", "samehash");
        save_meta(&db, &m1);
        save_meta(&db, &dup);
        resolve_pair_for_measurement(&db, &m1).unwrap();
        // content_hash_already_measured should catch before resolve in pipeline;
        // resolve also treats same hash as not a real second measurement.
        assert!(content_hash_already_measured(&db, "samehash", id_dup).unwrap());
    }

    #[test]
    fn third_file_after_ready_pair_is_extra() {
        let db = db::open_memory_for_test().unwrap();
        let (a, b, c) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/1", "2026-07-15 10:00:00"),
                insert_waiting(&conn, "/tmp/2", "2026-07-15 11:00:00"),
                insert_waiting(&conn, "/tmp/3", "2026-07-15 12:00:00"),
            )
        };
        let m1 = meta(a, "HCM1", 1, "2026-07-15 10:00:00", "h1");
        let m2 = meta(b, "HCM1", 2, "2026-07-15 11:00:00", "h2");
        let m3 = meta(c, "HCM1", 3, "2026-07-15 12:00:00", "h3");
        for m in [&m1, &m2, &m3] {
            save_meta(&db, m);
        }
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("expected ready, got {other:?}"),
        };
        let owner = "test-owner-1";
        assert!(claim_pair_for_send(&db, pair_id, owner).unwrap());
        finish_pair_success(&db, pair_id, "{}", owner).unwrap();
        match resolve_pair_for_measurement(&db, &m3).unwrap() {
            PairResolve::ExtraMeasurement { .. } => {}
            other => panic!("expected extra, got {other:?}"),
        }
        let status: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row("SELECT status FROM xml_files WHERE id = ?1", params![c], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(status, "extra_measurement");
    }

    #[test]
    fn concurrent_claim_pair_only_one_wins() {
        let db = db::open_memory_for_test().unwrap();
        let (a, b) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/c1", "2026-07-15 10:00:00"),
                insert_waiting(&conn, "/tmp/c2", "2026-07-15 11:00:00"),
            )
        };
        let m1 = meta(a, "HCMX", 1, "2026-07-15 10:00:00", "ch1");
        let m2 = meta(b, "HCMX", 2, "2026-07-15 11:00:00", "ch2");
        save_meta(&db, &m1);
        save_meta(&db, &m2);
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        let owner = "test-owner-claim";
        let first = claim_pair_for_send(&db, pair_id, owner).unwrap();
        let second = claim_pair_for_send(&db, pair_id, "other-owner").unwrap();
        assert!(first);
        assert!(!second);
    }

    #[test]
    fn live_lease_not_recovered_by_other_instance() {
        let db = db::open_memory_for_test().unwrap();
        let (a, b) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/live1", "2026-07-15 10:00:00"),
                insert_waiting(&conn, "/tmp/live2", "2026-07-15 11:00:00"),
            )
        };
        let m1 = meta(a, "HCMLIVE", 1, "2026-07-15 10:00:00", "lv1");
        let m2 = meta(b, "HCMLIVE", 2, "2026-07-15 11:00:00", "lv2");
        save_meta(&db, &m1);
        save_meta(&db, &m2);
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        assert!(claim_pair_for_send(&db, pair_id, "owner-a").unwrap());
        // Lease còn hiệu lực → không recover.
        assert_eq!(recover_expired_sending_pairs(&db).unwrap(), 0);
        // Owner B không fail được pair của A.
        assert!(fail_pair(&db, pair_id, "send_error", "nope", "owner-b").is_err());
        // Owner A finish OK.
        finish_pair_success(&db, pair_id, "{}", "owner-a").unwrap();
        // processed không bị fail path ghi đè.
        assert!(fail_pair(&db, pair_id, "send_error", "stale", "owner-a").is_err());
        let status: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT status FROM measurement_pairs WHERE id = ?1",
                params![pair_id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(status, "processed");
        assert!(retryable_pairs(&db).unwrap().is_empty());
    }

    #[test]
    fn recover_orphaned_sending_makes_pair_retryable() {
        let db = db::open_memory_for_test().unwrap();
        let (a, b) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/or1", "2026-07-15 10:00:00"),
                insert_waiting(&conn, "/tmp/or2", "2026-07-15 11:00:00"),
            )
        };
        let m1 = meta(a, "HCMOR", 1, "2026-07-15 10:00:00", "oh1");
        let m2 = meta(b, "HCMOR", 2, "2026-07-15 11:00:00", "oh2");
        save_meta(&db, &m1);
        save_meta(&db, &m2);
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        assert!(claim_pair_for_send(&db, pair_id, "owner-crash").unwrap());
        // Simulate crash: still sending, not in retryable while lease live.
        assert!(retryable_pairs(&db).unwrap().is_empty());
        // Force lease hết hạn (legacy NULL hoặc quá khứ).
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE measurement_pairs SET sending_lease_until = datetime('now', '-1 seconds') WHERE id = ?1",
                params![pair_id],
            )
            .unwrap();
        }
        let n = recover_expired_sending_pairs(&db).unwrap();
        assert_eq!(n, 1);
        assert_eq!(retryable_pairs(&db).unwrap(), vec![pair_id]);
        // Có thể claim lại và finish.
        assert!(claim_pair_for_send(&db, pair_id, "owner-retry").unwrap());
        save_pair_request(&db, pair_id, 7, r#"{"ok":true}"#, "owner-retry").unwrap();
        finish_pair_success(&db, pair_id, "{}", "owner-retry").unwrap();
    }

    #[test]
    fn snapshot_preferred_over_disk_mutation() {
        let db = db::open_memory_for_test().unwrap();
        let id = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/snap", "2026-07-15 10:00:00")
        };
        let m = meta(id, "HCMS", 1, "2026-07-15 10:00:00", "shash");
        save_meta(&db, &m);
        let snap = load_snapshot(&db, id).unwrap().expect("snapshot saved");
        assert_eq!(snap.xml_patient_id.as_deref(), Some("HCMS"));
        assert_eq!(snap.right.sphere, 0.25);
        assert_eq!(snap.content_hash, "shash");
        // load_or_rehydrate dùng snapshot, không cần file disk.
        let exp = ExpectedFileMeta {
            content_hash: "shash".into(),
            patient_code: "HCMS".into(),
            patient_no: 1,
            measured_at: "2026-07-15 10:00:00".into(),
        };
        let again = load_or_rehydrate_snapshot(&db, id, &exp).unwrap();
        assert_eq!(again.right.axis, 165);
    }

    #[test]
    fn filename_patient_code_wins_over_xml_patient_id() {
        let parsed = dummy_parsed("5548", 1, "2026-07-15 10:00:00");
        let meta = meta_from_parsed(1, &parsed, "hash", "HCM2607150275".into());
        assert_eq!(meta.patient_code, "HCM2607150275");
        assert_eq!(meta.patient_code_norm, "hcm2607150275");
        assert_eq!(parsed.xml_patient_id.as_deref(), Some("5548"));
    }

    #[test]
    fn reconcile_repairs_retry_code_and_is_idempotent_without_touching_processed() {
        let db = db::open_memory_for_test().unwrap();
        let (pending, processed) = {
            let conn = db.conn.lock().unwrap();
            conn.execute("INSERT INTO xml_files (device_key,file_name,file_path,status,patient_code,created_at,updated_at) VALUES ('kr-800',?1,'/tmp/retry.xml','patient_not_found','5548',datetime('now'),datetime('now'))", params!["HCM2607150275_20260715_151240_TOPCON_KR-800_4780634.xml"]).unwrap();
            let pending = conn.last_insert_rowid();
            conn.execute("INSERT INTO xml_files (device_key,file_name,file_path,status,patient_code,created_at,updated_at) VALUES ('kr-800',?1,'/tmp/done.xml','processed','5548',datetime('now'),datetime('now'))", params!["HCM2607150999_20260715_151240_TOPCON_KR-800_4780634.xml"]).unwrap();
            (pending, conn.last_insert_rowid())
        };
        assert_eq!(reconcile_pending_patient_codes(&db).unwrap().reconciled_files, 1);
        assert_eq!(reconcile_pending_patient_codes(&db).unwrap().reconciled_files, 0);
        let conn = db.conn.lock().unwrap();
        let pending_code: String = conn.query_row("SELECT patient_code FROM xml_files WHERE id=?1", params![pending], |row| row.get(0)).unwrap();
        let processed_code: String = conn.query_row("SELECT patient_code FROM xml_files WHERE id=?1", params![processed], |row| row.get(0)).unwrap();
        assert_eq!(pending_code, "HCM2607150275");
        assert_eq!(processed_code, "5548");
    }

    #[test]
    fn restart_keeps_awaiting_pair_state() {
        let db = db::open_memory_for_test().unwrap();
        let id = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/restart", "2026-07-15 15:00:00")
        };
        let m = meta(id, "HCMR", 5, "2026-07-15 15:00:00", "rh1");
        save_meta(&db, &m);
        resolve_pair_for_measurement(&db, &m).unwrap();
        // Giả lập "restart": chỉ đọc lại DB.
        let status: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT status FROM xml_files WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(status, "awaiting_pair");
        let pair_status: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT status FROM measurement_pairs WHERE patient_code_norm = 'hcmr'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(pair_status, "awaiting_pair");
    }

    #[test]
    fn send_error_retry_reuses_same_pair_not_new() {
        let db = db::open_memory_for_test().unwrap();
        let (a, b) = {
            let conn = db.conn.lock().unwrap();
            (
                insert_waiting(&conn, "/tmp/r1", "2026-07-15 10:00:00"),
                insert_waiting(&conn, "/tmp/r2", "2026-07-15 11:00:00"),
            )
        };
        let m1 = meta(a, "HCMRETRY", 1, "2026-07-15 10:00:00", "rhA");
        let m2 = meta(b, "HCMRETRY", 2, "2026-07-15 11:00:00", "rhB");
        save_meta(&db, &m1);
        save_meta(&db, &m2);
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        let owner = "owner-retry-keep";
        assert!(claim_pair_for_send(&db, pair_id, owner).unwrap());
        save_pair_request(&db, pair_id, 42, r#"{"kept":true}"#, owner).unwrap();
        fail_pair(&db, pair_id, "send_error", "HIS 500", owner).unwrap();

        let retries = retryable_pairs(&db).unwrap();
        assert_eq!(retries, vec![pair_id]);

        // Không tạo pair mới khi process lại cùng metadata.
        let pair_count: i64 = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM measurement_pairs WHERE patient_code_norm = 'hcmretry'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(pair_count, 1);

        let payload: String = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT request_payload FROM measurement_pairs WHERE id = ?1",
                params![pair_id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(payload, r#"{"kept":true}"#);
        assert!(claim_pair_for_send(&db, pair_id, "owner-retry-2").unwrap());
    }

    #[test]
    fn second_file_after_restart_completes_awaiting_pair() {
        let db = db::open_memory_for_test().unwrap();
        let id1 = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/rs1", "2026-07-15 10:00:00")
        };
        let m1 = meta(id1, "HCMRS", 10, "2026-07-15 10:00:00", "rs1");
        save_meta(&db, &m1);
        resolve_pair_for_measurement(&db, &m1).unwrap();

        // "Restart" — file 2 tới sau.
        let id2 = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/rs2", "2026-07-16 08:00:00")
        };
        let m2 = meta(id2, "HCMRS", 11, "2026-07-16 08:00:00", "rs2");
        save_meta(&db, &m2);
        match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { ordered, .. } => {
                assert_eq!(ordered.first.file_id, id1);
                assert_eq!(ordered.second.file_id, id2);
            }
            other => panic!("expected ready after restart, got {other:?}"),
        }
    }

    #[test]
    fn file_send_keeps_pair_open_then_reuses_cached_service_for_second_file() {
        let db = db::open_memory_for_test().unwrap();
        let (first_id, second_id) = {
            let conn = db.conn.lock().unwrap();
            (insert_waiting(&conn, "/tmp/new-first", "2026-07-15 10:00:00"), insert_waiting(&conn, "/tmp/new-second", "2026-07-15 11:00:00"))
        };
        let first = meta(first_id, "HCMNEW", 1, "2026-07-15 10:00:00", "new1");
        let second = meta(second_id, "HCMNEW", 2, "2026-07-15 11:00:00", "new2");
        save_meta(&db, &first);
        save_meta(&db, &second);
        let pair_id = match resolve_pair_for_measurement(&db, &first).unwrap() {
            PairResolve::AwaitingSecond { pair_id, .. } => pair_id,
            other => panic!("unexpected {other:?}"),
        };
        assert!(claim_file_for_send(&db, first_id, "owner-1").unwrap());
        save_file_request(&db, first_id, pair_id, 1103, 3462, r#"{"first":true}"#, "owner-1").unwrap();
        finish_file_success(&db, first_id, "{}", "owner-1").unwrap();
        assert_eq!(load_pair_by_id(&db, pair_id).unwrap().unwrap().status, "awaiting_pair");
        assert_eq!(load_pair_by_id(&db, pair_id).unwrap().unwrap().dv_kham_id, Some(3462));
        let ready = resolve_pair_for_measurement(&db, &second).unwrap();
        assert!(matches!(ready, PairResolve::Ready { .. }));
        assert!(claim_file_for_send(&db, second_id, "owner-2").unwrap());
        let second_record = load_file_send_record(&db, second_id).unwrap().unwrap();
        assert_eq!(second_record.dv_kham_id, None, "cache remains on pair until request is persisted");
        save_file_request(&db, second_id, pair_id, 1103, 3462, r#"{"second":true}"#, "owner-2").unwrap();
        finish_file_success(&db, second_id, "{}", "owner-2").unwrap();
        assert_eq!(load_pair_by_id(&db, pair_id).unwrap().unwrap().status, "processed");
    }

    #[test]
    fn live_file_lease_blocks_second_claim_and_expired_lease_recovers() {
        let db = db::open_memory_for_test().unwrap();
        let id = { let conn = db.conn.lock().unwrap(); insert_waiting(&conn, "/tmp/lease-file", "2026-07-15 10:00:00") };
        let m = meta(id, "HCMLEASEFILE", 1, "2026-07-15 10:00:00", "lease-file");
        save_meta(&db, &m);
        resolve_pair_for_measurement(&db, &m).unwrap();
        assert!(claim_file_for_send(&db, id, "owner-a").unwrap());
        assert!(!claim_file_for_send(&db, id, "owner-b").unwrap());
        { let conn = db.conn.lock().unwrap(); conn.execute("UPDATE xml_files SET sending_lease_until=datetime('now','-1 seconds') WHERE id=?1", params![id]).unwrap(); }
        assert_eq!(recover_expired_sending_files(&db).unwrap(), 1);
        assert!(claim_file_for_send(&db, id, "owner-b").unwrap());
    }
}
