//! Curated IE / vendor signatures (MIT-licensed data in-repo). See `docs/FINGERPRINTING.md`.

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct CuratedRule {
    pub ie_sig: String,
    pub hint: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    0.5
}

static RULES: OnceLock<Vec<CuratedRule>> = OnceLock::new();

fn rules_slice() -> &'static [CuratedRule] {
    RULES
        .get_or_init(|| {
            const JSON: &str = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/fingerprints/default.json"
            ));
            serde_json::from_str(JSON).unwrap_or_default()
        })
        .as_slice()
}

#[must_use]
pub fn match_probe_ie_sig(probe_ie_sig: &str) -> Option<&'static CuratedRule> {
    if probe_ie_sig.is_empty() {
        return None;
    }
    rules_slice().iter().find(|r| r.ie_sig == probe_ie_sig)
}

#[must_use]
pub fn match_vendor_ie_sig(vendor_sigs: &[String]) -> Option<&'static CuratedRule> {
    let joined = vendor_sigs.join("|");
    rules_slice()
        .iter()
        .find(|r| !r.ie_sig.is_empty() && joined.contains(&r.ie_sig))
}
