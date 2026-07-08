use serde::{Deserialize, Serialize};
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

pub fn preview_file(path: &str) -> Result<XmlPreview, String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string();

    Ok(XmlPreview {
        file_name,
        patient_id: None,
        measured_at: None,
        right: EyeRefraction {
            sphere: None,
            cylinder: None,
            axis: None,
        },
        left: EyeRefraction {
            sphere: None,
            cylinder: None,
            axis: None,
        },
    })
}
