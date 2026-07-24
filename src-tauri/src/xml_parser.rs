use chrono::NaiveDateTime;
use encoding_rs::SHIFT_JIS;
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EyeRefraction {
    pub sphere: Option<String>,
    pub cylinder: Option<String>,
    pub axis: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XmlPreview {
    pub file_name: String,
    pub patient_id: Option<String>,
    pub measured_at: Option<String>,
    pub right: EyeRefraction,
    pub left: EyeRefraction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEye {
    pub sphere: f64,
    pub cylinder: f64,
    pub axis: i64,
}

/// Kết quả đo REF đầy đủ từ một file KR-800 (cả hai mắt).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMeasurement {
    pub patient_id: String,
    /// `nsCommon:Patient/nsCommon:No.` — dùng ghép thứ tự lần đo.
    pub patient_no: i64,
    /// Date + Time trong XML; nguồn chính để so sánh trước/sau.
    pub measured_at: NaiveDateTime,
    pub right: ParsedEye,
    pub left: ParsedEye,
    pub machine_no: Option<String>,
}

pub fn preview_file(path: &str) -> Result<XmlPreview, String> {
    let bytes = fs::read(path).map_err(|error| format!("Không đọc được XML: {error}"))?;
    let parsed = parse_measurement(&bytes)?;
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string();
    Ok(XmlPreview {
        file_name,
        patient_id: Some(parsed.patient_id),
        measured_at: Some(format_measured_at(parsed.measured_at)),
        right: eye_preview(&parsed.right),
        left: eye_preview(&parsed.left),
    })
}

pub fn parse_measurement(bytes: &[u8]) -> Result<ParsedMeasurement, String> {
    let xml = decode_xml(bytes)?;
    let document = Document::parse(&xml).map_err(|error| format!("XML không hợp lệ: {error}"))?;
    let common = document
        .descendants()
        .find(|node| is_element(*node, "Common"))
        .ok_or_else(|| "Không tìm thấy Common trong XML.".to_string())?;
    let patient = common
        .children()
        .find(|node| is_element(*node, "Patient"))
        .ok_or_else(|| "Không tìm thấy Common.Patient trong XML.".to_string())?;
    let patient_id = child_text(patient, "ID")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Thiếu hoặc rỗng Common.Patient.ID trong XML.".to_string())?;

    let patient_no_raw = child_text(patient, "No.")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Thiếu Common.Patient.No. trong XML.".to_string())?;
    let patient_no = patient_no_raw.parse::<i64>().map_err(|_| {
        format!("Common.Patient.No. không phải số nguyên hợp lệ: {patient_no_raw}")
    })?;

    let date = child_text(common, "Date")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Thiếu Common.Date trong XML.".to_string())?;
    let time = child_text(common, "Time")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Thiếu Common.Time trong XML.".to_string())?;
    let measured_at = parse_measured_at(&date, &time)?;

    let machine_no = document
        .descendants()
        .find(|node| is_element(*node, "Company"))
        .and_then(|company| child_text(company, "No."));

    let ref_measure = document
        .descendants()
        .find(|node| is_element(*node, "Measure") && node.attribute("type") == Some("REF"))
        .ok_or_else(|| "Không tìm thấy REF Measure trong XML.".to_string())?;
    let ref_node = ref_measure
        .children()
        .find(|node| is_element(*node, "REF"))
        .ok_or_else(|| "Không tìm thấy REF Measure/REF trong XML.".to_string())?;

    Ok(ParsedMeasurement {
        patient_id,
        patient_no,
        measured_at,
        right: parse_eye(ref_node, "R")?,
        left: parse_eye(ref_node, "L")?,
        machine_no,
    })
}

/// Chuẩn hoá Date+Time XML → `YYYY-MM-DD HH:mm:ss` so sánh được.
pub fn format_measured_at(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn parse_measured_at(date: &str, time: &str) -> Result<NaiveDateTime, String> {
    let date = date.trim();
    let time = time.trim();
    let candidates = [
        format!("{date} {time}"),
        format!("{date}T{time}"),
    ];
    for raw in candidates {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S") {
            return Ok(dt);
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M") {
            return Ok(dt);
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(&raw, "%Y/%m/%d %H:%M:%S") {
            return Ok(dt);
        }
    }
    Err(format!(
        "Common.Date/Time sai định dạng (nhận Date={date:?}, Time={time:?})."
    ))
}

fn parse_eye(ref_node: Node<'_, '_>, side: &str) -> Result<ParsedEye, String> {
    let side_node = ref_node
        .children()
        .find(|node| is_element(*node, side))
        .ok_or_else(|| format!("Không tìm thấy mắt {side} trong REF."))?;
    let median = side_node
        .children()
        .find(|node| is_element(*node, "Median"))
        .ok_or_else(|| format!("Không tìm thấy Median cho mắt {side}."))?;
    let sphere = parse_decimal(&required_child_text(median, "Sphere", side)?, "Sphere")?;
    let cylinder = parse_decimal(&required_child_text(median, "Cylinder", side)?, "Cylinder")?;
    let axis_raw = parse_decimal(&required_child_text(median, "Axis", side)?, "Axis")?;
    if (axis_raw.fract()).abs() > 1e-9 {
        return Err(format!("Axis mắt {side} không phải số nguyên: {axis_raw}"));
    }
    Ok(ParsedEye {
        sphere,
        cylinder,
        axis: axis_raw as i64,
    })
}

fn decode_xml(bytes: &[u8]) -> Result<String, String> {
    let header = String::from_utf8_lossy(&bytes[..bytes.len().min(160)]).to_ascii_lowercase();
    if header.contains("shift-jis") || header.contains("shift_jis") {
        let (decoded, _, had_errors) = SHIFT_JIS.decode(bytes);
        if had_errors {
            return Err("XML Shift-JIS chứa byte không hợp lệ.".into());
        }
        return Ok(decoded.into_owned());
    }
    String::from_utf8(bytes.to_vec()).map_err(|error| format!("XML không phải UTF-8: {error}"))
}

fn required_child_text(node: Node<'_, '_>, name: &str, side: &str) -> Result<String, String> {
    child_text(node, name).ok_or_else(|| format!("Thiếu {name} Median cho mắt {side}."))
}

fn child_text(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| is_element(*child, name))
        .and_then(|child| child.text())
        .map(|value| value.trim().to_string())
}

fn is_element(node: Node<'_, '_>, local_name: &str) -> bool {
    node.is_element() && node.tag_name().name() == local_name
}

fn parse_decimal(value: &str, field: &str) -> Result<f64, String> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{field} không phải số: {value}"))?;
    Ok(if parsed == -0.0 { 0.0 } else { parsed })
}

