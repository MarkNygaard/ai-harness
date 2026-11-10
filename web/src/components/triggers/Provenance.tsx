import { parseInitiator } from "@/lib/initiator";
import { formatWhen } from "./ScheduleForm";

/** What a trigger or schedule records about who made it and who last saved it. */
export interface ProvenanceFields {
  created_at: string;
  updated_at: string;
  /** "Name <email>", when the creator is a member this harness knows. */
  created_by_actor?: string | null;
  /** "Name <email>", when whoever last saved it is a member this harness knows. */
  updated_by_actor?: string | null;
}

/**
 * Who created a trigger or schedule and when, and who changed it last and
 * when. A name is left out when nobody is recorded (one saved before this was
 * kept, or on an install without accounts), and the last change is left out
 * when it is the creation itself.
 */
export function Provenance({ row }: { row: ProvenanceFields }) {
  const creator = parseInitiator(row.created_by_actor)?.label;
  const editor = parseInitiator(row.updated_by_actor)?.label;
  // Saved once, at creation: the two times are the same instant.
  const changed =
    new Date(row.updated_at).getTime() - new Date(row.created_at).getTime() >
    1000;
  return (
    <div className="flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-muted-foreground">
      <span>
        Created{creator ? ` by ${creator}` : ""} · {formatWhen(row.created_at)}
      </span>
      {changed && (
        <span>
          Last changed{editor ? ` by ${editor}` : ""} ·{" "}
          {formatWhen(row.updated_at)}
        </span>
      )}
    </div>
  );
}
