pub mod git_context;
pub mod list_workspace_files;
pub mod project_metadata;
pub mod read_file;
pub mod redaction;
pub mod search_files;
pub mod terminal_search;
pub mod text_detection;
pub mod workspace;

pub use git_context::{GitContextOptions, GitContextResult, git_context};
pub use list_workspace_files::{ListWorkspaceFilesResult, WorkspaceEntry, list_workspace_files};
pub use project_metadata::{ProjectMetadata, ProjectMetadataOptions, project_metadata};
pub use read_file::{ReadFileResult, read_file, read_file_with_detection};
pub use search_files::{FileMatch, SearchFilesOptions, SearchFilesResult, search_files};
pub use terminal_search::{ObservedTerminalEntry, TerminalSearchResult, search_terminal_context};
pub use text_detection::{
    TextDetection, TextDetectionOptions, detect_plaintext_like, detect_plaintext_like_with_options,
    is_plaintext_like,
};
pub use workspace::{
    WorkspaceResolution, resolve_workspace_root, resolve_workspace_root_with_markers,
    resolve_workspace_root_with_policy,
};
