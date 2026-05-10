use serde::{Deserialize, Serialize};
use std::str;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextDetectionOptions {
    pub sample_bytes: usize,
    pub reject_nul_bytes: bool,
    pub accepted_encodings: Vec<String>,
    pub deny_binary_magic: bool,
}

impl Default for TextDetectionOptions {
    fn default() -> Self {
        Self {
            sample_bytes: 8_192,
            reject_nul_bytes: true,
            accepted_encodings: vec![
                "utf-8".to_string(),
                "utf-16le".to_string(),
                "utf-16be".to_string(),
            ],
            deny_binary_magic: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextDetection {
    pub plaintext_like: bool,
    pub detector: String,
    pub encoding: String,
    pub reason: String,
}

pub fn detect_plaintext_like(bytes: &[u8]) -> TextDetection {
    detect_plaintext_like_with_options(bytes, &TextDetectionOptions::default())
}

pub fn detect_plaintext_like_with_options(
    bytes: &[u8],
    opts: &TextDetectionOptions,
) -> TextDetection {
    let sample_len = bytes.len().min(opts.sample_bytes.max(1));
    let sample = &bytes[..sample_len];

    if opts.reject_nul_bytes && sample.contains(&0) {
        return TextDetection {
            plaintext_like: false,
            detector: "content_sniff_v1".to_string(),
            encoding: "binary".to_string(),
            reason: "nul_byte".to_string(),
        };
    }

    if opts.deny_binary_magic
        && (sample.starts_with(&[0x89, b'P', b'N', b'G'])
            || sample.starts_with(&[0xFF, 0xD8, 0xFF])
            || sample.starts_with(b"GIF89a")
            || sample.starts_with(b"GIF87a")
            || sample.starts_with(b"PK\x03\x04"))
    {
        return TextDetection {
            plaintext_like: false,
            detector: "content_sniff_v1".to_string(),
            encoding: "binary".to_string(),
            reason: "binary_magic".to_string(),
        };
    }

    if str::from_utf8(sample).is_ok() {
        if !encoding_allowed(opts, "utf8") {
            return TextDetection {
                plaintext_like: false,
                detector: "content_sniff_v1".to_string(),
                encoding: "utf8".to_string(),
                reason: "encoding_not_allowed".to_string(),
            };
        }
        return TextDetection {
            plaintext_like: true,
            detector: "content_sniff_v1".to_string(),
            encoding: "utf8".to_string(),
            reason: "utf8_valid".to_string(),
        };
    }

    if sample.len() >= 2 {
        // UTF-16 heuristic.
        let even_zeroes = sample.iter().step_by(2).filter(|b| **b == 0).count();
        let odd_zeroes = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|b| **b == 0)
            .count();
        let half = sample.len() / 2;
        if even_zeroes > half / 3 || odd_zeroes > half / 3 {
            if !encoding_allowed(opts, "utf16") {
                return TextDetection {
                    plaintext_like: false,
                    detector: "content_sniff_v1".to_string(),
                    encoding: "utf16".to_string(),
                    reason: "encoding_not_allowed".to_string(),
                };
            }
            return TextDetection {
                plaintext_like: true,
                detector: "content_sniff_v1".to_string(),
                encoding: "utf16".to_string(),
                reason: "utf16_heuristic".to_string(),
            };
        }
    }

    TextDetection {
        plaintext_like: false,
        detector: "content_sniff_v1".to_string(),
        encoding: "unknown".to_string(),
        reason: "decode_failed".to_string(),
    }
}

pub fn is_plaintext_like(bytes: &[u8]) -> bool {
    detect_plaintext_like(bytes).plaintext_like
}

fn encoding_allowed(opts: &TextDetectionOptions, encoding: &str) -> bool {
    if opts.accepted_encodings.is_empty() {
        return true;
    }
    let wanted = encoding.to_ascii_lowercase();
    opts.accepted_encodings
        .iter()
        .map(|v| v.trim().to_ascii_lowercase())
        .any(|candidate| match wanted.as_str() {
            "utf8" => candidate == "utf-8" || candidate == "utf8",
            "utf16" => {
                candidate == "utf-16"
                    || candidate == "utf16"
                    || candidate == "utf-16le"
                    || candidate == "utf16le"
                    || candidate == "utf-16be"
                    || candidate == "utf16be"
            }
            _ => candidate == wanted,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_utf8_text() {
        assert!(is_plaintext_like(b"hello world\n"));
        let d = detect_plaintext_like(b"hello world\n");
        assert_eq!(d.encoding, "utf8");
    }

    #[test]
    fn rejects_binary_magic() {
        assert!(!is_plaintext_like(b"\x89PNG\r\n\x1a\n...."));
        assert!(!is_plaintext_like(&[0, 159, 146, 150]));
        let d = detect_plaintext_like(b"\x89PNG\r\n\x1a\n....");
        assert_eq!(d.reason, "binary_magic");
    }

    #[test]
    fn honors_encoding_allowlist() {
        let opts = TextDetectionOptions {
            accepted_encodings: vec!["utf-16le".to_string()],
            ..TextDetectionOptions::default()
        };
        let d = detect_plaintext_like_with_options(b"hello", &opts);
        assert!(!d.plaintext_like);
        assert_eq!(d.reason, "encoding_not_allowed");
    }

    #[test]
    fn can_disable_binary_magic_rejection() {
        let opts = TextDetectionOptions {
            deny_binary_magic: false,
            ..TextDetectionOptions::default()
        };
        let d = detect_plaintext_like_with_options(b"GIF89a<html>", &opts);
        assert!(d.plaintext_like);
    }
}
