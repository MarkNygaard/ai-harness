//! Pure translation helpers: turn provider-agnostic [`NodeHooks`] into
//! provider-specific artifacts (Claude Code `settings.json`, omp extension
//! config, Cursor `hooks.json`).
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

/// Build the project-level Cursor `.cursor/hooks.json` value that registers the
/// bundled harness hook script on `preToolUse` and `postToolUse`. The script is
/// invoked as `node <script> <event>`; it reads the rules from `HARNESS_HOOKS`
/// (see [`omp_hooks_env`]) and the event payload from stdin. Cursor reads this
/// file relative to the workspace root, so the caller writes it into the run's
/// worktree (`req.cwd/.cursor/hooks.json`).
pub fn cursor_hooks_json(_hooks: &NodeHooks, script_path: &std::path::Path) -> Value {
    let quoted = shlex::try_quote(&script_path.to_string_lossy())
        .expect("tempdir path must not contain NUL bytes")
        .into_owned();
    let cmd = |event: &str| json!({ "command": format!("node {quoted} {event}") });
    json!({
        "version": 1,
        "hooks": {
            "preToolUse": [cmd("preToolUse")],
            "postToolUse": [cmd("postToolUse")],
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
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("read-only".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![HookRule {
                matcher: Some("Edit".into()),
                decision: None,
                reason: None,
                additional_context: Some("Run cargo check".into()),
                system_message: None,
            }],
        };
        let v = cursor_hooks_json(
            &hooks,
            std::path::Path::new("/tmp/harness-cursor-hooks-xyz/hook.js"),
        );
        assert_eq!(v["version"], 1);
        let pre = v["hooks"]["preToolUse"][0]["command"].as_str().unwrap();
        assert!(pre.starts_with("node "));
        assert!(pre.contains("hook.js"));
        assert!(pre.contains("preToolUse"));
        let post = v["hooks"]["postToolUse"][0]["command"].as_str().unwrap();
        assert!(post.starts_with("node "));
        assert!(post.contains("hook.js"));
        assert!(post.contains("postToolUse"));
    }

    #[test]
    fn cursor_hooks_json_quotes_script_path() {
        let hooks = NodeHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![],
        };
        let v = cursor_hooks_json(&hooks, std::path::Path::new("/tmp/my dir/hook.js"));
        let cmd = v["hooks"]["preToolUse"][0]["command"].as_str().unwrap();
        assert!(cmd.starts_with("node "));
        assert!(cmd.contains("hook.js"));
        assert!(cmd.contains("preToolUse"));
    }

    // -------------------------------------------------------------------------
    // Script-decision tests (gated on node availability)
    // -------------------------------------------------------------------------
    const CURSOR_HOOK_SCRIPT: &str = include_str!("../extensions/harness-cursor-hooks/hook.js");

    fn run_cursor_hook(event: &str, hooks_env: &str, stdin: &str) -> Option<Value> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let has_node = Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !has_node {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hook.js");
        std::fs::write(&script, CURSOR_HOOK_SCRIPT).unwrap();
        let mut child = Command::new("node")
            .arg(&script)
            .arg(event)
            .env("HARNESS_HOOKS", hooks_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        Some(serde_json::from_slice(&out.stdout).unwrap())
    }

    #[test]
    fn cursor_hook_pre_deny_match() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("read-only".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, r#"{"tool_name":"Write"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "deny");
        assert_eq!(out["agent_message"], "read-only");
    }

    #[test]
    fn cursor_hook_pre_deny_falls_back_to_additional_context() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: None,
                additional_context: Some("no writes".into()),
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, r#"{"tool_name":"Write"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "deny");
        assert_eq!(out["agent_message"], "no writes");
    }

    #[test]
    fn cursor_hook_pre_non_match_allows() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("read-only".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, r#"{"tool_name":"Read"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "allow");
        assert!(out.get("agent_message").is_none() || out["agent_message"].as_str() == Some(""));
    }

    #[test]
    fn cursor_hook_post_context() {
        let hooks = NodeHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![HookRule {
                matcher: Some("Edit".into()),
                decision: None,
                reason: None,
                additional_context: Some("Run cargo check".into()),
                system_message: None,
            }],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("postToolUse", &env, r#"{"tool_name":"Edit"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "allow");
        assert_eq!(out["agent_message"], "Run cargo check");
    }

    #[test]
    fn cursor_hook_empty_stdin_no_rules() {
        let hooks = NodeHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, "");
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "allow");
    }

    #[test]
    fn cursor_hook_invalid_regex_allows() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("[".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("broken".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, r#"{"tool_name":"Write"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "allow");
    }
    #[test]
    fn cursor_hook_post_system_message_only() {
        let hooks = NodeHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![HookRule {
                matcher: Some("Edit".into()),
                decision: None,
                reason: None,
                additional_context: None,
                system_message: Some("check formatting".into()),
            }],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("postToolUse", &env, r#"{"tool_name":"Edit"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "allow");
        assert_eq!(out["agent_message"], "check formatting");
    }
    #[test]
    fn cursor_hook_falls_back_to_camelcase_tool_name() {
        let hooks = NodeHooks {
            pre_tool_use: vec![HookRule {
                matcher: Some("Write".into()),
                decision: Some(HookDecision::Deny),
                reason: Some("read-only".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        };
        let env = omp_hooks_env(&hooks);
        let out = run_cursor_hook("preToolUse", &env, r#"{"toolName":"Write"}"#);
        let out = match out {
            Some(v) => v,
            None => return,
        };
        assert_eq!(out["permission"], "deny");
        assert_eq!(out["agent_message"], "read-only");
    }
}
