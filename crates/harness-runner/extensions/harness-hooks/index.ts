/**
 * harness-hooks — omp extension for per-node tool hooks and the loop guard.
 *
 * On load, reads HARNESS_HOOKS (serialized NodeHooks JSON, snake_case keys)
 * and registers pre/post tool handlers. When the step runs inside a
 * server-started run (HARNESS_RUN_URL, HARNESS_RUN_TOKEN, HARNESS_NODE_ID), it
 * also asks the harness's loop guard after each failing tool call whether a
 * redirect is waiting for this step, and hands it to the model.
 */

interface HookRule {
  matcher?: string;
  decision?: "allow" | "deny" | "ask";
  reason?: string;
  additional_context?: string;
  system_message?: string;
}

interface NodeHooks {
  pre_tool_use: HookRule[];
  post_tool_use: HookRule[];
}

interface ToolCall {
  toolName: string;
}

interface ToolResult {
  toolName: string;
  isError?: boolean;
}

type ToolResultAnswer = { additionalContext?: string } | void;

interface PiApi {
  on(event: "tool_call", handler: (call: ToolCall) => { block: boolean; reason: string } | void): void;
  on(
    event: "tool_result",
    handler: (res: ToolResult) => ToolResultAnswer | Promise<ToolResultAnswer>,
  ): void;
  sendMessage(content: string, opts: { deliverAs: "steer" }): void;
}

declare const pi: PiApi;

function testMatcher(matcher: string | undefined, toolName: string): boolean {
  if (!matcher) return true;
  try {
    return new RegExp(matcher).test(toolName);
  } catch {
    console.error(`[harness-hooks] Invalid matcher regex: ${matcher}`);
    return false;
  }
}

function registerHooks(): void {
  const raw = process.env.HARNESS_HOOKS;
  if (!raw) return;

  let hooks: NodeHooks;
  try {
    hooks = JSON.parse(raw) as unknown as NodeHooks;
  } catch {
    console.error("[harness-hooks] HARNESS_HOOKS is not valid JSON; skipping hook registration.");
    return;
  }
  if (!hooks || typeof hooks !== "object") {
    console.error("[harness-hooks] HARNESS_HOOKS did not parse to an object; skipping hook registration.");
    return;
  }

  if (hooks.pre_tool_use?.length) {
    pi.on("tool_call", (call: ToolCall) => {
      for (const rule of hooks.pre_tool_use) {
        const matches = testMatcher(rule.matcher, call.toolName);
        if (matches && rule.decision === "deny") {
          return {
            block: true,
            reason: rule.reason ?? "blocked by node hook",
          };
        }
      }
    });
  }

  if (hooks.post_tool_use?.length) {
    pi.on("tool_result", (res: ToolResult) => {
      for (const rule of hooks.post_tool_use) {
        const matches = testMatcher(rule.matcher, res.toolName);
        if (matches) {
          if (rule.additional_context) {
            pi.sendMessage(rule.additional_context, { deliverAs: "steer" });
          }
          if (rule.system_message) {
            pi.sendMessage(rule.system_message, { deliverAs: "steer" });
          }
        }
      }
    });
  }
}

/**
 * The loop guard. A redirect can only be waiting after a failure, so a
 * successful call asks nothing. Any problem reaching the server means no
 * note: the guard must never get in the way of the step it watches.
 */
function registerLoopGuard(): void {
  const base = process.env.HARNESS_RUN_URL?.trim().replace(/\/+$/, "");
  const token = process.env.HARNESS_RUN_TOKEN?.trim();
  const node = process.env.HARNESS_NODE_ID?.trim();
  if (!base || !token || !node) return;

  pi.on("tool_result", async (res: ToolResult): Promise<ToolResultAnswer> => {
    if (!res.isError) return;
    try {
      const resp = await fetch(
        `${base}/api/run/guard?node=${encodeURIComponent(node)}`,
        {
          headers: { Authorization: `Bearer ${token}` },
          signal: AbortSignal.timeout(3000),
        },
      );
      if (!resp.ok) return;
      const body = (await resp.json()) as { note?: string };
      if (body.note?.trim()) return { additionalContext: body.note };
    } catch {
      // Unreachable or slow: no note.
    }
  });
}

registerHooks();
registerLoopGuard();

export {};
