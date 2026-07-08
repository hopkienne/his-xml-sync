use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct HisClientConfig {
    pub base_url: String,
    pub username: String,
    pub facility_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct HisPatientMatch {
    pub patient_code: String,
    pub treatment_id: i64,
    pub full_name: String,
}
