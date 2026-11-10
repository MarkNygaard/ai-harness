use super::*;

#[test]
fn log_captured_stderr_does_not_panic_on_empty() {
    log_captured_stderr("", "agent");
}

#[test]
fn log_captured_stderr_truncates_long_lines() {
    let long_line = "x".repeat(2000);
    // Should not panic
    log_captured_stderr(&long_line, "agent");
}

#[test]
fn warning_classifier_preserves_real_warning_lines() {
    let disposition = stderr_line_disposition("warning: unused import");
    assert_eq!(disposition, Some(StderrLineDisposition::Warning));
}

#[test]
fn looks_like_code_content_matches_indented_rust_fragments() {
    assert!(looks_like_code_content(
        "                        tracing::warn!("
    ));
    assert!(looks_like_code_content(
        "                                error = %e,"
    ));
    assert!(looks_like_code_content(
            "                                \"scheduler: failed to load registry project config, skipping\""
        ));
}

#[test]
fn looks_like_code_content_matches_patch_lines() {
    assert!(looks_like_code_content(
        "+        // Incomplete %XX should be left as-is, not panic"
    ));
    assert!(looks_like_code_content(
        "+                Err(e) => tracing::warn!("
    ));
}

#[test]
fn looks_like_code_content_does_not_hide_real_warning_lines() {
    assert!(!looks_like_code_content("warning: unused import"));
    assert!(!looks_like_code_content(
        "agent execution failed: claude exited with status 1"
    ));
}

#[test]
fn looks_like_git_commit_line_matches_history_output() {
    assert!(looks_like_git_commit_line(
        "7f84b9a fix(lifecycle): add Item::Error for agent-not-found and stall paths"
    ));
    assert!(looks_like_git_commit_line(
            "fb15c77 Merge pull request #352 from majiayu000/feat/issue-88-warn-missing-preflight-skill"
        ));
}

#[test]
fn looks_like_search_result_matches_path_line_output() {
    assert!(looks_like_search_result(
            "./rules/go/quality.md:20:Return errors instead. `panic` only in `main()` for truly unrecoverable states."
        ));
    assert!(looks_like_search_result(
            "./crates/harness-server/src/http/background.rs:382:                            format!(\"startup recovery: failed to acquire concurrency permit: {e}\");"
        ));
    assert!(!looks_like_search_result(
        "thread 'main' panicked at src/main.rs:10:5:"
    ));
}

#[test]
fn looks_like_markdown_prompt_line_matches_headings_and_bullets() {
    assert!(looks_like_markdown_prompt_line("## Warnings"));
    assert!(looks_like_markdown_prompt_line(
        "- <action to resolve block/warn>"
    ));
    assert!(looks_like_markdown_prompt_line(
        "Pass: <N> | Warn: <N> | Block: <N>"
    ));
}
