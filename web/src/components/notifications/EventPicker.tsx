import { Checkbox } from "@/components/ui/checkbox";
import { EVENTS, type NotificationEvent } from "@/lib/notifications";

/**
 * Which events to be told about: one checkbox per event, each with what it
 * means. `only` narrows the list, for a place where some events cannot happen.
 */
export function EventPicker({
  value,
  onChange,
  only,
}: {
  value: NotificationEvent[];
  onChange: (next: NotificationEvent[]) => void;
  only?: NotificationEvent[];
}) {
  const shown = only ? EVENTS.filter((e) => only.includes(e.value)) : EVENTS;
  return (
    <div className="flex flex-col gap-2.5">
      {shown.map((e) => (
        <label
          key={e.value}
          className="flex cursor-pointer items-start gap-2.5"
        >
          <Checkbox
            className="mt-0.5"
            checked={value.includes(e.value)}
            onCheckedChange={(on) =>
              onChange(
                on ? [...value, e.value] : value.filter((v) => v !== e.value),
              )
            }
          />
          <span className="flex flex-col gap-0.5">
            <span className="text-[13px] font-medium">{e.label}</span>
            <span className="text-[11px] text-muted-foreground">
              {e.description}
            </span>
          </span>
        </label>
      ))}
    </div>
  );
}
