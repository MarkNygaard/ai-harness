import { describe, expect, it } from "vitest";
import { channelChoiceComplete } from "@/components/notifications/ChannelPicker";
import {
  describeChannels,
  describeEvents,
  type NotificationSubscription,
} from "./notifications";

function sub(
  over: Partial<NotificationSubscription>,
): NotificationSubscription {
  return {
    id: "s1",
    user_id: "u1",
    events: ["run_failed"],
    scope_kind: "mine",
    scope_value: "",
    channels: ["in_app"],
    slack_channel: null,
    created_at: "2026-11-26T00:00:00Z",
    ...over,
  };
}

describe("describeChannels", () => {
  it("always names the inbox, then what else the subscription reaches", () => {
    expect(describeChannels(sub({}))).toBe("Inbox");
    expect(
      describeChannels(
        sub({
          channels: ["in_app", "email", "slack_dm", "slack_channel"],
          slack_channel: "builds",
        }),
      ),
    ).toBe("Inbox, Email, Slack DM, Slack #builds");
  });
});

describe("describeEvents", () => {
  it("reads in the order the events are listed, not the order stored", () => {
    expect(describeEvents(["run_failed", "trigger_failed"])).toBe(
      "Trigger couldn't start a run, Run failed",
    );
  });
});

describe("channelChoiceComplete", () => {
  it("needs a name only when a Slack channel is chosen", () => {
    expect(channelChoiceComplete({ channels: [], slackChannel: "" })).toBe(
      true,
    );
    expect(
      channelChoiceComplete({ channels: ["slack_channel"], slackChannel: " " }),
    ).toBe(false);
    expect(
      channelChoiceComplete({
        channels: ["slack_channel"],
        slackChannel: "#builds",
      }),
    ).toBe(true);
  });
});
