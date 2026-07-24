//! Ghép cặp hai lần đo KR-800 theo Patient.ID + measuredAt + Patient.No.
//!
//! Invariant:
//! - Một XML thuộc tối đa một cặp.
//! - Một `patient_code_norm` chỉ có tối đa một cặp đang mở (chưa processed).
//! - Một cặp có đúng hai content_hash khác nhau khi đầy đủ.
//! - Claim/cập nhật hai file trong transaction để tránh double PUT.

use crate::db::AppDb;
use crate::xml_parser::{format_measured_at, ParsedMeasurement};
use chrono::NaiveDateTime;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::sync::MutexGuard;

pub const DEVICE_KEY: &str = "kr-800";

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

pub fn meta_from_parsed(file_id: i64, parsed: &ParsedMeasurement, content_hash: &str) -> MeasurementMeta {
    MeasurementMeta {
        file_id,
        patient_code: parsed.patient_id.clone(),
        patient_code_norm: normalize_patient_code(&parsed.patient_id),
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

pub fn save_measurement_meta(db: &AppDb, meta: &MeasurementMeta) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          content_hash = ?1,
          patient_code = ?2,
          patient_no = ?3,
          measured_at = ?4,
          updated_at = datetime('now')
        WHERE id = ?5
        "#,
        params![
            meta.content_hash,
            meta.patient_code,
            meta.patient_no,
            meta.measured_at,
            meta.file_id
        ],
    )
    .map_err(|e| format!("Lưu metadata đo thất bại: {e}"))?;
    Ok(())
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
        attach_file_to_pair(&tx, meta.file_id, pair_id, None, "awaiting_pair")?;
        tx.commit()
            .map_err(|e| format!("Commit awaiting_pair thất bại: {e}"))?;
        Ok(PairResolve::AwaitingSecond {
            pair_id,
            patient_code: meta.patient_code.clone(),
        })
    }
}

fn complete_pair_rows(tx: &Transaction<'_>, pair_id: i64, ordered: &OrderedPair) -> Result<(), String> {
    tx.execute(
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
        WHERE id = ?9
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

    attach_file_to_pair(tx, ordered.first.file_id, pair_id, Some(1), "pairing")?;
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
        WHERE id = ?3
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
               status, request_payload, nb_dot_dieu_tri_id
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
               status, request_payload, nb_dot_dieu_tri_id
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

/// Claim cặp để gửi HIS — chỉ một task thắng.
pub fn claim_pair_for_send(db: &AppDb, pair_id: i64) -> Result<bool, String> {
    let conn = lock_conn(db)?;
    let changed = conn
        .execute(
            r#"
            UPDATE measurement_pairs SET
              status = 'sending',
              error_message = NULL,
              updated_at = datetime('now')
            WHERE id = ?1
              AND status IN (
                'pairing', 'send_error', 'patient_not_found',
                'treatment_ambiguous', 'mapping_error'
              )
            "#,
            params![pair_id],
        )
        .map_err(|e| format!("Claim pair id={pair_id} thất bại: {e}"))?;
    if changed == 1 {
        conn.execute(
            r#"
            UPDATE xml_files SET
              status = 'sending',
              error_message = NULL,
              updated_at = datetime('now')
            WHERE pair_id = ?1
            "#,
            params![pair_id],
        )
        .map_err(|e| format!("Claim files of pair thất bại: {e}"))?;
    }
    Ok(changed == 1)
}

pub fn save_pair_request(
    db: &AppDb,
    pair_id: i64,
    treatment_id: i64,
    payload_json: &str,
) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        UPDATE measurement_pairs SET
          nb_dot_dieu_tri_id = ?1,
          request_payload = ?2,
          status = 'sending',
          updated_at = datetime('now')
        WHERE id = ?3
        "#,
        params![treatment_id, payload_json, pair_id],
    )
    .map_err(|e| format!("Lưu request pair thất bại: {e}"))?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          nb_dot_dieu_tri_id = ?1,
          request_payload = ?2,
          status = 'sending',
          updated_at = datetime('now')
        WHERE pair_id = ?3
        "#,
        params![treatment_id, payload_json, pair_id],
    )
    .map_err(|e| format!("Lưu request files pair thất bại: {e}"))?;
    Ok(())
}

