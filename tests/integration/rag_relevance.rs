use chrono::Utc;
use termlm_indexer::{Chunk, Chunker, HybridRetriever, RetrievalQuery};

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

fn assert_command_in_top3(chunks: Vec<Chunk>, query: &str, expected_command: &str) {
    let retriever = HybridRetriever::new(chunks);
    let result = retriever.search(&RetrievalQuery::new(query, 3, -100.0));

    assert!(
        result
            .iter()
            .any(|r| r.chunk.command_name.eq_ignore_ascii_case(expected_command)),
        "expected {expected_command} docs in top-3 for query {query:?}; got {:?}",
        result
            .iter()
            .map(|r| r.chunk.command_name.as_str())
            .collect::<Vec<_>>()
    );
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

#[test]
fn synthesized_option_intents_help_files_only_query() {
    let chunker = Chunker::new(4096);
    let mut chunks = chunker.chunk_document(
        "find",
        "/usr/bin/find",
        "man",
        "NAME\nfind - walk a file hierarchy\n\nOPTIONS\n  -type t        File is of type t. Type f is a regular file and d is a directory.\n  -name pattern  Base of file name matches shell pattern.\n",
    );
    chunks.push(chunk(
        "ls",
        "DESCRIPTION",
        "ls lists directory contents and can show entries in columns",
    ));
    chunks.push(chunk(
        "grep",
        "DESCRIPTION",
        "grep searches file contents for text patterns",
    ));

    assert_command_in_top3(chunks, "list files only but not directories", "find");
}

#[test]
fn synthesized_task_phrases_cover_common_prompt_language() {
    let chunker = Chunker::new(4096);
    let mut chunks = Vec::new();
    chunks.extend(chunker.chunk_document(
        "find",
        "/usr/bin/find",
        "man",
        "NAME\nfind - walk a file hierarchy\n\nOPTIONS\n  -type t        File is of type t. Type f is a regular file and d is a directory.\n  -name pattern  Base of file name matches shell pattern.\n  -size n        True if the file uses n units of space.\n",
    ));
    chunks.extend(chunker.chunk_document(
        "grep",
        "/usr/bin/grep",
        "man",
        "NAME\ngrep - file pattern searcher\n\nOPTIONS\n  -i, --ignore-case        Ignore case distinctions in patterns and input data.\n  -l, --files-with-matches Only the names of files containing selected lines are written.\n",
    ));
    chunks.extend(chunker.chunk_document(
        "head",
        "/usr/bin/head",
        "man",
        "NAME\nhead - display first lines of a file\n\nOPTIONS\n  -n number        The first number lines of each input file are displayed.\n",
    ));
    chunks.extend(chunker.chunk_document(
        "tail",
        "/usr/bin/tail",
        "man",
        "NAME\ntail - display the last part of a file\n\nOPTIONS\n  -n number        The last number lines of each input file are displayed.\n",
    ));
    chunks.extend(chunker.chunk_document(
        "wc",
        "/usr/bin/wc",
        "man",
        "NAME\nwc - word, line, character, and byte count\n\nOPTIONS\n  -l        The number of lines in each input file is written to standard output.\n",
    ));
    chunks.extend(chunker.chunk_document(
        "sort",
        "/usr/bin/sort",
        "man",
        "NAME\nsort - sort or merge records\n\nOPTIONS\n  -n        Compare according to numerical string value.\n  -r        Reverse the result of comparisons.\n",
    ));
    chunks.push(chunk(
        "ls",
        "DESCRIPTION",
        "ls lists directory entries and can show long details",
    ));

    assert_command_in_top3(
        chunks.clone(),
        "show files larger than one megabyte under here",
        "find",
    );
    assert_command_in_top3(chunks.clone(), "search for todo ignoring uppercase", "grep");
    assert_command_in_top3(
        chunks.clone(),
        "show only the names of files whose contents contain TODO",
        "grep",
    );
    assert_command_in_top3(
        chunks.clone(),
        "show the first 3 lines of notes.txt",
        "head",
    );
    assert_command_in_top3(chunks.clone(), "count the lines in notes.txt", "wc");
    assert_command_in_top3(chunks, "sort names alphabetically", "sort");
}

#[test]
fn task_phrases_exist_even_when_option_docs_are_sparse() {
    let chunker = Chunker::new(4096);
    let chunks = chunker.chunk_document(
        "pwd",
        "/bin/pwd",
        "help",
        "NAME\npwd - print working directory\n",
    );
    let usage = chunks
        .iter()
        .find(|chunk| chunk.section_name == "USAGE INTENTS")
        .expect("usage intents chunk for sparse pwd docs");
    assert!(usage.text.contains("what directory am I in"));
}
