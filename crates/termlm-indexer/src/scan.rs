use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryEntry {
    pub name: String,
    pub path: PathBuf,
    pub mtime_secs: i64,
    pub size: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverBinariesResult {
    pub entries: Vec<BinaryEntry>,
    pub capped: bool,
}

pub fn discover_binaries(path_var: &str, max_binaries: usize) -> Vec<BinaryEntry> {
    discover_binaries_with_stats(path_var, max_binaries).entries
}

pub fn discover_binaries_with_stats(path_var: &str, max_binaries: usize) -> DiscoverBinariesResult {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for dir in path_var.split(':') {
        let p = PathBuf::from(dir);
        let Ok(rd) = fs::read_dir(&p) else {
            continue;
        };

        for entry in rd.flatten() {
            let path = entry.path();
            if seen.contains(&path) {
                continue;
            }

            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }

            let mode = meta.permissions().mode();
            if mode & 0o111 == 0 {
                continue;
            }

            let inode = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    meta.ino()
                }
                #[cfg(not(unix))]
                {
                    0
                }
            };

            let mtime_secs = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    meta.mtime()
                }
                #[cfg(not(unix))]
                {
                    0
                }
            };

            out.push(BinaryEntry {
                name: path
                    .file_name()
                    .map(|x| x.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: path.clone(),
                mtime_secs,
                size: meta.len(),
                inode,
            });
            seen.insert(path);

            if out.len() >= max_binaries {
                return DiscoverBinariesResult {
                    entries: out,
                    capped: true,
                };
            }
        }
    }

    DiscoverBinariesResult {
        entries: out,
        capped: false,
    }
}
