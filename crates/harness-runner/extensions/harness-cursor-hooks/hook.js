/**
 * Cursor hook script — reads HARNESS_HOOKS + stdin payload and emits a
 * Cursor decision JSON object.  Fail-open: any error yields {"permission":"allow"}.
 *
 * Invoked as: node <this> <event>   (event = "preToolUse" | "postToolUse")
 */
"use strict";

function failOpen(event) {
  console.log(JSON.stringify(event === "postToolUse" ? {} : { permission: "allow" }));
  process.exit(0);
}

try {
  const event = process.argv[2];

  let hooks;
  try {
    hooks = JSON.parse(process.env.HARNESS_HOOKS || "{}");
  } catch {
    failOpen(event);
  }

  let stdinData = "";

  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    stdinData += chunk;
  });
  process.stdin.on("end", () => {
    let stdinPayload = {};
    try {
      if (stdinData.trim()) {
        stdinPayload = JSON.parse(stdinData);
      }
    } catch {
      // leave as {}
    }

    const toolName =
      stdinPayload.tool_name ||
      stdinPayload.toolName ||
      stdinPayload.name ||
      "";

    function matcherCandidates(name) {
      const candidates = [name];
      if (name === "Shell") candidates.push("Bash");
      if (name === "Bash") candidates.push("Shell");
      if (name.startsWith("MCP:")) {
        candidates.push(`mcp__${name.slice(4).replace(/[:.]/g, "__")}`);
      }
      if (name.startsWith("mcp__")) {
        candidates.push(`MCP:${name.slice(5).replace(/__/g, ":")}`);
      }
      return candidates;
    }

    function testMatcher(matcher, name) {
      if (!matcher) return true;
      try {
        const regex = new RegExp(matcher);
        return matcherCandidates(name).some((candidate) => regex.test(candidate));
      } catch (e) {
        console.error("invalid hook matcher regex:", matcher, e);
        return false;
      }
    }

    function decide() {
      if (event === "preToolUse") {
        for (const rule of hooks.pre_tool_use || []) {
          if (testMatcher(rule.matcher, toolName)) {
            const decision = rule.decision || "allow";
            if (decision === "deny" || decision === "ask") {
              const msg =
                rule.reason || rule.additional_context || "blocked by node hook";
              return { permission: decision, user_message: msg, agent_message: msg };
            }
          }
        }
        return { permission: "allow" };
      }

      if (event === "postToolUse") {
        for (const rule of hooks.post_tool_use || []) {
          if (testMatcher(rule.matcher, toolName)) {
            const parts = [];
            if (rule.additional_context) parts.push(rule.additional_context);
            if (rule.system_message) parts.push(rule.system_message);
            if (parts.length > 0) {
              return { additional_context: parts.join("\n\n") };
            }
          }
        }
        return {};
      }

      return event === "postToolUse" ? {} : { permission: "allow" };
    }

    const result = decide();
    console.log(JSON.stringify(result));
    process.exit(0);
  });
} catch {
  failOpen();
}
