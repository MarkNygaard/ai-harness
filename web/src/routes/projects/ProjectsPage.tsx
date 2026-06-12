import { useState } from "react";
import {
  IconDatabase,
  IconFolderCog,
  IconKey,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
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
import type { Project } from "@/types/project";

/** Display metadata for each per-project credential provider. */
/** Keyed by credential *field* (field names are unique across providers). */
const PROJECT_CRED_META: Record<
  string,
  { label: string; placeholder: string; help: string; secret?: boolean }
> = {
  api_key: {
    label: "Linear API key",
    placeholder: "lin_api_…",
    help: "Personal API key for this project's Linear workspace. Overrides the global Linear key; used by the Linear trigger to discover teams and claim issues.",
  },
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
    <AppShell title="Projects">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
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

        <RegisterForm />

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
    </AppShell>
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
        </div>
        <ProjectLinearDialog project={project.name} />
        <ProjectCredentialsDialog project={project.name} />
        <ProjectCacheDialog project={project} />
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            if (
              confirm(`Deregister "${project.name}" and remove its checkout?`)
            ) {
              del.mutate(project.name);
            }
          }}
          disabled={del.isPending}
          title="Deregister + remove checkout"
        >
          <IconTrash className="size-3.5" /> Remove
        </Button>
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
          <Button variant="ghost" size="sm" title="Project credentials" />
        }
      >
        <IconKey className="size-3.5" /> Keys
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
          <Button variant="ghost" size="sm" title="Build cache settings" />
        }
      >
        <IconDatabase className="size-3.5" /> Cache
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="font-mono text-base">
            {project.name} — cache
          </DialogTitle>
          <DialogDescription>
            View size, set a per-project cap, or clear the build cache. Blank
            cap falls back to the server default.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="text-sm">
            <span className="text-muted-foreground">Build cache: </span>
            <span className="font-medium">
              {size.isLoading || size.data == null
                ? "—"
                : `${gb(size.data.bytes)} GB / ${size.data.cap_gb} GB`}
            </span>
          </div>
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
            <IconTrash className="size-3.5" /> Clear cache
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

function RegisterForm() {
  const register = useRegisterProject();
  const [name, setName] = useState("");
  const [gitUrl, setGitUrl] = useState("");
  const [baseBranch, setBaseBranch] = useState("");
  const [defaultWorkflow, setDefaultWorkflow] = useState("");
  const [externalUrl, setExternalUrl] = useState("");
  const [toolchains, setToolchains] = useState("");
  const [warning, setWarning] = useState<string | null>(null);

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
      },
      {
        onSuccess: (res) => {
          setWarning(res.warning ?? null);
          if (!res.warning) {
            setName("");
            setGitUrl("");
            setBaseBranch("");
            setDefaultWorkflow("");
            setExternalUrl("");
            setToolchains("");
          }
        },
      },
    );
  }

  const nameOk = /^[A-Za-z0-9_-]{1,64}$/.test(name.trim());

  return (
    <Card>
      <CardContent className="py-4">
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
                className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
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
              <code>$EXTERNAL_URL</code> (used by flows that analyze the live
              site, e.g. a GEO audit).
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
          <div className="flex items-center gap-2">
            <Button
              type="submit"
              size="sm"
              disabled={register.isPending || !nameOk || !gitUrl.trim()}
            >
              <IconPlus className="size-4" />
              {register.isPending ? "Cloning…" : "Register project"}
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
      </CardContent>
    </Card>
  );
}
