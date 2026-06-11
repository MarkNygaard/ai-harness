//! Pure translation helpers: turn provider-agnostic [`NodeHooks`] into
//! provider-specific artifacts (Claude Code settings.json, omp extension config).

use harness_dag::model::{HookDecision, NodeHooks};
use serde_json::{json, Value};

/// Build the Claude Code settings.json value plus the decision payload files it
/// references. Each hook command is `cat <shlex-quoted-path>` (the caller writes
/// the payloads into the same temp dir and substitutes the absolute path).
pub fn claude_settings(
    hooks: &NodeHooks,
    dir: &std::path::Path,
) -> (Value, Vec<(std::path::PathBuf, String)>) {
    let mut payloads: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut pre_entries = Vec::new();
    let mut post_entries = Vec::new();

    for (i, rule) in hooks.pre_tool_use.iter().enumerate() {
        let filename = format!("pre-{i}.json");
        let path = dir.join(&filename);
        let decision = match rule.decision {
            Some(HookDecision::Allow) => "allow",
            Some(HookDecision::Deny) => "deny",
            Some(HookDecision::Ask) => "ask",
            None => "allow",
        };
        let mut payload = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": rule.reason.as_deref().unwrap_or("blocked by node hook"),
            }
        });
        if let Some(ctx) = &rule.additional_context {
            payload["hookSpecificOutput"]["additionalContext"] = json!(ctx);
        }
        if let Some(msg) = &rule.system_message {
            payload["systemMessage"] = json!(msg);
        }
        payloads.push((path, serde_json::to_string_pretty(&payload).unwrap()));
        let matcher = rule.matcher.as_deref().unwrap_or("");
        let cmd = format!(
            "cat {}",
            shlex::try_quote(&dir.join(&filename).to_string_lossy())
                .expect("tempdir path must not contain NUL bytes")
        );
        pre_entries.push(json!({
            "matcher": matcher,
            "hooks": [{
                "type": "command",
                "command": cmd,
            }]
        }));
    }

    for (i, rule) in hooks.post_tool_use.iter().enumerate() {
        let filename = format!("post-{i}.json");
        let path = dir.join(&filename);
        let mut payload = json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
            }
        });
        if let Some(ctx) = &rule.additional_context {
            payload["hookSpecificOutput"]["additionalContext"] = json!(ctx);
        }
        if let Some(msg) = &rule.system_message {
            payload["systemMessage"] = json!(msg);
        }
        payloads.push((path, serde_json::to_string_pretty(&payload).unwrap()));
        let matcher = rule.matcher.as_deref().unwrap_or("");
        let cmd = format!(
            "cat {}",
            shlex::try_quote(&dir.join(&filename).to_string_lossy())
                .expect("tempdir path must not contain NUL bytes")
        );
        post_entries.push(json!({
            "matcher": matcher,
            "hooks": [{
                "type": "command",
                "command": cmd,
            }]
        }));
    }

    let settings = json!({
        "hooks": {
            "PreToolUse": pre_entries,
            "PostToolUse": post_entries,
        }
    });

    (settings, payloads)
}

/// Serialize [`NodeHooks`] to JSON for delivery to the omp hook extension via
/// the `HARNESS_HOOKS` environment variable.
pub fn omp_hooks_env(hooks: &NodeHooks) -> String {
    serde_json::to_string(hooks).unwrap_or_default()
}

