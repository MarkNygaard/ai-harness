import { useEffect, useState } from "react";
import { IconCheck, IconPlus, IconTrash } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useProjectEnv, useSetProjectEnv } from "@/lib/credentials";

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
export function ProjectEnvPanel({ project }: { project: string }) {
  const env = useProjectEnv(project);
  const save = useSetProjectEnv(project);
  const [rows, setRows] = useState<Row[]>([]);
  // Edited since the last load or save. While set, the stored vars stop
  // overwriting the rows — the list refetches every few seconds, and a refetch
  // must not wipe what is being typed.
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (env.data && !dirty) {
      const loaded = Object.entries(env.data.vars).map(([key, value]) => ({
        key,
        value,
      }));
      setRows(loaded.length ? loaded : [{ key: "", value: "" }]);
    }
  }, [env.data, dirty]);

  function update(i: number, patch: Partial<Row>) {
    setDirty(true);
    save.reset();
    setRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }

  function mergeRows(parsed: Row[], dropIdx: number) {
    setDirty(true);
    save.reset();
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
    save.mutate(vars, { onSuccess: () => setDirty(false) });
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        Build-time environment variables for this project, injected into each
        run&rsquo;s process environment (read by .NET, Node, Next.js builds, …).
        Add rows, or paste a whole <code>.env</code> into a key field to fill
        them instantly.
      </p>

      {env.isLoading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : env.isError ? (
        <p className="text-xs text-destructive">
          Failed to load variables: {env.error.message}
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          {rows.map((row, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                className="flex-1 font-mono"
                placeholder="KEY"
                value={row.key}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => update(i, { key: e.target.value })}
                onPaste={(e) => onPasteKey(i, e)}
              />
              <Input
                className="flex-2 font-mono"
                placeholder="value"
                value={row.value}
                spellCheck={false}
                onChange={(e) => update(i, { value: e.target.value })}
              />
              <Button
                variant="ghost"
                size="icon-sm"
                title="Remove"
                onClick={() => {
                  setDirty(true);
                  save.reset();
                  setRows((rs) => rs.filter((_, idx) => idx !== i));
                }}
              >
                <IconTrash className="size-3.5" />
              </Button>
            </div>
          ))}

          <div className="flex items-center gap-2">
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
            <div className="ml-auto flex items-center gap-2">
              {save.isSuccess && !dirty && (
                <span className="flex items-center gap-1 text-xs text-muted-foreground">
                  <IconCheck className="size-3.5" /> Saved
                </span>
              )}
              <Button
                size="sm"
                onClick={submit}
                disabled={save.isPending || !dirty}
              >
                {save.isPending ? "Saving…" : "Save"}
              </Button>
            </div>
          </div>

          {save.isError && (
            <span className="text-[11px] text-destructive">
              {save.error.message}
            </span>
          )}
          <p className="text-[11px] text-muted-foreground">
            Values are encrypted at rest and exposed to the agent at run time.
            Use build-scoped, least-privilege tokens.
          </p>
        </div>
      )}
    </div>
  );
}
