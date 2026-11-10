import { useState } from "react";
import {
  IconCheck,
  IconPencil,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useRegisterProject } from "@/lib/projects";
import type { Project, ProjectRepo } from "@/types/project";

function Field({
  label,
  help,
  children,
}: {
  label: string;
  help?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
      {help && (
        <span className="text-[11px] text-muted-foreground">{help}</span>
      )}
    </label>
  );
}

/**
 * The project's own settings: repo, branch, default workflow, toolchains and
 * extra repos.
 *
 * Prefilled + name-locked = edit an existing project; blank = register a new
 * one. Both submit the same upsert (POST /api/projects). `onSaved` gets the
 * saved project's name, so a register can go straight to its page.
 */
export function ProjectForm({
  initial,
  onSaved,
}: {
  initial?: Project;
  onSaved?: (name: string) => void;
}) {
  const editing = !!initial;
  const register = useRegisterProject();
  const [name, setName] = useState(initial?.name ?? "");
  const [gitUrl, setGitUrl] = useState(initial?.git_url ?? "");
  const [baseBranch, setBaseBranch] = useState(initial?.base_branch ?? "");
  const [defaultWorkflow, setDefaultWorkflow] = useState(
    initial?.default_workflow ?? "",
  );
  const [externalUrl, setExternalUrl] = useState(initial?.external_url ?? "");
  const [toolchains, setToolchains] = useState(
    initial?.toolchains.join(", ") ?? "",
  );
  // Extra repos for a multi-repo project. Empty = single-repo (the Git URL
  // above). Each row needs a url + folder; blank branch defaults to `main`.
  const [repos, setRepos] = useState<ProjectRepo[]>(
    initial?.repos.map((r) => ({
      url: r.url,
      base_branch: r.base_branch,
      folder: r.folder,
      role: r.role ?? "",
    })) ?? [],
  );
  const [warning, setWarning] = useState<string | null>(null);

  // Any edit clears the last save's outcome, so "Saved" never sits next to
  // values that haven't been.
  function touched() {
    if (register.isSuccess || register.isError) register.reset();
  }
  function addRepo() {
    touched();
    setRepos((r) => [...r, { url: "", base_branch: "", folder: "", role: "" }]);
  }
  function updateRepo(i: number, patch: Partial<ProjectRepo>) {
    touched();
    setRepos((r) =>
      r.map((repo, j) => (j === i ? { ...repo, ...patch } : repo)),
    );
  }
  function removeRepo(i: number) {
    touched();
    setRepos((r) => r.filter((_, j) => j !== i));
  }
  const edit =
    (set: (v: string) => void) => (e: React.ChangeEvent<HTMLInputElement>) => {
      touched();
      set(e.target.value);
    };

  function submit(e: React.FormEvent) {
    e.preventDefault();
    setWarning(null);
    register.mutate(
      {
        name: name.trim(),
        git_url: gitUrl.trim(),
        // Empty → server auto-detects the repo's default branch (origin/HEAD).
        base_branch: baseBranch.trim() || undefined,
        default_workflow: defaultWorkflow.trim() || null,
        external_url: externalUrl.trim() || null,
        // Comma/space-separated mise specs → array (server drops blanks too).
        toolchains: toolchains
          .split(/[,\s]+/)
          .map((t) => t.trim())
          .filter(Boolean),
        // Keep only rows with a url + folder; server defaults a blank branch.
        repos: repos
          .map((r) => ({
            url: r.url.trim(),
            base_branch: r.base_branch.trim(),
            folder: r.folder.trim(),
            role: r.role?.trim() || undefined,
          }))
          .filter((r) => r.url && r.folder),
      },
      {
        onSuccess: (res) => {
          setWarning(res.warning ?? null);
          // A non-fatal repo warning has to be read — stay where it shows.
          if (res.warning) return;
          onSaved?.(res.project.name);
        },
      },
    );
  }

  const nameOk = /^[A-Za-z0-9_-]{1,64}$/.test(name.trim());

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name (slug)">
          <Input
            value={name}
            onChange={edit(setName)}
            placeholder="ticket0"
            disabled={editing}
            title={
              editing
                ? "The name is the project's key — remove & re-register to rename"
                : undefined
            }
            className="font-mono"
          />
        </Field>
        <Field label="Base branch (optional)">
          <Input
            value={baseBranch}
            onChange={edit(setBaseBranch)}
            placeholder="auto-detect (e.g. main, develop)"
            className="font-mono"
          />
        </Field>
      </div>
      <Field label="Git URL">
        <Input
          value={gitUrl}
          onChange={edit(setGitUrl)}
          placeholder="https://github.com/you/ticket0.git"
          className="font-mono"
        />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Default workflow (optional)">
          <Input
            value={defaultWorkflow}
            onChange={edit(setDefaultWorkflow)}
            placeholder="task"
            className="font-mono"
          />
        </Field>
        <Field
          label="Toolchains (optional)"
          help="Installed on demand with mise before each run (cached, no image rebuild)."
        >
          <Input
            value={toolchains}
            onChange={edit(setToolchains)}
            placeholder="rust, node@22, pnpm"
            className="font-mono"
          />
        </Field>
      </div>
      <Field
        label="External URL (optional)"
        help={
          <>
            The project&rsquo;s deployed site. Exposed to runs as{" "}
            <code>$EXTERNAL_URL</code> (used by flows that analyze the live
            site, e.g. a GEO audit).
          </>
        }
      >
        <Input
          value={externalUrl}
          onChange={edit(setExternalUrl)}
          placeholder="https://ticket0.ai/"
          className="font-mono"
        />
      </Field>

      {/* Multi-repo: additional repos checked out alongside the Git URL. */}
      <div className="flex flex-col gap-2 rounded-md border border-border p-3">
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs font-medium text-muted-foreground">
            Additional repos (optional — multi-repo project)
          </span>
          <Button type="button" size="sm" variant="outline" onClick={addRepo}>
            <IconPlus className="size-3.5" /> Add repo
          </Button>
        </div>
        <span className="text-[11px] text-muted-foreground">
          Leave empty for a single-repo project. The Git URL above is already
          the first repo — it is checked out into a folder named after it. Add a
          row for each <em>other</em> repo the work spans; each gets its own
          folder, and a run opens a PR in whichever repos it changed. To choose
          the primary repo&rsquo;s folder name, add a row with that same Git URL
          — it renames the folder rather than checking the repo out twice.
        </span>
        {repos.map((repo, i) => (
          <div
            key={i}
            className="flex flex-col gap-2 border-t border-border/50 pt-2"
          >
            <div className="grid gap-2 sm:grid-cols-2">
              <Input
                value={repo.folder}
                onChange={(e) => updateRepo(i, { folder: e.target.value })}
                placeholder="folder (e.g. backend)"
                className="font-mono"
              />
              <Input
                value={repo.base_branch}
                onChange={(e) => updateRepo(i, { base_branch: e.target.value })}
                placeholder="base branch (default main)"
                className="font-mono"
              />
            </div>
            <Input
              value={repo.url}
              onChange={(e) => updateRepo(i, { url: e.target.value })}
              placeholder="https://github.com/you/backend.git"
              className="font-mono"
            />
            <div className="flex items-center gap-2">
              <Input
                value={repo.role ?? ""}
                onChange={(e) => updateRepo(i, { role: e.target.value })}
                placeholder="role (optional, e.g. orders + payments API)"
                className="flex-1"
              />
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => removeRepo(i)}
              >
                <IconTrash className="size-3.5" /> Remove
              </Button>
            </div>
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="submit"
          size="sm"
          disabled={register.isPending || !nameOk || !gitUrl.trim()}
        >
          {editing ? (
            <IconPencil className="size-4" />
          ) : (
            <IconPlus className="size-4" />
          )}
          {register.isPending
            ? editing
              ? "Saving…"
              : "Cloning…"
            : editing
              ? "Save changes"
              : "Register project"}
        </Button>
        {editing && register.isSuccess && !warning && (
          <span className="flex items-center gap-1 text-xs text-muted-foreground">
            <IconCheck className="size-3.5" /> Saved — the repo re-syncs in the
            background.
          </span>
        )}
        {name.trim() && !nameOk && (
          <span className="text-xs text-destructive">
            name must be [A-Za-z0-9_-], ≤64 chars
          </span>
        )}
        {register.isError && (
          <span className="text-xs text-destructive">
            {register.error.message}
          </span>
        )}
        {warning && (
          <span className="text-xs text-status-running">{warning}</span>
        )}
      </div>
    </form>
  );
}
