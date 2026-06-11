#!/usr/bin/env node
/**
 * harness-hook — Cursor project hook script for per-node tool hooks.
 *
 * Reads HARNESS_HOOKS (serialized NodeHooks JSON, snake_case keys) and applies
 * pre/post rules to the stdin event payload. Emits Cursor decision JSON on stdout.
 */

const fs = require("fs");

function testMatcher(matcher, toolName) {
  if (!matcher) return true;
  try {
    return new RegExp(matcher).test(toolName);
  } catch {
    return false;
  }
}

function respond(decision) {
  process.stdout.write(JSON.stringify(decision));
  process.exit(0);
}

function allow() {
  respond({ permission: "allow" });
}

function parseHarnessHooks() {
  const raw = process.env.HARNESS_HOOKS;
  if (!raw) return null;
  try {
    const hooks = JSON.parse(raw);
    if (!hooks || typeof hooks !== "object") return null;
    return hooks;
  } catch {
    return null;
  }
}

function readStdinPayload() {
  try {
    const text = fs.readFileSync(0, "utf8").trim();
    if (!text) return {};
    return JSON.parse(text);
  } catch {
    return {};
  }
}

function extractToolName(payload) {
  if (!payload || typeof payload !== "object") return "";
  const tool = payload.tool;
  if (tool && typeof tool === "object" && typeof tool.name === "string") {
    return tool.name;
  }
  for (const key of ["tool_name", "toolName", "name"]) {
    if (typeof payload[key] === "string") return payload[key];
  }
  return "";
}

function detectPhase(argvPhase, payload) {
  if (argvPhase === "preToolUse") {
    return "preToolUse";
  }
  if (argvPhase === "postToolUse" || argvPhase === "afterFileEdit") {
    return "postToolUse";
  }
  const event =
    payload.hook_event_name ||
    payload.hookEventName ||
    payload.event ||
    "";
  const lower = String(event).toLowerCase();
  if (lower.includes("pre")) return "preToolUse";
  return "postToolUse";
}

function preDecisionMessage(rule) {
  return rule.additional_context || rule.reason || "blocked by node hook";
}

function joinMessage(rule) {
  const parts = [];
  if (rule.additional_context) parts.push(rule.additional_context);
  if (rule.system_message) parts.push(rule.system_message);
  return parts.join("\n");
}

function handlePre(hooks, toolName) {
  const rules = hooks.pre_tool_use || [];
  for (const rule of rules) {
    if (!testMatcher(rule.matcher, toolName)) continue;
    if (rule.decision === "deny" || rule.decision === "ask") {
      respond({
        permission: rule.decision,
        agent_message: preDecisionMessage(rule),
      });
    }
  }
  allow();
}

function handlePost(hooks, toolName) {
  const rules = hooks.post_tool_use || [];
  const messages = [];
  for (const rule of rules) {
    if (!testMatcher(rule.matcher, toolName)) continue;
    const message = joinMessage(rule);
    if (message) messages.push(message);
  }
  if (messages.length > 0) {
    respond({ permission: "allow", agent_message: messages.join("\n") });
  }
  allow();
}

try {
  const payload = readStdinPayload();
  const phase = detectPhase(process.argv[2], payload);
  const hooks = parseHarnessHooks();
  if (!hooks) {
    allow();
  }
  const toolName = extractToolName(payload);
  if (phase === "preToolUse") {
    handlePre(hooks, toolName);
  } else {
    handlePost(hooks, toolName);
  }
} catch {
  allow();
}