fn eye_preview(eye: &ParsedEye) -> EyeRefraction {
    EyeRefraction {
        sphere: Some(format!("{:.2}", eye.sphere)),
        cylinder: Some(format!("{:.2}", eye.cylinder)),
        axis: Some(eye.axis.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture khớp nghiệp vụ KR-800 (Patient.ID / No. / R+L Median).
    fn sample_xml_measurement1() -> &'static [u8] {
        br#"<?xml version="1.0"?><Ophthalmology xmlns:nsCommon="urn:c" xmlns:r="urn:r"><nsCommon:Common><nsCommon:Date>2026-07-15</nsCommon:Date><nsCommon:Time>15:12:40</nsCommon:Time><nsCommon:Patient><nsCommon:ID>HCM2607150275</nsCommon:ID><nsCommon:No.>1694</nsCommon:No.></nsCommon:Patient></nsCommon:Common><nsCommon:Company><nsCommon:No.>4780634</nsCommon:No.></nsCommon:Company><r:Measure type="REF"><r:REF><r:R><r:Median><r:Sphere>+0.25</r:Sphere><r:Cylinder>-1.00</r:Cylinder><r:Axis>165.0</r:Axis></r:Median></r:R><r:L><r:Median><r:Sphere>+1.25</r:Sphere><r:Cylinder>-1.75</r:Cylinder><r:Axis>176</r:Axis></r:Median></r:L></r:REF></r:Measure></Ophthalmology>"#
    }

    #[test]
    fn parses_patient_id_no_measured_at_and_ref_medians() {
        let parsed = parse_measurement(sample_xml_measurement1()).expect("parse fixture");
        assert_eq!(parsed.patient_id, "HCM2607150275");
        assert_eq!(parsed.patient_no, 1694);
        assert_eq!(
            format_measured_at(parsed.measured_at),
            "2026-07-15 15:12:40"
        );
        assert_eq!(parsed.machine_no.as_deref(), Some("4780634"));
        assert_eq!(
            parsed.right,
            ParsedEye {
                sphere: 0.25,
                cylinder: -1.0,
                axis: 165
            }
        );
        assert_eq!(
            parsed.left,
            ParsedEye {
                sphere: 1.25,
                cylinder: -1.75,
                axis: 176
            }
        );
    }

    #[test]
    fn rejects_missing_patient_no() {
        let xml = br#"<?xml version="1.0"?><Ophthalmology xmlns:c="urn:c" xmlns:r="urn:r"><c:Common><c:Date>2026-07-15</c:Date><c:Time>15:12:40</c:Time><c:Patient><c:ID>HCM2607150275</c:ID></c:Patient></c:Common><r:Measure type="REF"><r:REF><r:R><r:Median><r:Sphere>0.25</r:Sphere><r:Cylinder>-1.00</r:Cylinder><r:Axis>165</r:Axis></r:Median></r:R><r:L><r:Median><r:Sphere>1.25</r:Sphere><r:Cylinder>-1.75</r:Cylinder><r:Axis>176</r:Axis></r:Median></r:L></r:REF></r:Measure></Ophthalmology>"#;
        let err = parse_measurement(xml).unwrap_err();
        assert!(err.contains("No."), "{err}");
    }

    #[test]
    fn rejects_missing_date() {
        let xml = br#"<?xml version="1.0"?><Ophthalmology xmlns:c="urn:c" xmlns:r="urn:r"><c:Common><c:Time>15:12:40</c:Time><c:Patient><c:ID>X</c:ID><c:No.>1</c:No.></c:Patient></c:Common><r:Measure type="REF"><r:REF><r:R><r:Median><r:Sphere>0</r:Sphere><r:Cylinder>0</r:Cylinder><r:Axis>0</r:Axis></r:Median></r:R><r:L><r:Median><r:Sphere>0</r:Sphere><r:Cylinder>0</r:Cylinder><r:Axis>0</r:Axis></r:Median></r:L></r:REF></r:Measure></Ophthalmology>"#;
        let err = parse_measurement(xml).unwrap_err();
        assert!(err.contains("Date"), "{err}");
    }

    #[test]
    fn parses_patient_and_ref_medians_legacy_shape() {
        let xml = br#"<?xml version="1.0"?><Ophthalmology xmlns:c="urn:c" xmlns:r="urn:r"><c:Common><c:Date>2026-07-07</c:Date><c:Time>14:50:00</c:Time><c:Patient><c:ID>HCM2607070269</c:ID><c:No.>100</c:No.></c:Patient></c:Common><r:Measure type="REF"><r:REF><r:R><r:Median><r:Sphere>+1.750</r:Sphere><r:Cylinder>-1.00</r:Cylinder><r:Axis>178.0</r:Axis></r:Median></r:R><r:L><r:Median><r:Sphere>0.75</r:Sphere><r:Cylinder>-0.25</r:Cylinder><r:Axis>35</r:Axis></r:Median></r:L></r:REF></r:Measure></Ophthalmology>"#;
        let parsed = parse_measurement(xml).expect("parse fixture");
        assert_eq!(parsed.patient_id, "HCM2607070269");
        assert_eq!(parsed.patient_no, 100);
        assert_eq!(
            parsed.right,
            ParsedEye {
                sphere: 1.75,
                cylinder: -1.0,
                axis: 178
            }
        );
        assert_eq!(
            parsed.left,
            ParsedEye {
                sphere: 0.75,
                cylinder: -0.25,
                axis: 35
            }
        );
    }
}
