use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub scanned_files: usize,
    pub sent_results: usize,
    pub skipped_files: usize,
    pub failed_files: usize,
}

pub fn run_once() -> Result<SyncSummary, String> {
    Ok(SyncSummary {
        scanned_files: 0,
        sent_results: 0,
        skipped_files: 0,
        failed_files: 0,
    })
}
