import { IconClockPlay, IconWebhook } from "@tabler/icons-react";
import { BrandMark } from "@/components/providers/ProviderMark";
import type { TriggerKind } from "@/lib/triggers";

/**
 * A trigger kind's icon. Linear gets its own mark, in its brand colour, since
 * a trigger of that kind *is* Linear's; a webhook or a schedule is no one's,
 * so they get neutral glyphs.
 */
export function TriggerKindIcon({
  kind,
  className = "size-4",
}: {
  kind: TriggerKind;
  className?: string;
}) {
  if (kind === "linear")
    return <BrandMark brand="linear" className={className} />;
  const Icon = kind === "webhook" ? IconWebhook : IconClockPlay;
  return <Icon className={`shrink-0 text-muted-foreground ${className}`} />;
}
