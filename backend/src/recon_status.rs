//! Live recon job status for HTTP polling.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReconIfaceStatus {
    pub name: String,
    pub channel: Option<u8>,
    pub role: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReconStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub duration_secs: u64,
    pub elapsed_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub interfaces: Vec<ReconIfaceStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_sequence: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble_adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl ReconStatus {
    pub fn idle(last_output: Option<String>, last_error: Option<String>) -> Self {
        Self {
            running: false,
            last_output,
            last_error,
            ..Default::default()
        }
    }
}
