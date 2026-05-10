use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub source_type: String,
    pub source_id: String,
    pub hash: String,
    pub redacted: bool,
    pub truncated: bool,
    pub observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_end: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_version: Option<u32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SourceLedger {
    pub refs: Vec<SourceRef>,
}

impl SourceLedger {
    #[cfg(test)]
    pub fn push(&mut self, r: SourceRef) {
        self.refs.push(r);
    }

    pub fn extend<I>(&mut self, refs: I)
    where
        I: IntoIterator<Item = SourceRef>,
    {
        self.refs.extend(refs);
    }
}
