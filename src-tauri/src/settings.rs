use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub api_base_url: String,
    pub username: String,
    pub xml_folder: Option<String>,
    pub facility_id: Option<i64>,
    pub auto_sync_enabled: bool,
}

pub fn load() -> AppSettings {
    AppSettings {
        api_base_url: "https://api-hisvn.vietngagroup.vn".to_string(),
        username: String::new(),
        xml_folder: None,
        facility_id: Some(4),
        auto_sync_enabled: false,
    }
}

pub fn save(settings: AppSettings) -> Result<AppSettings, String> {
    Ok(settings)
}
