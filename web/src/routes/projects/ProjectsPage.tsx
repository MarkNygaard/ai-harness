import { useState } from "react";
import {
  IconDatabase,
  IconFolderCog,
  IconKey,
  IconPencil,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  PROJECT_CREDENTIALS,
  useDeleteProjectCredential,
  useProjectCredentials,
  useSetProjectCredential,
} from "@/lib/credentials";
import {
  useClearProjectCache,
  useDeleteProject,
  useProjectCacheSize,
  useProjects,
  useRegisterProject,
  useSetProjectCacheCap,
} from "@/lib/projects";
import { ProjectLinearDialog } from "@/components/projects/ProjectLinearDialog";
import { ProjectEnvDialog } from "@/components/projects/ProjectEnvDialog";
import type { Project, ProjectRepo } from "@/types/project";

/** Display metadata for each per-project credential provider. */
/** Keyed by credential *field* (field names are unique across providers). */
const PROJECT_CRED_META: Record<
  string,
  { label: string; placeholder: string; help: string; secret?: boolean }
> = {
  token: {
    label: "GitHub token",
    placeholder: "ghp_… / github_pat_…",
    help: "PAT with repo + pull-request access for this project's repo. Overrides the global GitHub token; used to clone the repo and open PRs.",
  },
  git_author_email: {
    label: "GitHub commit author email",
    placeholder: "you@users.noreply.github.com",
    secret: false,
    help: "Authors this project's PR commits with this email so platforms that validate the commit author against a GitHub account (e.g. Vercel) accept them. Overrides the global value; unset → a per-step synthetic address.",
  },
};

/** Browse + register projects. A project scopes runs to a git repo. */
export function ProjectsPage() {
  const projects = useProjects();

  return (
    <SettingsShell title="Projects" viewActions={<RegisterProjectDialog />}>
      <div className="mx-auto flex max-w-4xl flex-col gap-5 p-6">
        {/* `sm:pr-64` reserves room for the view actions, which are positioned
            out of flow against the view's right edge. */}
        <div className="sm:pr-64">
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <IconFolderCog className="size-5 text-accent-orange" /> Projects
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            A project scopes runs to a git repo. Registering one clones it onto
            the control plane; runs you trigger for it operate on an isolated
            worktree off its base branch. Private repos use the global GitHub
            token from the Credentials page.
          </p>
        </div>

        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Registered projects
          </h2>
          {projects.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {projects.isError && (
            <p className="text-sm text-destructive">
              Failed to load projects: {projects.error.message}
            </p>
          )}
          {projects.data?.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No projects yet. Register one above.
            </p>
          )}
          <div className="flex flex-col gap-2">
            {projects.data?.map((p) => (
              <ProjectRow key={p.name} project={p} />
            ))}
          </div>
        </section>
      </div>
    </SettingsShell>
  );
}

