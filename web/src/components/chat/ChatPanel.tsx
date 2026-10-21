import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  IconCheck,
  IconHistory,
  IconLoader2,
  IconPlus,
  IconSend,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useProjects } from "@/lib/projects";
import { agentLabel } from "@/lib/agents";
import { ApiError } from "@/lib/api";
import {
  type ChatContext,
  type ChatMessage,
  contextFromPath,
  describeAction,
  describeContext,
  toolCalls,
  useChatSettings,
  useConversation,
  useConversations,
  useCreateConversation,
  useDeleteConversation,
  useResolveAction,
  useSendMessage,
} from "@/lib/chat";
import { useChatOpen } from "./ChatProvider";

/** Base UI's Select needs a concrete value for "none". */
const NO_PROJECT = "__none__";

/** Which conversation the panel shows, remembered per browser. */
const CURRENT_KEY = "harness.chat.current";

function readCurrent(): string | null {
  try {
    return window.localStorage.getItem(CURRENT_KEY);
  } catch {
    return null;
  }
}

function writeCurrent(id: string | null) {
  try {
    if (id) window.localStorage.setItem(CURRENT_KEY, id);
    else window.localStorage.removeItem(CURRENT_KEY);
  } catch {
    // Unwritable storage only means the panel opens on a new conversation.
  }
}

/**
 * The chat panel, mounted once beside the routes and opened from the header.
 * Its contents mount only while it is open, so nothing here asks the server
 * anything until someone opens it.
 */
export function ChatPanel() {
  const { open, setOpen } = useChatOpen();
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetContent
        side="right"
        className="w-full gap-0 p-0 sm:max-w-lg data-[side=right]:sm:w-[28rem]"
      >
        <ChatBody />
      </SheetContent>
    </Sheet>
  );
}

