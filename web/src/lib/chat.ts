/**
 * The built-in chat: conversations with an agent whose only tools are the
 * harness's own (runs, PRs, Linear wiring, workflow authoring).
 *
 * A message is answered asynchronously — the POST returns at once and the
 * answer arrives on a later read of the conversation — so the conversation is
 * polled while a turn is running and not otherwise.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";

/** One agent the chat can run on, with the models it offers. */
export interface ChatOption {
  id: string;
  label: string;
  models: string[];
}

export interface ChatSettings {
  /** What new conversations run on; null when no agent can. */
  provider: string | null;
  model: string | null;
  effort: string | null;
  /** What an administrator chose, as opposed to the default. */
  stored: {
    provider: string | null;
    model: string | null;
    effort: string | null;
  };
  options: ChatOption[];
}

export interface ChatContext {
  run_id?: string;
  workflow?: string;
  project?: string;
}

export interface Conversation {
  id: string;
  title: string;
  provider: string;
  model: string | null;
  context: ChatContext;
  created_at: string;
  updated_at: string;
}

/** A tool call or result the agent made while answering. */
export interface ChatActivity {
  kind: "text" | "tool" | "tool_result";
  text: string;
  detail?: string;
  tool_id?: string;
  is_error?: boolean;
}

/** A tool call held for the person to confirm. */
export interface ChatAction {
  tool: string;
  args: Record<string, unknown>;
  status: "pending" | "confirmed" | "failed" | "cancelled";
  /** What the tool said, once run. */
  result?: string;
}

export interface ChatMessage {
  id: number;
  conversation_id: string;
  role: "user" | "assistant" | "action" | "error";
  body: string;
  activity: ChatActivity[];
  action: ChatAction | null;
  created_at: string;
}

export interface ConversationDetail {
  conversation: Conversation;
  messages: ChatMessage[];
  /** Present while the agent is answering. */
  running: { activity: ChatActivity[] } | null;
}

/**
 * What the person has open, read from the URL, so "this run" and "this
 * workflow" mean something to the agent.
 */
export function contextFromPath(pathname: string): ChatContext {
  const run = /^\/runs\/(?!pair\/)([^/]+)\/?$/.exec(pathname);
  if (run) return { run_id: decodeURIComponent(run[1]) };
  const editor = /^\/editor\/(?!new\/?$)([^/]+)\/?$/.exec(pathname);
  if (editor) return { workflow: decodeURIComponent(editor[1]) };
  return {};
}

/** A one-line account of what is open, for the panel's empty state. */
export function describeContext(ctx: ChatContext): string | null {
  if (ctx.run_id) return `run ${ctx.run_id.slice(0, 8)}`;
  if (ctx.workflow) return `the ${ctx.workflow} workflow`;
  if (ctx.project) return `project ${ctx.project}`;
  return null;
}

/**
 * The tool calls worth showing as "what it did": the calls themselves, not
 * their results or the prose between them.
 */
export function toolCalls(activity: ChatActivity[]): ChatActivity[] {
  return activity.filter((a) => a.kind === "tool");
}

/** What a held tool call will do, in words a confirmation card can use. */
export function describeAction(action: ChatAction): string {
  const arg = (k: string) =>
    typeof action.args?.[k] === "string" ? (action.args[k] as string) : "";
  switch (action.tool) {
    case "run_trigger": {
      const wf = arg("workflow") || "the project's default workflow";
      const project = arg("project");
      return `Start a run of ${wf}${project ? ` in ${project}` : ""}.`;
    }
    case "run_trigger_pair": {
      const wf = arg("workflow") || "the project's default workflow";
      const project = arg("project");
      return `Start an A/B pair of ${wf}${project ? ` in ${project}` : ""}.`;
    }
    case "workflow_delete":
      return `Delete the workflow ${arg("name")}. It stays in the bin for 14 days.`;
    case "library_uninstall":
      return `Uninstall ${arg("name") || "the workflow"} from the library.`;
    default:
      return `Run ${action.tool}.`;
  }
}

/** How often to read the conversation while the agent is answering. */
const POLL_MS = 1500;

export function useChatSettings() {
  return useQuery<ChatSettings, Error>({
    queryKey: ["chat", "settings"],
    queryFn: ({ signal }) =>
      apiJson<ChatSettings>("/api/chat/settings", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export function useSetChatSettings() {
  const qc = useQueryClient();
  return useMutation<
    ChatSettings,
    Error,
    { provider: string | null; model: string | null; effort: string | null }
  >({
    mutationFn: (body) =>
      apiJson<ChatSettings>("/api/chat/settings", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => qc.setQueryData(["chat", "settings"], data),
  });
}

export function useConversations(enabled: boolean) {
  return useQuery<{ conversations: Conversation[] }, Error>({
    queryKey: ["chat", "conversations"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<{ conversations: Conversation[] }>("/api/chat/conversations", {
        signal,
      }),
    refetchInterval: false,
  });
}

export function useConversation(id: string | null) {
  return useQuery<ConversationDetail, Error>({
    queryKey: ["chat", "conversation", id],
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiJson<ConversationDetail>(`/api/chat/conversations/${id}`, { signal }),
    // Poll only while an answer is being written.
    refetchInterval: (q) => (q.state.data?.running ? POLL_MS : false),
  });
}

export function useCreateConversation() {
  const qc = useQueryClient();
  return useMutation<Conversation, Error, ChatContext>({
    mutationFn: (context) =>
      apiJson<Conversation>("/api/chat/conversations", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ context }),
      }),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["chat", "conversations"] }),
  });
}

export function useDeleteConversation() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    // `apiFetch`, not `apiJson`: a 204 has no body to parse.
    mutationFn: async (id) => {
      await apiFetch(`/api/chat/conversations/${id}`, { method: "DELETE" });
    },
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["chat", "conversations"] }),
  });
}

/**
 * Send a message. The conversation is named per call rather than per hook: a
 * first message creates the conversation and sends to it in one go.
 */
export function useSendMessage() {
  const qc = useQueryClient();
  return useMutation<
    unknown,
    Error,
    { id: string; text: string; context: ChatContext }
  >({
    mutationFn: ({ id, text, context }) =>
      apiJson<unknown>(`/api/chat/conversations/${id}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text, context }),
      }),
    onSuccess: (_, { id }) => {
      qc.invalidateQueries({ queryKey: ["chat", "conversation", id] });
      qc.invalidateQueries({ queryKey: ["chat", "conversations"] });
    },
  });
}

export function useResolveAction(conversationId: string | null) {
  const qc = useQueryClient();
  return useMutation<unknown, Error, { id: number; confirm: boolean }>({
    mutationFn: ({ id, confirm }) =>
      apiJson<unknown>(`/api/chat/actions/${id}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ confirm }),
      }),
    onSuccess: () => {
      qc.invalidateQueries({
        queryKey: ["chat", "conversation", conversationId],
      });
      // A confirmed run shows up on the dashboard.
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}
