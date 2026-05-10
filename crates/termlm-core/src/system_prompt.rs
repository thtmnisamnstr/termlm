use crate::tasks::TaskClassification;

pub fn build_system_prompt(
    shell_kind: &str,
    provider: &str,
    cwd: &str,
    approval_mode: &str,
    capture_enabled: bool,
    cls: &TaskClassification,
    cls_confidence: f32,
) -> String {
    let task_class = match cls {
        TaskClassification::FreshCommandRequest => "fresh_command_request",
        TaskClassification::ReferentialFollowup => "referential_followup",
        TaskClassification::DiagnosticDebugging => "diagnostic_debugging",
        TaskClassification::DocumentationQuestion => "documentation_question",
        TaskClassification::WebCurrentInfoQuestion => "web_current_info_question",
        TaskClassification::ExploratoryShellQuestion => "exploratory_shell_question",
    };

    let capture_rule = if capture_enabled {
        "12. Command output capture is enabled, but outputs may be truncated."
    } else {
        "12. Command output capture is disabled; do not assume stdout/stderr will be available."
    };

    format!(
        "You are termlm, a terminal assistant running in the user's {shell_kind} session.\n\
Provider: {provider}\n\
Current working directory: {cwd}\n\
Approval mode: {approval_mode}\n\
Task class: {task_class} (confidence={cls_confidence:.2})\n\
\n\
Core rules:\n\
1. Prefer commands that are installed and documented in local context.\n\
2. Do not invent commands, flags, aliases, or functions.\n\
3. For referential/debugging tasks, use recent terminal context first.\n\
4. For fresh tasks, avoid terminal history unless the prompt references it.\n\
5. Use lookup_command_docs or relevant docs before uncommon flags.\n\
6. Prefer read-only local tools before executing shell commands when sufficient.\n\
7. Use web tools only for current/web information requests.\n\
8. Keep local trust order: question, recent terminal, local files/git/project/docs, then web.\n\
9. Use execute_shell_command only when the user wants execution.\n\
10. If clarification is required, ask one focused question ending with '?'.\n\
11. Keep responses concise and action-oriented.\n\
{capture_rule}\n"
    )
}
