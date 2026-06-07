/**
 * harness-hooks — omp extension for per-node tool hooks.
 *
 * On load, reads HARNESS_HOOKS (serialized NodeHooks JSON, snake_case keys)
 * and registers pre/post tool handlers.
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
}

interface PiApi {
  on(event: "tool_call", handler: (call: ToolCall) => { block: boolean; reason: string } | void): void;
  on(event: "tool_result", handler: (res: ToolResult) => void): void;
  sendMessage(content: string, opts: { deliverAs: "steer" }): void;
}

declare const pi: PiApi;

const raw = process.env.HARNESS_HOOKS;
if (raw) {
  const hooks = JSON.parse(raw) as unknown as NodeHooks;

  if (hooks.pre_tool_use?.length) {
    pi.on("tool_call", (call: ToolCall) => {
      for (const rule of hooks.pre_tool_use) {
        const matches = !rule.matcher || new RegExp(rule.matcher).test(call.toolName);
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
        const matches = !rule.matcher || new RegExp(rule.matcher).test(res.toolName);
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

export {};
