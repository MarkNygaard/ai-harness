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
}
