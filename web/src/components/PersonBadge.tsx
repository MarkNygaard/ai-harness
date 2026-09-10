import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { parseInitiator, sourceLabel } from "@/lib/initiator";

/**
 * A person, as a circle of initials with their full name on hover.
 *
 * Used for whoever started a run and whoever wrote a workflow — the same
 * question in two places, so the same badge.
 *
 * Renders **nothing** when there is nobody to name: runs from before
 * attribution existed, workflows that shipped bundled or were dropped into the
 * directory by hand, and anything an install with no sign-in did. A row of
 * empty circles would say less than no circle at all, and every list shows a
 * mix.
 *
 * `size="sm"` is for dense rows; the default suits a header.
 */
export function PersonBadge({
  actor,
  source,
  action = "Started by",
  size = "default",
  className,
}: {
  actor: string | null | undefined;
  source: string | null | undefined;
  /** What this person did, for the tooltip and for screen readers. */
  action?: string;
  size?: "sm" | "default";
  className?: string;
}) {
  const who = parseInitiator(actor);
  if (!who) return null;
  const how = sourceLabel(source);
  // The tooltip carries what the circle cannot: the whole name, the address
  // that distinguishes two people sharing one, and how it was done.
  const detail = [who.email !== who.label ? who.email : null, how]
    .filter(Boolean)
    .join(" · ");
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Avatar
            data-slot="person-badge"
            className={[
              size === "sm" ? "size-5" : "size-6",
              "shrink-0",
              className ?? "",
            ]
              .filter(Boolean)
              .join(" ")}
            // Reachable without a pointer: the initials alone identify nobody,
            // and hover is not available to every reader.
            aria-label={`${action} ${who.label}`}
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
        <span className="font-medium">
          {action} {who.label}
        </span>
        {detail && <span className="text-background/70">{detail}</span>}
      </TooltipContent>
    </Tooltip>
  );
}