function ProjectRow({ project }: { project: Project }) {
  const del = useDeleteProject();
  return (
    <Card>
      <CardContent className="flex items-center gap-3 py-3">
        <IconFolderCog className="size-5 shrink-0 text-accent-orange" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate font-mono text-sm font-medium">
              {project.name}
            </span>
            <Badge variant="outline">{project.base_branch}</Badge>
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {project.git_url}
          </div>
          {project.default_workflow && (
            <div className="truncate text-[11px] text-muted-foreground">
              default: {project.default_workflow}
            </div>
          )}
          {project.external_url && (
            <a
              href={project.external_url}
              target="_blank"
              rel="noreferrer"
              className="truncate text-[11px] text-accent-orange hover:underline"
            >
              {project.external_url}
            </a>
          )}
          {project.toolchains.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              {project.toolchains.map((t) => (
                <Badge key={t} variant="secondary" className="text-[10px]">
                  {t}
                </Badge>
              ))}
            </div>
          )}
          {project.repos?.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              <Badge variant="outline" className="text-[10px]">
                multi-repo
              </Badge>
              {project.repos.map((r) => (
                <Badge
                  key={r.folder}
                  variant="secondary"
                  className="font-mono text-[10px]"
                  title={`${r.url}${r.role ? ` — ${r.role}` : ""}`}
                >
                  {r.folder}
                </Badge>
              ))}
            </div>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <ProjectEditDialog project={project} />
          <ProjectLinearDialog project={project.name} />
          <ProjectCredentialsDialog project={project.name} />
          <ProjectEnvDialog project={project.name} />
          <ProjectCacheDialog project={project} />
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => {
              if (
                confirm(`Deregister "${project.name}" and remove its checkout?`)
              ) {
                del.mutate(project.name);
              }
            }}
            disabled={del.isPending}
            title="Deregister + remove checkout"
            className="text-destructive hover:text-destructive"
          >
            <IconTrash className="size-3.5" />
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

/** A "Keys" button that opens a dialog of per-project credential overrides. */
function ProjectCredentialsDialog({ project }: { project: string }) {
  const creds = useProjectCredentials(project);
  return (
    <Dialog>
      <DialogTrigger
        render={
          <Button variant="ghost" size="icon-sm" title="Project credentials" />
        }
      >
        <IconKey className="size-3.5" />
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="font-mono text-base">{project}</DialogTitle>
          <DialogDescription>
            Project-scoped keys override the global ones from the Credentials
            page for this project. Leave blank to fall back to the global value.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          {PROJECT_CREDENTIALS.map(({ provider, field }) => (
            <CredentialField
              key={`${provider}:${field}`}
              project={project}
              provider={provider}
              field={field}
              configured={
                creds.data?.some(
                  (c) => c.provider === provider && c.configured,
                ) ?? false
              }
            />
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
/** A "Cache" button that opens a dialog to view/set the build-cache cap and clear it. */
function ProjectCacheDialog({ project }: { project: Project }) {
  const [open, setOpen] = useState(false);
  const size = useProjectCacheSize(project.name, open);
  const setCap = useSetProjectCacheCap();
  const clear = useClearProjectCache();
  const [raw, setRaw] = useState(project.cargo_target_cap_gb?.toString() ?? "");
  const capGb = raw.trim() === "" ? null : Number(raw);
  const isValid =
    raw.trim() === "" ||
    (Number.isFinite(capGb) && Number.isInteger(capGb) && (capGb ?? 0) > 0);
  const hasChanged =
    (project.cargo_target_cap_gb == null && raw.trim() !== "") ||
    (project.cargo_target_cap_gb != null &&
      Number(project.cargo_target_cap_gb) !== capGb);

  const gb = (n: number) => (n / 1_073_741_824).toFixed(1);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={
          <Button variant="ghost" size="icon-sm" title="Build cache settings" />
        }
      >
        <IconDatabase className="size-3.5" />
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="font-mono text-base">
            {project.name} — cache
          </DialogTitle>
          <DialogDescription>
            View size, set a per-project cap, or clear the build cache. Blank
            cap falls back to the server default. The dependency and git caches
            below are shared by every project — a pnpm store or NuGet folder is
            content-addressed, so no project owns an entry in it.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <dl className="flex flex-col gap-1 text-sm">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                Build cache (this project)
              </dt>
              <dd className="font-medium">
                {size.isLoading || size.data == null
                  ? "—"
                  : `${gb(size.data.bytes)} GB / ${size.data.cap_gb} GB`}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Dependencies (shared)</dt>
              <dd className="font-medium">
                {size.isLoading || size.data == null
                  ? "—"
                  : `${gb(size.data.deps_bytes)} GB / ${size.data.deps_cap_gb} GB`}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Git mirrors (shared)</dt>
              <dd className="font-medium">
                {size.isLoading || size.data == null
                  ? "—"
                  : `${gb(size.data.git_bytes)} GB`}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                Workflow cache (this project)
              </dt>
              <dd className="font-medium">
                {size.isLoading || size.data == null
                  ? "—"
                  : `${gb(size.data.workflow_bytes)} GB / ${size.data.workflow_cap_gb} GB`}
              </dd>
            </div>
          </dl>
          <div className="flex flex-col gap-1">
            <label
              className="text-xs text-muted-foreground"
              htmlFor={`cap-${project.name}`}
            >
              Cap (GiB)
            </label>
            <div className="flex items-center gap-2">
              <Input
                id={`cap-${project.name}`}
                type="number"
                min={1}
                placeholder={`default (${size.data?.cap_gb ?? 50} GB)`}
                value={raw}
                onChange={(e) => setRaw(e.target.value)}
                className="h-8 w-32"
              />
              <Button
                size="sm"
                disabled={!hasChanged || !isValid || setCap.isPending}
                onClick={() => {
                  setCap.mutate({
                    name: project.name,
                    cap_gb: capGb,
                  });
                }}
              >
                Save
              </Button>
            </div>
            {!isValid && raw.trim() !== "" && (
              <div className="text-xs text-destructive">
                Cap must be a positive whole number.
              </div>
            )}
            {setCap.error && (
              <div className="text-xs text-destructive">
                {setCap.error.message}
              </div>
            )}
          </div>
          <Button
            variant="destructive"
            size="sm"
            disabled={clear.isPending}
            onClick={() => {
              if (
                confirm(
                  `Clear build cache for "${project.name}"? The next run rebuilds from scratch (cold).`,
                )
              ) {
                clear.mutate(project.name);
              }
            }}
          >
            <IconTrash className="size-3.5" /> Clear build cache
          </Button>
          {clear.error && (
            <div className="text-xs text-destructive">
              {clear.error.message}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function CredentialField({
  project,
  provider,
  field,
  configured,
}: {
  project: string;
  provider: string;
  field: string;
  configured: boolean;
}) {
  const meta = PROJECT_CRED_META[field];
  const set = useSetProjectCredential(project);
  const del = useDeleteProjectCredential(project);
  const [value, setValue] = useState("");

  const save = () => {
    const v = value.trim();
    if (!v) return;
    set.mutate(
      { provider, fields: { [field]: v } },
      { onSuccess: () => setValue("") },
    );
  };

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-muted-foreground">
          {meta.label}
        </span>
        <Badge
          variant={configured ? "success" : "outline"}
          className="text-[10px]"
        >
          {configured ? "set" : "not set"}
        </Badge>
      </div>
      <div className="flex items-center gap-2">
        <input
          type={meta.secret === false ? "text" : "password"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={
            configured && meta.secret !== false
              ? "set — paste a new value to replace"
              : meta.placeholder
          }
          className="h-8 flex-1 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
        <Button
          size="sm"
          onClick={save}
          disabled={!value.trim() || set.isPending}
        >
          {set.isPending ? "Saving…" : "Save"}
        </Button>
        {configured && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => del.mutate(provider)}
            disabled={del.isPending}
            title="Clear this project's override"
          >
            <IconTrash className="size-3.5" /> Clear
          </Button>
        )}
      </div>
      <span className="text-[10px] text-muted-foreground">{meta.help}</span>
      {(set.isError || del.isError) && (
        <span className="text-[10px] text-destructive">
          {set.error?.message ?? del.error?.message}
        </span>
      )}
    </div>
  );
}

function ProjectForm({
  initial,
  onSaved,
}: {
  initial?: Project;
  onSaved?: () => void;
}) {
  // Prefilled + name-locked = edit an existing project; blank = register a new
  // one. Both submit the same upsert (POST /api/projects).
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

  function addRepo() {
    setRepos((r) => [...r, { url: "", base_branch: "", folder: "", role: "" }]);
  }
  function updateRepo(i: number, patch: Partial<ProjectRepo>) {
    setRepos((r) =>
      r.map((repo, j) => (j === i ? { ...repo, ...patch } : repo)),
    );
  }
  function removeRepo(i: number) {
    setRepos((r) => r.filter((_, j) => j !== i));
  }

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
          // A non-fatal repo warning stays on screen — keep the dialog open.
          if (res.warning) return;
          // Reset the fields in register mode (edit mode keeps the values).
          if (!editing) {
            setName("");
            setGitUrl("");
            setBaseBranch("");
            setDefaultWorkflow("");
            setExternalUrl("");
            setToolchains("");
            setRepos([]);
          }
          // Both register and edit are modal now — close on success.
          onSaved?.();
        },
      },
    );
  }

  const nameOk = /^[A-Za-z0-9_-]{1,64}$/.test(name.trim());

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-muted-foreground">
            Name (slug)
          </span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="ticket0"
            disabled={editing}
            title={
              editing
                ? "The name is the project's key — remove & re-register to rename"
                : undefined
            }
            className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-muted-foreground">
            Base branch (optional)
          </span>
          <input
            value={baseBranch}
            onChange={(e) => setBaseBranch(e.target.value)}
            placeholder="auto-detect (e.g. main, develop)"
            className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
          />
        </label>
      </div>
      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-muted-foreground">
          Git URL
        </span>
        <input
          value={gitUrl}
          onChange={(e) => setGitUrl(e.target.value)}
          placeholder="https://github.com/you/ticket0.git"
          className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-muted-foreground">
          Default workflow (optional)
        </span>
        <input
          value={defaultWorkflow}
          onChange={(e) => setDefaultWorkflow(e.target.value)}
          placeholder="idea-to-pr"
          className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-muted-foreground">
          External URL (optional)
        </span>
        <input
          value={externalUrl}
          onChange={(e) => setExternalUrl(e.target.value)}
          placeholder="https://ticket0.ai/"
          className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
        <span className="text-[10px] text-muted-foreground">
          The project's deployed site. Exposed to runs as{" "}
          <code>$EXTERNAL_URL</code> (used by flows that analyze the live site,
          e.g. a GEO audit).
        </span>
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-muted-foreground">
          Toolchains (optional)
        </span>
        <input
          value={toolchains}
          onChange={(e) => setToolchains(e.target.value)}
          placeholder="rust, node@22, pnpm"
          className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
        <span className="text-[10px] text-muted-foreground">
          Installed on demand with mise before each run (cached, no image
          rebuild).
        </span>
      </label>

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
        <span className="text-[10px] text-muted-foreground">
          Leave empty for a single-repo project. Add repos to span e.g. a
          frontend + backend — each is checked out into its folder, and a run
          works and opens a PR across whichever repos it changes. The Git URL
          above is the primary repo.
        </span>
        {repos.map((repo, i) => (
          <div
            key={i}
            className="flex flex-col gap-2 border-t border-border/50 pt-2"
          >
            <div className="grid gap-2 sm:grid-cols-2">
              <input
                value={repo.folder}
                onChange={(e) => updateRepo(i, { folder: e.target.value })}
                placeholder="folder (e.g. backend)"
                className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
              <input
                value={repo.base_branch}
                onChange={(e) => updateRepo(i, { base_branch: e.target.value })}
                placeholder="base branch (default main)"
                className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
            </div>
            <input
              value={repo.url}
              onChange={(e) => updateRepo(i, { url: e.target.value })}
              placeholder="https://github.com/you/backend.git"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
            <div className="flex items-center gap-2">
              <input
                value={repo.role ?? ""}
                onChange={(e) => updateRepo(i, { role: e.target.value })}
                placeholder="role (optional, e.g. orders + payments API)"
                className="h-8 flex-1 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
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

      <div className="flex items-center gap-2">
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

/** Pencil button opening a dialog with the project form prefilled — edit one
 *  field without retyping everything (the name is locked; it's the upsert key). */
function ProjectEditDialog({ project }: { project: Project }) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={<Button variant="ghost" size="icon-sm" title="Edit project" />}
      >
        <IconPencil className="size-3.5" />
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="font-mono text-base">
            {project.name}
          </DialogTitle>
          <DialogDescription>
            Update this project's settings. Saving re-registers it — metadata
            applies immediately and the repo re-syncs. The name is the project's
            key and can't be changed here.
          </DialogDescription>
        </DialogHeader>
        <ProjectForm initial={project} onSaved={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}

/** "Register project" button that opens the blank project form in a dialog, so
 *  the form doesn't occupy the page when you're just browsing projects. */
function RegisterProjectDialog() {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={
          <Button
            size="sm"
            className="shrink-0"
            title="Register a new project"
          />
        }
      >
        <IconPlus className="size-4" /> Register project
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Register project</DialogTitle>
          <DialogDescription>
            A project scopes runs to a git repo. Registering clones it onto the
            control plane; private repos use the global GitHub token from the
            Credentials page.
          </DialogDescription>
        </DialogHeader>
        <ProjectForm onSaved={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}
