use anyhow::Result;
use termlm_indexer::extract::extract_docs_with_method;
use termlm_indexer::{Chunker, discover_binaries};

fn mk_temp_dir(prefix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_executable(path: &std::path::Path, help_text: &str) {
    let script = format!(
        "#!/usr/bin/env bash\nif [[ \"${{1:-}}\" == \"--help\" ]] || [[ \"${{1:-}}\" == \"-h\" ]]; then\n  echo '{help_text}'\n  exit 0\nfi\necho run\n"
    );
    std::fs::write(path, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod");
    }
}

#[test]
fn discovers_adds_removes_and_reextracts() -> Result<()> {
    let bin_dir = mk_temp_dir("termlm-indexer-e2e-bin");

    for i in 0..5 {
        let p = bin_dir.join(format!("fake{i}"));
        write_executable(&p, &format!("fake{i} usage"));
    }

    let mut found = discover_binaries(bin_dir.to_string_lossy().as_ref(), 100);
    found.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(found.len(), 5);

    let added = bin_dir.join("fake_added");
    write_executable(&added, "fake_added usage");
    let found_after_add = discover_binaries(bin_dir.to_string_lossy().as_ref(), 100);
    assert_eq!(found_after_add.len(), 6);

    std::fs::remove_file(bin_dir.join("fake3"))?;
    let found_after_remove = discover_binaries(bin_dir.to_string_lossy().as_ref(), 100);
    assert_eq!(found_after_remove.len(), 5);

    write_executable(&bin_dir.join("fake2"), "fake2 updated help");
    let extracted = extract_docs_with_method(
        "fake2",
        bin_dir.join("fake2").to_string_lossy().as_ref(),
        16 * 1024,
    )?;
    if extracted.method != "man" {
        assert!(extracted.text.contains("fake2 updated help"));
    } else {
        assert!(!extracted.text.trim().is_empty());
    }

    let chunker = Chunker::new(256);
    let chunks = chunker.chunk_document(
        "fake2",
        bin_dir.join("fake2").to_string_lossy().as_ref(),
        &extracted.method,
        &format!("NAME\nfake2\n\nOPTIONS\n{}\n", extracted.text),
    );
    assert!(!chunks.is_empty());
    assert!(
        chunks
            .iter()
            .any(|c| c.section_name.eq_ignore_ascii_case("NAME"))
    );

    let _ = std::fs::remove_dir_all(&bin_dir);
    Ok(())
}
