import { useState } from "react";
import { IconFolderCog, IconPlus, IconTrash } from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useDeleteProject, useProjects, useRegisterProject } from "@/lib/projects";
import type { Project } from "@/types/project";

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
            A project scopes runs to a git repo. Registering one clones it onto the control plane;
            runs you trigger for it operate on an isolated worktree off its base branch. Private
            repos use the global GitHub token from the Credentials page.
          </p>
        </div>

        <RegisterForm />

        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Registered projects
          </h2>
          {projects.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
          {projects.isError && (
            <p className="text-sm text-destructive">
              Failed to load projects: {projects.error.message}
            </p>
          )}
          {projects.data?.length === 0 && (
            <p className="text-sm text-muted-foreground">No projects yet. Register one above.</p>
          )}
          <div className="flex flex-col gap-2">
            {projects.data?.map((p) => <ProjectRow key={p.name} project={p} />)}
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
            <span className="truncate font-mono text-sm font-medium">{project.name}</span>
            <Badge variant="outline">{project.base_branch}</Badge>
          </div>
          <div className="truncate text-xs text-muted-foreground">{project.git_url}</div>
          {project.default_workflow && (
            <div className="truncate text-[11px] text-muted-foreground">
              default: {project.default_workflow}
            </div>
          )}
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            if (confirm(`Deregister "${project.name}" and remove its checkout?`)) {
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

function RegisterForm() {
  const register = useRegisterProject();
  const [name, setName] = useState("");
  const [gitUrl, setGitUrl] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");
  const [defaultWorkflow, setDefaultWorkflow] = useState("");
  const [warning, setWarning] = useState<string | null>(null);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    setWarning(null);
    register.mutate(
      {
        name: name.trim(),
        git_url: gitUrl.trim(),
        base_branch: baseBranch.trim() || "main",
        default_workflow: defaultWorkflow.trim() || null,
      },
      {
        onSuccess: (res) => {
          setWarning(res.warning ?? null);
          if (!res.warning) {
            setName("");
            setGitUrl("");
            setBaseBranch("main");
            setDefaultWorkflow("");
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
              <span className="text-xs font-medium text-muted-foreground">Name (slug)</span>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="ticket0"
                className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-muted-foreground">Base branch</span>
              <input
                value={baseBranch}
                onChange={(e) => setBaseBranch(e.target.value)}
                placeholder="main"
                className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
            </label>
          </div>
          <label className="flex flex-col gap-1">
            <span className="text-xs font-medium text-muted-foreground">Git URL</span>
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
              placeholder="idea-to-pr-with-kimi-coding-and-codex"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
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
              <span className="text-xs text-destructive">{register.error.message}</span>
            )}
            {warning && <span className="text-xs text-status-running">{warning}</span>}
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