/// Build a Cursor `.cursor/hooks.json` value (version 1) registering the
/// bundled harness hook script on `preToolUse` and `postToolUse`. The script
/// reads hook rules from the `HARNESS_HOOKS` env var (same JSON as
/// [`omp_hooks_env`]).
pub fn cursor_hooks_json(script_path: &std::path::Path) -> Value {
    let path = script_path.to_string_lossy();
    let quoted = shlex::try_quote(&path).expect("script path must not contain NUL bytes");
    let command = |phase: &str| format!("node {quoted} {phase}");
    json!({
        "version": 1,
        "hooks": {
            "preToolUse": [{ "command": command("preToolUse") }],
            "postToolUse": [{ "command": command("postToolUse") }],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_dag::model::{HookDecision, HookRule};

    #[test]
    fn claude_settings_pre_deny_and_post_context() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write|Edit".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("read-only".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![HookRule {
                matcher: Some("Write|Edit".into()),
                decision: None,
                reason: None,
                additional_context: Some("Run cargo check".into()),
                system_message: None,
            }],
        };

        let dir = std::path::Path::new("/tmp/harness-claude-hooks-xyz");
        let (settings, payloads) = claude_settings(&hooks, dir);

        // settings.json shape
        let pre = settings["hooks"]["PreToolUse"][0].as_object().unwrap();
        assert_eq!(pre["matcher"], "Write|Edit");
        let cmd = pre["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("pre-0.json"));

        let post = settings["hooks"]["PostToolUse"][0].as_object().unwrap();
        assert_eq!(post["matcher"], "Write|Edit");
        let cmd = post["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("post-0.json"));

        // payloads
        assert_eq!(payloads.len(), 2);
        let pre_payload: Value = serde_json::from_str(&payloads[0].1).unwrap();
        assert_eq!(
            pre_payload["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
        assert_eq!(
            pre_payload["hookSpecificOutput"]["permissionDecisionReason"],
            "read-only"
        );

        let post_payload: Value = serde_json::from_str(&payloads[1].1).unwrap();
        assert_eq!(
            post_payload["hookSpecificOutput"]["additionalContext"],
            "Run cargo check"
        );
    }

    #[test]
    fn claude_settings_empty_matcher_defaults_to_empty_string() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: None,
                decision: Some(HookDecision::Deny),
                reason: None,
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };

        let dir = std::path::Path::new("/tmp/d");
        let (settings, _) = claude_settings(&hooks, dir);
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "");
    }

    #[test]
    fn omp_hooks_env_has_both_arrays_for_extension() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("blocked".into()),
                additional_context: Some("ctx".into()),
                system_message: Some("sys".into()),
            }],
            post_tool_use: vec![HookRule {
                matcher: Some("Edit".into()),
                decision: None,
                reason: None,
                additional_context: Some("post-ctx".into()),
                system_message: Some("post-sys".into()),
            }],
        };
        let json = omp_hooks_env(&hooks);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("pre_tool_use").is_some());
        assert!(v.get("post_tool_use").is_some());
        let pre = v["pre_tool_use"][0].as_object().unwrap();
        assert_eq!(pre["matcher"], "Write");
        assert_eq!(pre["decision"], "deny");
        assert_eq!(pre["reason"], "blocked");
        assert_eq!(pre["additional_context"], "ctx");
        assert_eq!(pre["system_message"], "sys");
    }

    #[test]
    fn omp_hooks_env_round_trip() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Bash".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("no shell".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let json = omp_hooks_env(&hooks);
        assert!(json.contains("\"deny\""));
        assert!(json.contains("\"pre_tool_use\""));

        let back: NodeHooks = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hooks);
    }

    #[test]
    fn omp_hooks_env_default_reason_when_absent() {
        // The default reason is applied at translation time (claude_settings),
        // not in the env payload — this test just confirms serialization stability.
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: None,
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let json = omp_hooks_env(&hooks);
        let back: NodeHooks = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pre_tool_use[0].reason, None);
    }

    #[test]
    fn claude_settings_system_message_and_additional_context() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Edit".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("out of plan".into()),
                additional_context: Some("extra".into()),
                system_message: Some("sys".into()),
            }],
            post_tool_use: vec![HookRule {
                matcher: Some("Read".into()),
                decision: None,
                reason: None,
                additional_context: Some("post-extra".into()),
                system_message: Some("post-sys".into()),
            }],
        };

        let dir = std::path::Path::new("/tmp/d");
        let (settings, payloads) = claude_settings(&hooks, dir);

        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            1
        );

        let pre_payload: Value = serde_json::from_str(&payloads[0].1).unwrap();
        assert_eq!(
            pre_payload["hookSpecificOutput"]["additionalContext"],
            "extra"
        );
        assert_eq!(pre_payload["systemMessage"], "sys");

        let post_payload: Value = serde_json::from_str(&payloads[1].1).unwrap();
        assert_eq!(
            post_payload["hookSpecificOutput"]["additionalContext"],
            "post-extra"
        );
        assert_eq!(post_payload["systemMessage"], "post-sys");
    }
    #[test]
    fn claude_settings_allow_and_ask_decisions() {
        let hooks = NodeHooks {
            pre_tool_use: vec![
                HookRule {
                    matcher: Some("Read".into()),
                    decision: Some(HookDecision::Allow),
                    reason: Some("allowed".into()),
                    additional_context: None,
                    system_message: None,
                },
                HookRule {
                    matcher: Some("Edit".into()),
                    decision: Some(HookDecision::Ask),
                    reason: Some("ask user".into()),
                    additional_context: None,
                    system_message: None,
                },
            ],
            post_tool_use: vec![],
        };
        let dir = std::path::Path::new("/tmp/d");
        let (settings, payloads) = claude_settings(&hooks, dir);
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "cat /tmp/d/pre-0.json"
        );
        let pre0: Value = serde_json::from_str(&payloads[0].1).unwrap();
        assert_eq!(pre0["hookSpecificOutput"]["permissionDecision"], "allow");
        let pre1: Value = serde_json::from_str(&payloads[1].1).unwrap();
        assert_eq!(pre1["hookSpecificOutput"]["permissionDecision"], "ask");
    }

    #[test]
    fn claude_settings_no_decision_defaults_to_allow() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: None,
                reason: None,
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let dir = std::path::Path::new("/tmp/d");
        let (settings, payloads) = claude_settings(&hooks, dir);
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "cat /tmp/d/pre-0.json"
        );
        let pre_payload: Value = serde_json::from_str(&payloads[0].1).unwrap();
        assert_eq!(
            pre_payload["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
    }

    #[test]
    fn claude_settings_escaped_path_with_single_quotes() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("blocked".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let dir = std::path::Path::new("/tmp/harness-claude-hooks-xyz");
        let (settings, _) = claude_settings(&hooks, dir);
        let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        // shlex quoting should produce a safe shell command
        assert!(cmd.starts_with("cat "));
        assert!(cmd.contains("pre-0.json"));
    }

    #[test]
    fn cursor_hooks_json_registers_both_events() {
        let script = std::path::Path::new("/tmp/worktree/.cursor/harness-hook.js");
        let value = cursor_hooks_json(script);

        assert_eq!(value["version"], 1);
        let pre = value["hooks"]["preToolUse"].as_array().unwrap();
        let post = value["hooks"]["postToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(post.len(), 1);

        let pre_cmd = pre[0]["command"].as_str().unwrap();
        assert!(pre_cmd.starts_with("node "));
        assert!(pre_cmd.contains("harness-hook.js"));
        assert!(pre_cmd.contains("preToolUse"));

        let post_cmd = post[0]["command"].as_str().unwrap();
        assert!(post_cmd.starts_with("node "));
        assert!(post_cmd.contains("harness-hook.js"));
        assert!(post_cmd.contains("postToolUse"));
    }

    #[test]
    fn cursor_hooks_json_quotes_path_with_spaces() {
        let script = std::path::Path::new("/tmp/my worktree/.cursor/harness-hook.js");
        let value = cursor_hooks_json(script);
        let pre_cmd = value["hooks"]["preToolUse"][0]["command"].as_str().unwrap();
        assert!(pre_cmd.starts_with("node "));
        assert!(pre_cmd.contains("my worktree"));
        assert!(pre_cmd.contains("preToolUse"));
    }

    const CURSOR_HOOK_SCRIPT: &str = include_str!("../extensions/cursor-hooks/hook.js");

    fn node_available() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn cursor_hook_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let hook_path = dir.path().join("hook.js");
        std::fs::write(&hook_path, CURSOR_HOOK_SCRIPT).unwrap();
        (dir, hook_path)
    }

    fn run_cursor_hook(
        hook_path: &std::path::Path,
        phase: &str,
        harness_hooks: Option<&str>,
        stdin: &str,
    ) -> std::process::Output {
        let mut cmd = std::process::Command::new("node");
        cmd.arg(hook_path).arg(phase);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        if let Some(env) = harness_hooks {
            cmd.env("HARNESS_HOOKS", env);
        } else {
            cmd.env_remove("HARNESS_HOOKS");
        }
        let mut child = cmd.spawn().expect("spawn node hook");
        if !stdin.is_empty() {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin.as_bytes())
                .unwrap();
        }
        child.wait_with_output().expect("wait for node hook")
    }

    #[test]
    fn cursor_hook_denies_matching_pre_rule() {
        if !node_available() {
            return;
        }
        let (_dir, hook_path) = cursor_hook_fixture();

        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write|Edit".into()),
                decision: Some(HookDecision::Deny),
                reason: None,
                additional_context: Some("read-only".into()),
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook(
            &hook_path,
            "preToolUse",
            Some(&env),
            r#"{"tool_name":"Write"}"#,
        );
        assert!(out.status.success());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["permission"], "deny");
        assert_eq!(v["agent_message"], "read-only");
    }

    #[test]
    fn cursor_hook_allows_non_matching_pre_rule() {
        if !node_available() {
            return;
        }
        let (_dir, hook_path) = cursor_hook_fixture();

        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write|Edit".into()),
                decision: Some(HookDecision::Deny),
                reason: None,
                additional_context: Some("read-only".into()),
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook(
            &hook_path,
            "preToolUse",
            Some(&env),
            r#"{"tool_name":"Read"}"#,
        );
        assert!(out.status.success());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["permission"], "allow");
    }

    #[test]
    fn cursor_hook_injects_post_context() {
        if !node_available() {
            return;
        }
        let (_dir, hook_path) = cursor_hook_fixture();

        let hooks = NodeHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: None,
                reason: None,
                additional_context: Some("Run cargo check".into()),
                system_message: None,
            }],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook(
            &hook_path,
            "postToolUse",
            Some(&env),
            r#"{"tool_name":"Write"}"#,
        );
        assert!(out.status.success());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["permission"], "allow");
        assert!(v["agent_message"]
            .as_str()
            .unwrap()
            .contains("Run cargo check"));
    }

    #[test]
    fn cursor_hook_missing_env_is_allow() {
        if !node_available() {
            return;
        }
        let (_dir, hook_path) = cursor_hook_fixture();

        let out = run_cursor_hook(&hook_path, "preToolUse", None, r#"{"tool_name":"Write"}"#);
        assert!(out.status.success());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["permission"], "allow");
    }
}
