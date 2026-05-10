use crate::redaction::redact_secrets;
use crate::text_detection::{TextDetectionOptions, detect_plaintext_like_with_options};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileResult {
    pub path: String,
    pub cwd: String,
    pub workspace_root: String,
    pub content: String,
    pub redacted: bool,
    pub truncated: bool,
    pub bytes_read: usize,
    pub detector: String,
    pub encoding: String,
    pub detection_reason: String,
}

pub fn read_file(path: &Path, max_bytes: usize) -> Result<ReadFileResult> {
    read_file_with_detection(path, max_bytes, &TextDetectionOptions::default())
}

pub fn read_file_with_detection(
    path: &Path,
    max_bytes: usize,
    detection_options: &TextDetectionOptions,
) -> Result<ReadFileResult> {
    let max_bytes = max_bytes.max(1);
    let mut file = File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut data = Vec::with_capacity(max_bytes.min(8192).saturating_add(1));
    file.by_ref()
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut data)
        .with_context(|| format!("read {}", path.display()))?;
    let detection = detect_plaintext_like_with_options(&data, detection_options);
    if !detection.plaintext_like {
        return Err(anyhow!("binary_or_unsupported_file"));
    }

    let truncated = data.len() > max_bytes;
    if truncated {
        data.truncate(max_bytes);
    }
    let text = String::from_utf8_lossy(&data).to_string();
    let redacted = redact_secrets(&text);
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| String::new());
    let workspace_root = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());

    Ok(ReadFileResult {
        path: path.display().to_string(),
        cwd,
        workspace_root,
        content: redacted,
        redacted: true,
        truncated,
        bytes_read: data.len(),
        detector: detection.detector,
        encoding: detection.encoding,
        detection_reason: detection.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_byte_budget() {
        let root = std::env::temp_dir().join(format!(
            "termlm-read-file-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let file = root.join("large.txt");
        std::fs::write(&file, "a".repeat(5000)).expect("write");

        let out = read_file(&file, 128).expect("read");
        assert!(out.truncated);
        assert_eq!(out.bytes_read, 128);
        assert_eq!(out.content.len(), 128);

        let _ = std::fs::remove_dir_all(&root);
    }
}
