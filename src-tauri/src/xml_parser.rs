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

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMeasurement {
    pub patient_id: String,
    pub measured_at: Option<String>,
    pub right: ParsedEye,
    pub left: ParsedEye,
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
        measured_at: parsed.measured_at,
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
        .ok_or_else(|| "Không tìm thấy Common.Patient.ID trong XML.".to_string())?;

    let measured_at = {
        let date = child_text(common, "Date");
        let time = child_text(common, "Time");
        date.zip(time).map(|(date, time)| format!("{date} {time}"))
    };

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
        measured_at,
        right: parse_eye(ref_node, "R")?,
        left: parse_eye(ref_node, "L")?,
    })
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

    #[test]
    fn parses_patient_and_ref_medians() {
        let xml = br#"<?xml version="1.0"?><Ophthalmology xmlns:c="urn:c" xmlns:r="urn:r"><c:Common><c:Date>2026-07-07</c:Date><c:Time>14:50:00</c:Time><c:Patient><c:ID>HCM2607070269</c:ID></c:Patient></c:Common><r:Measure type="REF"><r:REF><r:R><r:Median><r:Sphere>+1.750</r:Sphere><r:Cylinder>-1.00</r:Cylinder><r:Axis>178.0</r:Axis></r:Median></r:R><r:L><r:Median><r:Sphere>0.75</r:Sphere><r:Cylinder>-0.25</r:Cylinder><r:Axis>35</r:Axis></r:Median></r:L></r:REF></r:Measure></Ophthalmology>"#;
        let parsed = parse_measurement(xml).expect("parse fixture");
        assert_eq!(parsed.patient_id, "HCM2607070269");
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
