import { Fragment, type ReactNode, useEffect, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { cn } from "@/lib/utils";
import {
  IconAlertTriangle,
  IconArrowsMaximize,
  IconCheck,
  IconChevronDown,
  IconFolder,
  IconHistory,
  IconPlayerPlay,
  IconPlus,
  IconSend,
  IconSparkles,
  IconTool,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import { Markdown } from "@/components/Markdown";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Message, MessageContent } from "@/components/ui/message";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
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
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
  InputGroupText,
} from "@/components/ui/input-group";
import { useProjects } from "@/lib/projects";
import { agentLabel } from "@/lib/agents";
import { ApiError } from "@/lib/api";
import {
  type ChatActivity,
  type ChatContext,
  type ChatMessage,
  type ChatOption,
  type ChatSettings,
  type ToolStep,
  contextFromPath,
  describeAction,
  describeContext,
  toolSteps,
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
        <ChatView variant="panel" />
      </SheetContent>
    </Sheet>
  );
}

/**
 * The conversation view: in the side panel, or full-size on the Chat page.
 * Both read and write the same remembered conversation, so moving from one to
 * the other keeps your place.
 */
export function ChatView({ variant }: { variant: "panel" | "page" }) {
  const location = useLocation();
  const navigate = useNavigate();
  const { setOpen } = useChatOpen();
  const context = contextFromPath(location.pathname);
  const [currentId, setCurrentIdState] = useState<string | null>(readCurrent);
  const setCurrentId = (id: string | null) => {
    setCurrentIdState(id);
    writeCurrent(id);
  };

  const settings = useChatSettings();
  // The project whose code a *new* conversation may read. Both chat agents
  // can: Claude with its own confined tools, Pi through the harness's guard.
  const [project, setProject] = useState<string | null>(null);
  const canReadCode = !!settings.data?.provider;
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

  // The agent and model a *new* conversation runs on, when picked here rather
  // than left to the chat setting. A pick that can no longer run (its agent
  // disconnected since) falls back to the setting.
  const [picked, setPicked] = useState<AgentChoice | null>(null);
  const choice = validChoice(picked, settings.data);

  const conv = detail.data?.conversation;
  const running = detail.data?.running ?? null;
  const busy = !!running || create.isPending || send.isPending;
  const provider =
    conv?.provider ?? choice?.provider ?? settings.data?.provider ?? null;
  const model = conv
    ? conv.model
    : (choice?.model ?? settings.data?.model ?? null);
  const unavailable = settings.isSuccess && !settings.data.provider && !conv;

  async function submit(text: string) {
    const trimmed = text.trim();
    if (!trimmed || busy) return;
    let id = currentId;
    if (!id) {
      const created = await create.mutateAsync({
        context: canReadCode && project ? { ...context, project } : context,
        provider: choice?.provider,
        model: choice?.model,
      });
      id = created.id;
      setCurrentId(id);
    }
    await send.mutateAsync({ id, text: trimmed, context });
  }

  const error = create.error ?? send.error ?? null;
  const title = conv?.title || "Ask the harness";
  const subtitle = provider
    ? `${agentLabel(provider)}${model ? ` · ${model}` : ""}${
        conv?.context.project ? ` · reading ${conv.context.project}` : ""
      }`
    : "Runs, pull requests, Linear and workflows";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div
        className={cn(
          "flex items-center gap-2 border-b py-2.5 pl-4",
          // The panel's own close button sits in the top-right corner.
          variant === "panel" ? "pr-12" : "pr-4",
        )}
      >
        <div className="min-w-0 flex-1">
          {variant === "panel" ? (
            <>
              <SheetTitle className="truncate text-sm">{title}</SheetTitle>
              <SheetDescription className="truncate text-[11px]">
                {subtitle}
              </SheetDescription>
            </>
          ) : (
            <>
              <h2 className="truncate text-sm font-medium">{title}</h2>
              <p className="truncate text-[11px] text-muted-foreground">
                {subtitle}
              </p>
            </>
          )}
        </div>
        {variant === "panel" && (
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Open the chat page"
            title="Open the chat page"
            onClick={() => {
              setOpen(false);
              navigate("/chat");
            }}
          >
            <IconArrowsMaximize />
          </Button>
        )}
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
                <span className="truncate">{c.title || "Untitled"}</span>
                {/* Asked by mentioning the harness in Linear; it answered there. */}
                {c.context.linear_issue && (
                  <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
                    {c.context.linear_issue}
                  </span>
                )}
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
            model={
              conv ? (
                // Fixed once it starts, like the project: the agent keeps its
                // own session, so switching mid-way would lose the thread.
                <InputGroupText
                  className="px-1.5 text-[12px]"
                  title="This conversation's agent and model"
                >
                  {modelName(conv.provider, conv.model)}
                </InputGroupText>
              ) : settings.data && settings.data.options.length > 0 ? (
                <ModelMenu
                  options={settings.data.options}
                  provider={provider}
                  model={model}
                  onPick={setPicked}
                />
              ) : null
            }
            toolbar={
              conv ? (
                // A conversation's project is fixed when it starts: shown,
                // not offered.
                conv.context.project ? (
                  <InputGroupText
                    className="px-1.5 text-[12px]"
                    title={`This conversation reads ${conv.context.project}'s code`}
                  >
                    <IconFolder />
                    {conv.context.project}
                  </InputGroupText>
                ) : null
              ) : canReadCode && (projects.data ?? []).length > 0 ? (
                <Select
                  value={project ?? NO_PROJECT}
                  onValueChange={(v) =>
                    v != null && setProject(v === NO_PROJECT ? null : v)
                  }
                >
                  <SelectTrigger
                    size="sm"
                    aria-label="Project whose code to read"
                    title="Project whose code the answer may read"
                    className="h-6 gap-1 border-transparent bg-transparent px-1.5 text-[12px] text-muted-foreground shadow-none hover:bg-muted hover:text-foreground dark:bg-transparent dark:hover:bg-muted"
                  >
                    <IconFolder className="size-3.5" />
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
              ) : null
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
  running: { activity: ChatActivity[] } | null;
  conversationId: string | null;
  context: ChatContext;
  empty: boolean;
  onSuggest: (text: string) => void;
}) {
  if (empty) {
    const open = describeContext(context);
    return (
      <div className="flex min-h-0 flex-1 flex-col justify-end gap-3 p-4">
        <p className="text-[12px] text-muted-foreground">
          Ask why a run failed, which pull request to test for a Linear issue,
          why an issue didn't start, or have it build a workflow with you.
          {open && <> It can see you have {open} open.</>}
        </p>
        {/* Suggestions: one row that scrolls sideways rather than wraps. */}
        <div className="flex scroll-fade-x scrollbar-none gap-2 overflow-x-auto overscroll-x-contain">
          {suggestions(context).map((s) => (
            <Button
              key={s}
              variant="outline"
              size="sm"
              className="shrink-0"
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
    <MessageScrollerProvider autoScroll>
      <MessageScroller className="flex-1">
        <MessageScrollerViewport>
          <MessageScrollerContent aria-busy={!!running} className="gap-5 p-4">
            {messages.map((m) => (
              <MessageScrollerItem
                key={m.id}
                messageId={String(m.id)}
                // A question is where reading picks up: it is kept in view
                // when sent, and the answer fills in below it.
                scrollAnchor={m.role === "user"}
              >
                <ChatMessageView message={m} conversationId={conversationId} />
              </MessageScrollerItem>
            ))}
            {running && (
              <MessageScrollerItem messageId="running">
                <Message>
                  <MessageContent>
                    <Steps steps={toolSteps(running.activity)} live />
                  </MessageContent>
                </Message>
              </MessageScrollerItem>
            )}
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton />
      </MessageScroller>
    </MessageScrollerProvider>
  );
}

function ChatMessageView({
  message,
  conversationId,
}: {
  message: ChatMessage;
  conversationId: string | null;
}) {
  switch (message.role) {
    case "user":
      return (
        <Message align="end">
          <MessageContent>
            <Bubble align="end" variant="muted">
              <BubbleContent className="text-[13px] whitespace-pre-wrap">
                {message.body}
              </BubbleContent>
            </Bubble>
          </MessageContent>
        </Message>
      );
    case "assistant": {
      const steps = toolSteps(message.activity);
      return (
        <Message>
          <MessageContent className="gap-3">
            {steps.length > 0 && <Steps steps={steps} />}
            <Markdown className="text-[13px]">{message.body}</Markdown>
          </MessageContent>
        </Message>
      );
    }
    case "action":
      return message.action ? (
        <ActionView
          id={message.id}
          action={message.action}
          conversationId={conversationId}
        />
      ) : null;
    default:
      return (
        <Message>
          <MessageContent>
            <Trace>
              <TraceLine status="error">
                <TraceIcon>
                  <IconAlertTriangle />
                </TraceIcon>
                <TraceTitle className="whitespace-normal">
                  Couldn't answer: {message.body}
                </TraceTitle>
              </TraceLine>
            </Trace>
          </MessageContent>
        </Message>
      );
  }
}

/**
 * The tool calls behind an answer, one line each; a line with something to
 * show opens to the call's input and what came back. `live` is the answer
 * still being written: the call that is out shimmers, and with none out it
 * says it is thinking.
 */
function Steps({ steps, live = false }: { steps: ToolStep[]; live?: boolean }) {
  const waiting = live && !steps.some((s) => !s.done);
  return (
    <Trace>
      {steps.map((step, i) => (
        <StepLine key={i} step={step} live={live} />
      ))}
      {waiting && (
        <TraceLine status="running">
          <TraceIcon>
            <IconSparkles />
          </TraceIcon>
          <TraceTitle>Thinking…</TraceTitle>
        </TraceLine>
      )}
    </Trace>
  );
}

function StepLine({ step, live }: { step: ToolStep; live: boolean }) {
  const status = step.error
    ? "error"
    : live && !step.done
      ? "running"
      : undefined;
  const line = (
    <TraceLine status={status}>
      <TraceIcon>{step.error ? <IconAlertTriangle /> : <IconTool />}</TraceIcon>
      <TraceTitle>
        {live && !step.done ? `Using ${step.name}…` : step.name}
      </TraceTitle>
    </TraceLine>
  );
  if (!step.input && !step.output) return <div>{line}</div>;
  return (
    <TraceCollapsible>
      <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
      <TraceCollapsibleContent className="flex flex-col gap-1.5 text-[12px]">
        {step.input && (
          <p className="font-mono break-all whitespace-pre-wrap">
            {step.input}
          </p>
        )}
        {step.output && (
          <pre
            className={cn(
              "max-h-48 overflow-auto font-mono text-[11px] whitespace-pre-wrap",
              step.error && "text-destructive",
            )}
          >
            {step.output}
          </pre>
        )}
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

/**
 * A tool call held for a yes or no. While it waits it asks, with Cancel and
 * Confirm; once answered it is a plain line saying what became of it.
 */
function ActionView({
  id,
  action,
  conversationId,
}: {
  id: number;
  action: NonNullable<ChatMessage["action"]>;
  conversationId: string | null;
}) {
  const resolve = useResolveAction(conversationId);
  const description =
    typeof action.args?.description === "string"
      ? action.args.description
      : null;

  if (action.status !== "pending") {
    const failed = action.status === "failed";
    const outcome =
      action.status === "confirmed"
        ? "Done"
        : action.status === "cancelled"
          ? "Cancelled"
          : "Failed";
    const line = (
      <TraceLine status={failed ? "error" : undefined}>
        <TraceIcon>
          {failed ? (
            <IconAlertTriangle />
          ) : action.status === "cancelled" ? (
            <IconX />
          ) : (
            <IconCheck />
          )}
        </TraceIcon>
        <TraceTitle>
          {outcome}: {describeAction(action)}
        </TraceTitle>
      </TraceLine>
    );
    return (
      <Trace>
        {action.result ? (
          <TraceCollapsible>
            <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
            <TraceCollapsibleContent className="text-[12px] whitespace-pre-wrap">
              {action.result}
            </TraceCollapsibleContent>
          </TraceCollapsible>
        ) : (
          <div>{line}</div>
        )}
      </Trace>
    );
  }

  return (
    <Item variant="outline" size="sm">
      <ItemMedia variant="icon">
        <IconPlayerPlay />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="text-[13px]">{describeAction(action)}</ItemTitle>
        {description && (
          <ItemDescription className="line-clamp-4 text-[12px]">
            {description}
          </ItemDescription>
        )}
        {resolve.isError && (
          <ItemDescription className="text-[11px] text-destructive">
            {resolve.error.message}
          </ItemDescription>
        )}
      </ItemContent>
      <ItemActions>
        <Button
          size="sm"
          variant="ghost"
          disabled={resolve.isPending}
          onClick={() => resolve.mutate({ id, confirm: false })}
        >
          Cancel
        </Button>
        <Button
          size="sm"
          disabled={resolve.isPending}
          onClick={() => resolve.mutate({ id, confirm: true })}
        >
          Confirm
        </Button>
      </ItemActions>
    </Item>
  );
}

/** An agent and model picked for a new conversation. */
interface AgentChoice {
  provider: string;
  model: string;
}

/** The pick, if the agent can still run it; else null (use the setting). */
function validChoice(
  picked: AgentChoice | null,
  settings: ChatSettings | undefined,
): AgentChoice | null {
  if (!picked || !settings) return null;
  const option = settings.options.find((o) => o.id === picked.provider);
  if (!option) return null;
  const isDefault =
    settings.provider === picked.provider && settings.model === picked.model;
  return option.models.includes(picked.model) || isDefault ? picked : null;
}

/** "Claude Code · opus", or just the agent when no model is named. */
function modelName(provider: string, model: string | null): string {
  return model ? `${agentLabel(provider)} · ${model}` : agentLabel(provider);
}

/**
 * Which agent and model a new conversation runs on: every model the chat's
 * agents offer, grouped by agent. Starts on the chat setting's choice.
 */
function ModelMenu({
  options,
  provider,
  model,
  onPick,
}: {
  options: ChatOption[];
  provider: string | null;
  model: string | null;
  onPick: (choice: AgentChoice) => void;
}) {
  // Model ids repeat across agents, so a radio value names both. Ids may
  // contain "/" (`kimi-code/k2`), so the separator is one they cannot.
  const SEP = "\u0000";
  const value = provider && model ? `${provider}${SEP}${model}` : "";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <InputGroupButton
            size="xs"
            variant="ghost"
            className="min-w-0 text-[12px] text-muted-foreground"
            aria-label={`Model: ${provider ? modelName(provider, model) : "none"}`}
            title="Agent and model for this conversation"
          />
        }
      >
        <span className="truncate">{model ?? "Model"}</span>
        <IconChevronDown />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuRadioGroup
          value={value}
          onValueChange={(v: string) => {
            const [p, m] = v.split(SEP);
            if (p && m) onPick({ provider: p, model: m });
          }}
        >
          {options.map((option, i) => (
            <Fragment key={option.id}>
              {i > 0 && <DropdownMenuSeparator />}
              <DropdownMenuGroup>
                <DropdownMenuLabel>{option.label}</DropdownMenuLabel>
                {option.models.map((m) => (
                  <DropdownMenuRadioItem
                    key={m}
                    value={`${option.id}${SEP}${m}`}
                    closeOnClick
                  >
                    {m}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuGroup>
            </Fragment>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * The message box: one rounded field with the text on top and a toolbar
 * below it — `toolbar` on the left (the project picker), `model` and send on
 * the right.
 */
function Composer({
  disabled,
  placeholder,
  onSubmit,
  toolbar,
  model,
}: {
  disabled: boolean;
  placeholder: string;
  onSubmit: (text: string) => Promise<void>;
  toolbar?: ReactNode;
  model?: ReactNode;
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
      onSubmit={(e) => {
        e.preventDefault();
        go();
      }}
    >
      <InputGroup>
        <InputGroupTextarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends; Shift+Enter is a new line.
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing
            ) {
              e.preventDefault();
              go();
            }
          }}
          placeholder={placeholder}
          aria-label="Message"
          className="max-h-40 min-h-12 text-[13px]"
          rows={2}
        />
        <InputGroupAddon align="block-end">
          {toolbar}
          <div className="ml-auto flex min-w-0 items-center gap-1">
            {model}
            <InputGroupButton
              type="submit"
              variant="default"
              size="icon-xs"
              aria-label="Send"
              disabled={disabled || !text.trim()}
            >
              <IconSend />
            </InputGroupButton>
          </div>
        </InputGroupAddon>
      </InputGroup>
    </form>
  );
}