pub fn finish_pair_success(db: &AppDb, pair_id: i64, response: &str) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        UPDATE measurement_pairs SET
          status = 'processed',
          error_message = NULL,
          response_payload = ?1,
          processed_at = datetime('now'),
          updated_at = datetime('now')
        WHERE id = ?2
        "#,
        params![response, pair_id],
    )
    .map_err(|e| format!("Hoàn tất pair thất bại: {e}"))?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          status = 'processed',
          error_message = NULL,
          response_payload = ?1,
          processed_at = datetime('now'),
          updated_at = datetime('now')
        WHERE pair_id = ?2
        "#,
        params![response, pair_id],
    )
    .map_err(|e| format!("Hoàn tất files pair thất bại: {e}"))?;
    Ok(())
}

pub fn fail_pair(db: &AppDb, pair_id: i64, status: &str, message: &str) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        UPDATE measurement_pairs SET
          status = ?1,
          error_message = ?2,
          updated_at = datetime('now')
        WHERE id = ?3
        "#,
        params![status, message, pair_id],
    )
    .map_err(|e| format!("Lưu lỗi pair thất bại: {e}"))?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          status = ?1,
          error_message = ?2,
          updated_at = datetime('now')
        WHERE pair_id = ?3
        "#,
        params![status, message, pair_id],
    )
    .map_err(|e| format!("Lưu lỗi files pair thất bại: {e}"))?;
    Ok(())
}

pub fn increment_pair_attempt(db: &AppDb, pair_id: i64) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        UPDATE measurement_pairs SET
          attempt_count = attempt_count + 1,
          updated_at = datetime('now')
        WHERE id = ?1
        "#,
        params![pair_id],
    )
    .map_err(|e| format!("Tăng attempt_count pair thất bại: {e}"))?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          attempt_count = attempt_count + 1,
          updated_at = datetime('now')
        WHERE pair_id = ?1
        "#,
        params![pair_id],
    )
    .map_err(|e| format!("Tăng attempt_count files pair thất bại: {e}"))?;
    Ok(())
}

/// Cặp cần retry gửi (giữ nguyên hai file + payload nếu có).
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
        save_measurement_meta(&db, &m).unwrap();
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
        save_measurement_meta(&db, &late).unwrap();
        save_measurement_meta(&db, &early).unwrap();

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
        save_measurement_meta(&db, &m1).unwrap();
        save_measurement_meta(&db, &dup).unwrap();
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
            save_measurement_meta(&db, m).unwrap();
        }
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("expected ready, got {other:?}"),
        };
        finish_pair_success(&db, pair_id, "{}").unwrap();
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
        save_measurement_meta(&db, &m1).unwrap();
        save_measurement_meta(&db, &m2).unwrap();
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        let first = claim_pair_for_send(&db, pair_id).unwrap();
        let second = claim_pair_for_send(&db, pair_id).unwrap();
        assert!(first);
        assert!(!second);
    }

    #[test]
    fn restart_keeps_awaiting_pair_state() {
        let db = db::open_memory_for_test().unwrap();
        let id = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/restart", "2026-07-15 15:00:00")
        };
        let m = meta(id, "HCMR", 5, "2026-07-15 15:00:00", "rh1");
        save_measurement_meta(&db, &m).unwrap();
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
        save_measurement_meta(&db, &m1).unwrap();
        save_measurement_meta(&db, &m2).unwrap();
        resolve_pair_for_measurement(&db, &m1).unwrap();
        let pair_id = match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { pair_id, .. } => pair_id,
            other => panic!("{other:?}"),
        };
        save_pair_request(&db, pair_id, 42, r#"{"kept":true}"#).unwrap();
        fail_pair(&db, pair_id, "send_error", "HIS 500").unwrap();

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
        assert!(claim_pair_for_send(&db, pair_id).unwrap());
    }

    #[test]
    fn second_file_after_restart_completes_awaiting_pair() {
        let db = db::open_memory_for_test().unwrap();
        let id1 = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/rs1", "2026-07-15 10:00:00")
        };
        let m1 = meta(id1, "HCMRS", 10, "2026-07-15 10:00:00", "rs1");
        save_measurement_meta(&db, &m1).unwrap();
        resolve_pair_for_measurement(&db, &m1).unwrap();

        // "Restart" — file 2 tới sau.
        let id2 = {
            let conn = db.conn.lock().unwrap();
            insert_waiting(&conn, "/tmp/rs2", "2026-07-16 08:00:00")
        };
        let m2 = meta(id2, "HCMRS", 11, "2026-07-16 08:00:00", "rs2");
        save_measurement_meta(&db, &m2).unwrap();
        match resolve_pair_for_measurement(&db, &m2).unwrap() {
            PairResolve::Ready { ordered, .. } => {
                assert_eq!(ordered.first.file_id, id1);
                assert_eq!(ordered.second.file_id, id2);
            }
            other => panic!("expected ready after restart, got {other:?}"),
        }
    }
}
