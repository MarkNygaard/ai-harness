import { useEffect, useState } from "react";
import { IconBraces, IconPlus, IconTrash } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { useProjectEnv, useSetProjectEnv } from "@/lib/credentials";

const inputCls =
  "h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-[13px] outline-none focus:ring-2 focus:ring-ring";

type Row = { key: string; value: string };

/**
 * Parse pasted `.env` content into KEY/VALUE pairs. Tolerates `export `,
 * surrounding quotes, `#` comments, and blank lines.
 */
function parseDotenv(text: string): Row[] {
  const out: Row[] = [];
  for (const raw of text.split(/\r?\n/)) {
    let line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("export ")) line = line.slice(7).trimStart();
    const eq = line.indexOf("=");
    if (eq <= 0) continue;
    const key = line.slice(0, eq).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
    let value = line.slice(eq + 1).trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    out.push({ key, value });
  }
  return out;
}

/**
 * Per-project build environment variables. Add rows manually, or paste a whole
 * `.env` into a key field and it expands into rows instantly (Vercel-style).
 * Values are encrypted at rest and injected into the run's process environment
 * (the universal delivery — .NET, Node, Next.js builds all read it).
 */
export function ProjectEnvDialog({ project }: { project: string }) {
  const [open, setOpen] = useState(false);
  const env = useProjectEnv(open ? project : null);
  const save = useSetProjectEnv(project);
  const [rows, setRows] = useState<Row[]>([]);
  const [dirty, setDirty] = useState(false);

  // Load stored vars into rows when the dialog opens (until the user edits).
  useEffect(() => {
    if (open && env.data && !dirty) {
      const loaded = Object.entries(env.data.vars).map(([key, value]) => ({
        key,
        value,
      }));
      setRows(loaded.length ? loaded : [{ key: "", value: "" }]);
    }
  }, [open, env.data, dirty]);

  function onOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setDirty(false);
      setRows([]);
    }
  }

  function update(i: number, patch: Partial<Row>) {
    setDirty(true);
    setRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }

  function mergeRows(parsed: Row[], dropIdx: number) {
    setDirty(true);
    setRows((rs) => {
      // Drop the (empty) row the paste landed in; keep the rest.
      const base = rs.filter(
        (r, idx) => idx !== dropIdx || r.key.trim() || r.value.trim(),
      );
      for (const p of parsed) {
        const at = base.findIndex((r) => r.key === p.key);
        if (at >= 0) base[at] = p;
        else base.push(p);
      }
      return base.length ? base : [{ key: "", value: "" }];
    });
  }

  // Pasting into a key field: a single `KEY=VALUE` fills the row; multiple
  // lines (a whole .env) expand into rows. Plain text falls through to default.
  function onPasteKey(i: number, e: React.ClipboardEvent<HTMLInputElement>) {
    const text = e.clipboardData.getData("text");
    if (!text.includes("=")) return;
    const parsed = parseDotenv(text);
    if (parsed.length === 0) return;
    e.preventDefault();
    if (parsed.length === 1 && !text.includes("\n")) {
      update(i, { key: parsed[0].key, value: parsed[0].value });
    } else {
      mergeRows(parsed, i);
    }
  }

  function submit() {
    const vars: Record<string, string> = {};
    for (const r of rows) {
      const k = r.key.trim();
      if (k) vars[k] = r.value;
    }
    save.mutate(vars, { onSuccess: () => onOpenChange(false) });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger
        render={
          <Button variant="ghost" size="sm" title="Environment variables" />
        }
      >
        <IconBraces className="size-3.5" /> Env
      </DialogTrigger>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="font-mono text-base">{project}</DialogTitle>
          <DialogDescription>
            Build-time environment variables for this project — injected into
            each run's process environment (read by .NET, Node, Next.js builds,
            …). Add rows, or paste a whole <code>.env</code> into a key field to
            fill them instantly.
          </DialogDescription>
        </DialogHeader>

        {env.isLoading ? (
          <p className="text-xs text-muted-foreground">Loading…</p>
        ) : (
          <div className="flex flex-col gap-2">
            {rows.map((row, i) => (
              <div key={i} className="flex items-center gap-2">
                <input
                  className={inputCls}
                  placeholder="KEY"
                  value={row.key}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  onChange={(e) => update(i, { key: e.target.value })}
                  onPaste={(e) => onPasteKey(i, e)}
                />
                <input
                  className={inputCls}
                  placeholder="value"
                  value={row.value}
                  spellCheck={false}
                  onChange={(e) => update(i, { value: e.target.value })}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  title="Remove"
                  onClick={() => {
                    setDirty(true);
                    setRows((rs) => rs.filter((_, idx) => idx !== i));
                  }}
                >
                  <IconTrash className="size-3.5" />
                </Button>
              </div>
            ))}

            <div className="flex items-center justify-between">
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setDirty(true);
                  setRows((rs) => [...rs, { key: "", value: "" }]);
                }}
              >
                <IconPlus className="size-4" /> Add variable
              </Button>
              <Button size="sm" onClick={submit} disabled={save.isPending}>
                {save.isPending ? "Saving…" : "Save"}
              </Button>
            </div>

            {save.isError && (
              <span className="text-[11px] text-destructive">
                {save.error.message}
              </span>
            )}
            <p className="text-[11px] text-muted-foreground">
              Values are encrypted at rest and exposed to the agent at run time
              — use build-scoped, least-privilege tokens.
            </p>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
