import { useState } from "react";
import { IconTrash } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  type ClaudeAccountRow,
  describePlan,
  useAddClaudeAccount,
  useClaudeAccounts,
  useRemoveClaudeAccount,
  useUpdateClaudeAccount,
} from "@/lib/claude-accounts";
import { ProviderCard, providerDef } from "./parts";

/** Plan sizes to pick from; "auto" reads it from the credential. */
const SIZES: { value: string; label: string }[] = [
  { value: "auto", label: "From the credential" },
  { value: "1", label: "Pro (1)" },
  { value: "5", label: "Max 5x (5)" },
  { value: "20", label: "Max 20x (20)" },
];

function PlanSize({ row }: { row: ClaudeAccountRow }) {
  const update = useUpdateClaudeAccount();
  const a = row.account;
  const value = a.weight_source === "override" ? String(a.weight) : "auto";
  const options = SIZES.some((s) => s.value === value)
    ? SIZES
    : [...SIZES, { value, label: `Custom (${a.weight})` }];
  return (
    <Select
      value={value}
      onValueChange={(v) =>
        v &&
        update.mutate({ id: a.id, plan_weight: v === "auto" ? 0 : Number(v) })
      }
    >
      <SelectTrigger className="w-44" aria-label={`Plan size of ${a.label}`}>
        <SelectValue>
          {(v: string | null) =>
            options.find((o) => o.value === v)?.label ?? v ?? ""
          }
        </SelectValue>
      </SelectTrigger>
      <SelectContent>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function AccountRow({ row }: { row: ClaudeAccountRow }) {
  const remove = useRemoveClaudeAccount();
  const a = row.account;
  const first = a.id === "default";
  return (
    <div className="flex flex-col gap-3 border-t border-border px-4 py-3.5 first:border-t-0">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-[13px] font-medium">
            {a.label}
            {!a.connected && <Badge variant="outline">no credential</Badge>}
            {row.running > 0 && (
              <Badge variant="secondary">
                {row.running} run{row.running === 1 ? "" : "s"} now
              </Badge>
            )}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {a.connected ? describePlan(a) : "Paste its credential below."}
            {first && " · the first account, set above"}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <PlanSize row={row} />
          {!first && (
            <Button
              variant="ghost"
              disabled={remove.isPending}
              onClick={() => {
                if (
                  window.confirm(
                    `Remove the Claude account “${a.label}”? Runs stop using it, and its credential and settings are deleted.`,
                  )
                )
                  remove.mutate(a.id);
              }}
            >
              <IconTrash /> Remove
            </Button>
          )}
        </div>
      </div>
      {!first && (
        <ProviderCard
          provider={providerDef("claude")}
          configured={a.connected}
          credentialKey={`claude:${a.id}`}
        />
      )}
    </div>
  );
}

/**
 * Further Claude subscriptions. With more than one connected, each run takes
 * the account with the most room left, weighted by its plan size, and moves to
 * another when it hits a usage limit.
 */
export function ClaudeAccounts() {
  const accounts = useClaudeAccounts();
  const add = useAddClaudeAccount();
  const [label, setLabel] = useState("");
  const rows = accounts.data ?? [];
  const connected = rows.filter((r) => r.account.connected).length;

  return (
    <div className="flex flex-col gap-3">
      <div className="text-[12px] text-muted-foreground">
        {connected > 1
          ? `${connected} Claude accounts. Each run uses the one with the most room left, counting a Max 20x as twenty Pros, and moves to another when it reaches a usage limit.`
          : "Add another Claude subscription and runs share them: each uses the account with the most room left, weighted by its plan, and moves to another at a usage limit."}
      </div>
      {accounts.isError ? (
        <p className="text-[12px] text-destructive">{accounts.error.message}</p>
      ) : (
        rows.length > 0 && (
          <div className="rounded-lg border border-border">
            {rows.map((row) => (
              <AccountRow key={row.account.id} row={row} />
            ))}
          </div>
        )
      )}
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (label.trim())
            add.mutate(label.trim(), { onSuccess: () => setLabel("") });
        }}
      >
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="Name, e.g. Team Max 20x"
          aria-label="New Claude account name"
          className="max-w-64"
        />
        <Button type="submit" disabled={!label.trim() || add.isPending}>
          Add a Claude account
        </Button>
      </form>
      {add.isError && (
        <p className="text-[12px] text-destructive">{add.error.message}</p>
      )}
    </div>
  );
}
