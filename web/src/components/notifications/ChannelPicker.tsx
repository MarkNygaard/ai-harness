import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  CHANNELS,
  type NotificationChannel,
  useNotificationChannels,
} from "@/lib/notifications";

export interface ChannelChoice {
  channels: Exclude<NotificationChannel, "in_app">[];
  slackChannel: string;
}

export const NO_CHANNELS: ChannelChoice = { channels: [], slackChannel: "" };

/** Whether a choice can be saved: a Slack channel needs a name. */
export function channelChoiceComplete(c: ChannelChoice): boolean {
  return !c.channels.includes("slack_channel") || c.slackChannel.trim() !== "";
}

/**
 * Where a notification goes besides the inbox, which it always reaches.
 *
 * A channel this harness cannot deliver to yet is shown disabled with the
 * reason, rather than hidden: knowing Slack exists and needs connecting is
 * what gets it connected.
 */
export function ChannelPicker({
  value,
  onChange,
}: {
  value: ChannelChoice;
  onChange: (next: ChannelChoice) => void;
}) {
  const available = useNotificationChannels().data;
  const toggle = (channel: ChannelChoice["channels"][number], on: boolean) =>
    onChange({
      ...value,
      channels: on
        ? [...value.channels, channel]
        : value.channels.filter((c) => c !== channel),
    });

  return (
    <div className="flex flex-col gap-2.5">
      <label className="flex items-center gap-2.5 text-[13px] text-muted-foreground">
        <Checkbox checked disabled />
        Inbox — always
      </label>
      {CHANNELS.map((c) => {
        const ready = available?.[c.needs].available ?? false;
        const why =
          c.needs === "email"
            ? "Email isn’t set up on this harness (Settings → Email)."
            : "Slack isn’t connected to this harness (Settings → Integrations).";
        const checked = value.channels.includes(c.value);
        return (
          <div key={c.value} className="flex flex-col gap-1.5">
            <label
              className={
                ready
                  ? "flex cursor-pointer items-start gap-2.5"
                  : "flex items-start gap-2.5 opacity-60"
              }
            >
              <Checkbox
                className="mt-0.5"
                checked={checked}
                disabled={!ready}
                onCheckedChange={(on) => toggle(c.value, on)}
              />
              <span className="flex flex-col gap-0.5">
                <span className="text-[13px] font-medium">{c.label}</span>
                <span className="text-[11px] text-muted-foreground">
                  {!ready
                    ? why
                    : c.value === "email"
                      ? `To ${available?.email.address}.`
                      : c.value === "slack_dm"
                        ? "To the Slack account with your email address."
                        : "Posted once per event, however many people send it there. A private channel needs the app invited."}
                </span>
              </span>
            </label>
            {checked && c.value === "slack_channel" && (
              <Input
                className="ml-6.5 w-auto max-w-64"
                placeholder="#builds"
                aria-label="Slack channel"
                value={value.slackChannel}
                onChange={(e) =>
                  onChange({ ...value, slackChannel: e.target.value })
                }
              />
            )}
          </div>
        );
      })}
    </div>
  );
}
