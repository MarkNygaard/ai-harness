import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useWorkflowList } from "@/lib/authoring";
import { useProjects } from "@/lib/projects";

/** A labelled control, with an optional line under it. */
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {hint && (
        <span className="text-[11px] text-muted-foreground">{hint}</span>
      )}
    </div>
  );
}

/**
 * A Select over plain string options. `none` adds a first option meaning
 * "unset", since Base UI's Select needs a concrete value for it.
 */
export function OptionSelect({
  value,
  onChange,
  options,
  placeholder,
  none,
  disabled,
}: {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
  placeholder: string;
  /** Label of the "unset" option; omitted = a value is required. */
  none?: string;
  disabled?: boolean;
}) {
  const NONE = "__none__";
  return (
    <Select
      value={value || (none ? NONE : null)}
      onValueChange={(v) => v != null && onChange(v === NONE ? "" : v)}
      disabled={disabled}
    >
      <SelectTrigger className="w-full">
        <SelectValue>
          {(v: string | null) =>
            !v
              ? placeholder
              : v === NONE
                ? none
                : (options.find((o) => o.value === v)?.label ?? v)
          }
        </SelectValue>
      </SelectTrigger>
      <SelectContent>
        {none && <SelectItem value={NONE}>{none}</SelectItem>}
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

export function ProjectSelect({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (project: string) => void;
  disabled?: boolean;
}) {
  const projects = useProjects();
  return (
    <OptionSelect
      value={value}
      onChange={onChange}
      disabled={disabled}
      placeholder="Choose a project"
      options={(projects.data ?? []).map((p) => ({
        value: p.name,
        label: p.name,
      }))}
    />
  );
}

export function WorkflowSelect({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (workflow: string) => void;
  disabled?: boolean;
}) {
  const workflows = useWorkflowList();
  return (
    <OptionSelect
      value={value}
      onChange={onChange}
      disabled={disabled}
      placeholder="Choose a workflow"
      options={(workflows.data ?? []).map((w) => ({
        value: w.name,
        label: w.name,
      }))}
    />
  );
}
