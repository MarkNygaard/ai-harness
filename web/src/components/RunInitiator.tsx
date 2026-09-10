import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { parseInitiator, sourceLabel } from "@/lib/initiator";

/**
 * Who asked for a run, as a circle of initials with the full name on hover.
 *
 * Renders **nothing** when there is nobody to name — runs from before this was
 * recorded, and anything an install with no sign-in started. A row of empty
 * circles would say less than no circle at all, and every overview shows a mix
 * of old and new runs.
 *
 * `size="sm"` is for dense rows (the dashboard's task list); the default suits
 * a run's own header.
 */
export function RunInitiator({
  actor,
  source,
  size = "default",
  className,
}: {
  actor: string | null | undefined;
  source: string | null | undefined;
  size?: "sm" | "default";
  className?: string;
}) {
  const who = parseInitiator(actor);
  if (!who) return null;
  const how = sourceLabel(source);
  // The tooltip carries what the circle cannot: the whole name, the address
  // that distinguishes two people sharing one, and how the run was started.
  const detail = [who.email !== who.label ? who.email : null, how]
    .filter(Boolean)
    .join(" · ");
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Avatar
            data-slot="run-initiator"
            className={[
              size === "sm" ? "size-5" : "size-6",
              "shrink-0",
              className ?? "",
            ]
              .filter(Boolean)
              .join(" ")}
            // Reachable without a pointer: the initials alone do not identify
            // anyone, and hover is not available to every reader.
            aria-label={`Started by ${who.label}`}
          />
        }
      >
        <AvatarFallback
          className={
            size === "sm"
              ? "bg-muted text-[9px] font-medium"
              : "bg-muted text-[10px] font-medium"
          }
        >
          {who.initials}
        </AvatarFallback>
      </TooltipTrigger>
      <TooltipContent>
        <span className="font-medium">{who.label}</span>
        {detail && <span className="text-background/70">{detail}</span>}
      </TooltipContent>
    </Tooltip>
  );
}
