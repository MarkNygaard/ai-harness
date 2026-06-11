import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

/**
 * Renders a markdown string with the app's semantic tokens — headings, lists,
 * tables (GFM), code blocks, blockquotes. Raw HTML in the source is NOT
 * rendered (no `rehype-raw`), so this is safe for agent-produced content.
 * Visual styling lives in the `.markdown` scope in `styles/globals.css`.
 */
export function Markdown({
  children,
  className,
}: {
  children: string;
  className?: string;
}) {
  return (
    <div className={cn("markdown", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}

/**
 * A two-state segmented toggle for switching a markdown surface between its
 * rendered view and its raw source (StepDialog artifacts, the editor's prompt
 * field). `value === true` means the left/rendered option is active.
 */
export function ViewToggle({
  value,
  onChange,
  renderedLabel = "Rendered",
  rawLabel = "Raw",
}: {
  value: boolean;
  onChange: (rendered: boolean) => void;
  renderedLabel?: string;
  rawLabel?: string;
}) {
  return (
    <div className="flex overflow-hidden rounded-md border border-border text-[10px]">
      <ToggleTab active={value} onClick={() => onChange(true)}>
        {renderedLabel}
      </ToggleTab>
      <ToggleTab active={!value} onClick={() => onChange(false)}>
        {rawLabel}
      </ToggleTab>
    </div>
  );
}

function ToggleTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "px-2 py-0.5 font-medium transition-colors",
        active
          ? "bg-accent-orange/10 text-foreground"
          : "text-muted-foreground hover:bg-muted",
      )}
    >
      {children}
    </button>
  );
}