function ChatBody() {
  const location = useLocation();
  const context = contextFromPath(location.pathname);
  const [currentId, setCurrentIdState] = useState<string | null>(readCurrent);
  const setCurrentId = (id: string | null) => {
    setCurrentIdState(id);
    writeCurrent(id);
  };

  const settings = useChatSettings();
  // The project whose code a *new* conversation may read. Claude only: Pi's
  // file tools can't be kept inside the checkout, so it isn't offered there.
  const [project, setProject] = useState<string | null>(null);
  const canReadCode = settings.data?.provider === "claude";
  const projects = useProjects();
  const conversations = useConversations(true);
  const detail = useConversation(currentId);
  const create = useCreateConversation();
  const send = useSendMessage();
  const remove = useDeleteConversation();

  // A remembered conversation that is gone (deleted, or another account's on
  // a shared browser) is forgotten rather than shown as an error.
  useEffect(() => {
    if (detail.error instanceof ApiError && detail.error.status === 404) {
      setCurrentId(null);
    }
  }, [detail.error]);

  const conv = detail.data?.conversation;
  const running = detail.data?.running ?? null;
  const busy = !!running || create.isPending || send.isPending;
  const provider = conv?.provider ?? settings.data?.provider ?? null;
  const model = conv ? conv.model : (settings.data?.model ?? null);
  const unavailable = settings.isSuccess && !settings.data.provider && !conv;

  async function submit(text: string) {
    const trimmed = text.trim();
    if (!trimmed || busy) return;
    let id = currentId;
    if (!id) {
      const created = await create.mutateAsync(
        canReadCode && project ? { ...context, project } : context,
      );
      id = created.id;
      setCurrentId(id);
    }
    await send.mutateAsync({ id, text: trimmed, context });
  }

  const error = create.error ?? send.error ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b py-2.5 pr-12 pl-4">
        <div className="min-w-0 flex-1">
          <SheetTitle className="truncate text-sm">
            {conv?.title || "Ask the harness"}
          </SheetTitle>
          <SheetDescription className="truncate text-[11px]">
            {provider
              ? `${agentLabel(provider)}${model ? ` · ${model}` : ""}${
                  conv?.context.project
                    ? ` · reading ${conv.context.project}`
                    : ""
                }`
              : "Runs, pull requests, Linear and workflows"}
          </SheetDescription>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Conversations"
              />
            }
          >
            <IconHistory />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-72">
            {(conversations.data?.conversations ?? []).length === 0 && (
              <DropdownMenuItem disabled>No conversations yet</DropdownMenuItem>
            )}
            {(conversations.data?.conversations ?? []).map((c) => (
              <DropdownMenuItem
                key={c.id}
                onClick={() => setCurrentId(c.id)}
                className="truncate"
              >
                {c.title || "Untitled"}
              </DropdownMenuItem>
            ))}
            {currentId && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  disabled={!!running}
                  onClick={() => {
                    const id = currentId;
                    setCurrentId(null);
                    remove.mutate(id);
                  }}
                >
                  <IconTrash />
                  Delete this conversation
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="New conversation"
          disabled={!currentId || busy}
          onClick={() => setCurrentId(null)}
        >
          <IconPlus />
        </Button>
      </div>

      <MessageList
        messages={detail.data?.messages ?? []}
        running={running}
        conversationId={currentId}
        context={context}
        empty={!currentId}
        onSuggest={(s) => void submit(s)}
      />

      <div className="border-t p-3">
        {!currentId && canReadCode && (projects.data ?? []).length > 0 && (
          <div className="mb-2 flex items-center gap-2">
            <span className="text-[11px] text-muted-foreground">
              Read code from
            </span>
            <Select
              value={project ?? NO_PROJECT}
              onValueChange={(v) =>
                v != null && setProject(v === NO_PROJECT ? null : v)
              }
            >
              <SelectTrigger size="sm" className="min-w-36">
                {/* Base UI shows the raw value unless told its label. */}
                <SelectValue>
                  {(v: string | null) =>
                    !v || v === NO_PROJECT ? "No project" : v
                  }
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NO_PROJECT}>No project</SelectItem>
                {(projects.data ?? []).map((p) => (
                  <SelectItem key={p.name} value={p.name}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
        {unavailable ? (
          <p className="text-[12px] text-muted-foreground">
            No agent can run the chat yet. Connect Claude Code or Pi under{" "}
            <Link to="/settings/agents" className="underline">
              Settings → Agents
            </Link>
            .
          </p>
        ) : (
          <Composer
            disabled={busy}
            onSubmit={(text) => submit(text)}
            placeholder={
              running ? "Answering…" : "Ask about a run, a PR, or a workflow"
            }
          />
        )}
        {error && (
          <p className="mt-2 text-[11px] text-destructive">{error.message}</p>
        )}
      </div>
    </div>
  );
}

/** Starting points, picked by what is open. */
function suggestions(context: ChatContext): string[] {
  if (context.run_id) {
    return ["Why did this run fail?", "What is this run doing right now?"];
  }
  if (context.workflow) {
    return [
      "Explain what this workflow does, step by step.",
      "Which models does this workflow use?",
    ];
  }
  return [
    "What are the agents failing on most this week?",
    "Which runs failed today, and why?",
  ];
}

function MessageList({
  messages,
  running,
  conversationId,
  context,
  empty,
  onSuggest,
}: {
  messages: ChatMessage[];
  running: { activity: { kind: string; text: string }[] } | null;
  conversationId: string | null;
  context: ChatContext;
  empty: boolean;
  onSuggest: (text: string) => void;
}) {
  const end = useRef<HTMLDivElement>(null);
  const lastActivity = running?.activity.at(-1);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [messages.length, running?.activity.length]);

  if (empty) {
    const open = describeContext(context);
    return (
      <div className="flex min-h-0 flex-1 flex-col justify-end gap-3 overflow-auto p-4">
        <p className="text-[12px] text-muted-foreground">
          Ask why a run failed, which pull request to test for a Linear issue,
          why an issue didn't start, or have it build a workflow with you.
          {open && <> It can see you have {open} open.</>}
        </p>
        <div className="flex flex-col items-start gap-1.5">
          {suggestions(context).map((s) => (
            <Button
              key={s}
              variant="outline"
              size="sm"
              className="h-auto py-1.5 text-left whitespace-normal"
              onClick={() => onSuggest(s)}
            >
              {s}
            </Button>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-4">
      {messages.map((m) => (
        <Message key={m.id} message={m} conversationId={conversationId} />
      ))}
      {running && (
        <div className="flex items-center gap-2 text-[12px] text-muted-foreground">
          <IconLoader2 className="size-3.5 animate-spin" />
          {lastActivity?.kind === "tool"
            ? `Using ${lastActivity.text}…`
            : "Thinking…"}
        </div>
      )}
      <div ref={end} />
    </div>
  );
}

function Message({
  message,
  conversationId,
}: {
  message: ChatMessage;
  conversationId: string | null;
}) {
  switch (message.role) {
    case "user":
      return (
        <div className="ml-8 self-end rounded-lg bg-muted px-3 py-2 text-[13px] whitespace-pre-wrap">
          {message.body}
        </div>
      );
    case "assistant": {
      const tools = toolCalls(message.activity);
      return (
        <div className="flex flex-col gap-1">
          <Markdown className="text-[13px]">{message.body}</Markdown>
          {tools.length > 0 && (
            <div className="text-[11px] text-muted-foreground">
              Looked at: {[...new Set(tools.map((t) => t.text))].join(", ")}
            </div>
          )}
        </div>
      );
    }
    case "action":
      return message.action ? (
        <ActionCard
          id={message.id}
          action={message.action}
          conversationId={conversationId}
        />
      ) : null;
    default:
      return (
        <div className="text-[12px] text-destructive">
          Couldn't answer: {message.body}
        </div>
      );
  }
}

function ActionCard({
  id,
  action,
  conversationId,
}: {
  id: number;
  action: NonNullable<ChatMessage["action"]>;
  conversationId: string | null;
}) {
  const resolve = useResolveAction(conversationId);
  const pending = action.status === "pending";
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-2 px-3 py-2.5">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1 text-[13px] font-medium">
            {describeAction(action)}
          </div>
          {!pending && (
            <Badge
              variant={
                action.status === "confirmed"
                  ? "secondary"
                  : action.status === "failed"
                    ? "destructive"
                    : "outline"
              }
            >
              {action.status}
            </Badge>
          )}
        </div>
        {typeof action.args?.description === "string" && (
          <p className="line-clamp-4 text-[12px] text-muted-foreground">
            {action.args.description}
          </p>
        )}
        {action.result && !pending && (
          <p className="text-[11px] text-muted-foreground">{action.result}</p>
        )}
        {pending && (
          <div className="flex gap-2">
            <Button
              size="sm"
              disabled={resolve.isPending}
              onClick={() => resolve.mutate({ id, confirm: true })}
            >
              <IconCheck />
              Confirm
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={resolve.isPending}
              onClick={() => resolve.mutate({ id, confirm: false })}
            >
              <IconX />
              Cancel
            </Button>
          </div>
        )}
        {resolve.isError && (
          <p className="text-[11px] text-destructive">
            {resolve.error.message}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function Composer({
  disabled,
  placeholder,
  onSubmit,
}: {
  disabled: boolean;
  placeholder: string;
  onSubmit: (text: string) => Promise<void>;
}) {
  const [text, setText] = useState("");
  const go = () => {
    if (!text.trim() || disabled) return;
    const sent = text;
    setText("");
    // Put it back if it didn't go, so nothing typed is lost.
    onSubmit(sent).catch(() => setText(sent));
  };
  return (
    <form
      className="flex items-end gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        go();
      }}
    >
      <Textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // Enter sends; Shift+Enter is a new line.
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            go();
          }
        }}
        placeholder={placeholder}
        aria-label="Message"
        className="max-h-40 min-h-9 resize-none text-[13px]"
        rows={1}
      />
      <Button
        type="submit"
        size="icon"
        aria-label="Send"
        disabled={disabled || !text.trim()}
      >
        <IconSend />
      </Button>
    </form>
  );
}
