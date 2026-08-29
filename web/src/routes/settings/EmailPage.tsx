import { useEffect, useState } from "react";
import { IconSend } from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  useMailSettings,
  useSetMailSettings,
  useTestMail,
} from "@/lib/settings";
import type { MailInput } from "@/lib/settings";

const inputCls =
  "h-8 w-full rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring";

function Field({
  label,
  help,
  children,
}: {
  label: string;
  help?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {help && (
        <span className="text-[10px] text-muted-foreground">{help}</span>
      )}
    </label>
  );
}

export function EmailPage() {
  const settings = useMailSettings(true);
  const save = useSetMailSettings();
  const test = useTestMail();

  const [form, setForm] = useState<MailInput>({});
  const [password, setPassword] = useState("");
  const data = settings.data;

  // Seed once from the server, then leave the operator's typing alone.
  const [seeded, setSeeded] = useState(false);
  useEffect(() => {
    if (!data || seeded) return;
    setForm({
      host: data.host ?? "",
      port: data.port ?? 587,
      username: data.username ?? "",
      from: data.from ?? "",
      encryption: data.encryption,
    });
    setSeeded(true);
  }, [data, seeded]);

  const set = <K extends keyof MailInput>(key: K, value: MailInput[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  return (
    <SettingsShell title="Email">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <p className="max-w-prose text-xs text-muted-foreground">
          Used for invites and password resets. Optional — every invite also
          shows a link you can copy, so nobody is blocked on getting SMTP right.
        </p>

        <Card>
          <CardContent className="flex flex-col gap-3 px-4 py-3.5">
            <form
              className="flex flex-col gap-3"
              onSubmit={(e) => {
                e.preventDefault();
                // An untouched password field means "leave it", so it is only
                // sent when something was typed.
                save.mutate(password ? { ...form, password } : form, {
                  onSuccess: () => setPassword(""),
                });
              }}
            >
              <div className="grid gap-3 sm:grid-cols-[2fr_1fr]">
                <Field label="Host">
                  <input
                    className={inputCls}
                    value={form.host ?? ""}
                    onChange={(e) => set("host", e.target.value)}
                    placeholder="smtp.example.com"
                  />
                </Field>
                <Field label="Port">
                  <input
                    className={inputCls}
                    type="number"
                    value={form.port ?? 587}
                    onChange={(e) => set("port", Number(e.target.value))}
                  />
                </Field>
              </div>

              <Field
                label="Encryption"
                help="STARTTLS is the usual choice on port 587; implicit TLS is port 465."
              >
                <select
                  className={inputCls}
                  value={form.encryption ?? "starttls"}
                  onChange={(e) =>
                    set("encryption", e.target.value as MailInput["encryption"])
                  }
                >
                  <option value="starttls">STARTTLS</option>
                  <option value="tls">TLS</option>
                  <option value="none">None</option>
                </select>
              </Field>

              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Username" help="Leave blank for an open relay.">
                  <input
                    className={inputCls}
                    value={form.username ?? ""}
                    onChange={(e) => set("username", e.target.value)}
                    autoComplete="off"
                  />
                </Field>
                <Field
                  label="Password"
                  help={
                    data?.password_set
                      ? "A password is stored. Leave blank to keep it."
                      : "Not set."
                  }
                >
                  <input
                    className={inputCls}
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoComplete="new-password"
                  />
                </Field>
              </div>

              <Field
                label="From address"
                help="What recipients see. Some providers require it to match the account."
              >
                <input
                  className={inputCls}
                  value={form.from ?? ""}
                  onChange={(e) => set("from", e.target.value)}
                  placeholder="harness@example.com"
                />
              </Field>

              <div className="flex items-center gap-2 pt-0.5">
                <Button type="submit" size="sm" disabled={save.isPending}>
                  {save.isPending ? "Saving…" : "Save"}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={!data?.configured || test.isPending}
                  title={
                    data?.configured
                      ? "Send a message to your own address"
                      : "Set a host and a from-address first"
                  }
                  onClick={() => test.mutate()}
                >
                  <IconSend className="size-3.5" />
                  {test.isPending ? "Sending…" : "Send test"}
                </Button>
              </div>
            </form>

            {test.isSuccess && (
              <span className="text-[11px] text-status-success">
                Sent to {test.data.to}. If it does not arrive, check the
                from-address is one your provider will accept.
              </span>
            )}
            {(save.isError || test.isError || settings.isError) && (
              <span className="text-[11px] text-destructive">
                {save.error?.message ??
                  test.error?.message ??
                  settings.error?.message}
              </span>
            )}
          </CardContent>
        </Card>
      </div>
    </SettingsShell>
  );
}
