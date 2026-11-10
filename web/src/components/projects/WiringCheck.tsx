import { IconChevronRight } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useLinearCheck } from "@/lib/linear";

/**
 * The wiring check, under the binding list: whether each binding's columns hand
 * work on to another binding. Problems are shown; what is fine folds away, so a
 * healthy project reads as one line.
 */
export function WiringCheck({ project }: { project: string }) {
  const check = useLinearCheck(project);
  if (check.isLoading) return null;
  if (check.isError) {
    return (
      <p className="text-[11px] text-muted-foreground">
        Could not check the wiring: {check.error.message}
      </p>
    );
  }
  const findings = check.data ?? [];
  const problems = findings.filter((f) => f.level !== "ok");
  const fine = findings.filter((f) => f.level === "ok");

  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted/30 p-2">
      <span className="text-[11px] font-medium text-muted-foreground">
        Wiring
      </span>
      {problems.length === 0 && (
        <span className="text-[12px]">
          Every column hands its work on to another binding.
        </span>
      )}
      {problems.map((f, i) => (
        <div key={i} className="flex items-start gap-2 text-[12px]">
          <Badge
            variant={f.level === "error" ? "destructive" : "outline"}
            className="shrink-0 text-[10px]"
          >
            {f.level}
          </Badge>
          <span className="min-w-0">
            {f.workflow && <span className="font-mono">{f.workflow}: </span>}
            {f.message}
          </span>
        </div>
      ))}
      {fine.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger className="group flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground">
            <IconChevronRight className="size-3 transition-transform group-data-[panel-open]:rotate-90" />
            {fine.length} {fine.length === 1 ? "link" : "links"} join up
          </CollapsibleTrigger>
          <CollapsibleContent className="flex flex-col gap-1 pt-1 pl-4">
            {fine.map((f, i) => (
              <span key={i} className="text-[11px] text-muted-foreground">
                {f.workflow && (
                  <span className="font-mono">{f.workflow}: </span>
                )}
                {f.message}
              </span>
            ))}
          </CollapsibleContent>
        </Collapsible>
      )}
    </div>
  );
}
