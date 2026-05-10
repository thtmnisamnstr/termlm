use chrono::Utc;
use termlm_indexer::{Chunk, HybridRetriever, RetrievalQuery};

fn chunk(command: &str, section: &str, text: &str) -> Chunk {
    Chunk {
        command_name: command.to_string(),
        path: format!("/usr/bin/{command}"),
        extraction_method: "fixture".to_string(),
        section_name: section.to_string(),
        chunk_index: 0,
        total_chunks: 1,
        doc_hash: format!("{command}-{section}"),
        extracted_at: Utc::now(),
        text: text.to_string(),
    }
}

#[test]
fn recursively_search_query_surfaces_grep() {
    let chunks = vec![
        chunk(
            "find",
            "OPTIONS",
            "find recursively traverses paths and can filter by name",
        ),
        chunk(
            "grep",
            "OPTIONS",
            "grep -r, --recursive reads all files under each directory and searches for patterns such as TODO",
        ),
        chunk("awk", "DESCRIPTION", "awk processes records and fields"),
        chunk("sed", "DESCRIPTION", "sed is a stream editor"),
        chunk(
            "git",
            "REBASE",
            "git rebase rewrites commits onto another base",
        ),
    ];

    let retriever = HybridRetriever::new(chunks);
    let result = retriever.search(&RetrievalQuery::new(
        "search recursively for files containing TODO",
        3,
        -100.0,
    ));

    assert!(
        result
            .iter()
            .any(|r| r.chunk.command_name.eq_ignore_ascii_case("grep")),
        "expected grep docs in top-3"
    );
}

#[test]
fn rebase_query_surfaces_git() {
    let chunks = vec![
        chunk(
            "grep",
            "OPTIONS",
            "grep searches text with regular expressions",
        ),
        chunk("find", "DESCRIPTION", "find traverses directory trees"),
        chunk(
            "git",
            "REBASE",
            "git rebase reapplies commits on top of another branch and can resolve conflicts",
        ),
        chunk("ssh", "DESCRIPTION", "ssh connects to remote hosts"),
    ];

    let retriever = HybridRetriever::new(chunks);
    let result = retriever.search(&RetrievalQuery::new("rebase my branch", 3, -100.0));

    assert!(
        result
            .iter()
            .any(|r| r.chunk.command_name.eq_ignore_ascii_case("git")),
        "expected git docs in top-3"
    );
}
