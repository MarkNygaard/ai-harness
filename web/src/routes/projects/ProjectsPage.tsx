import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { IconChevronRight, IconFolderCog, IconPlus } from "@tabler/icons-react";
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
import { useProjects } from "@/lib/projects";
import { ProjectForm } from "@/components/projects/ProjectForm";
import { RecentlyDeletedSettings } from "@/components/projects/RecentlyDeletedSettings";
import type { Project } from "@/types/project";

/**
 * Browse + register projects. A project scopes runs to a git repo; each row
 * opens that project's own page, where its settings live in tabs.
 */
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

        <RecentlyDeletedSettings />
      </div>
    </SettingsShell>
  );
}

function ProjectRow({ project }: { project: Project }) {
  return (
    <Link
      to={`/settings/projects/${encodeURIComponent(project.name)}`}
      className="group rounded-xl outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
    >
      <Card className="transition-colors group-hover:bg-muted/40">
        <CardContent className="flex items-center gap-3 py-3">
          <IconFolderCog className="size-5 shrink-0 text-accent-orange" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate font-mono text-sm font-medium">
                {project.name}
              </span>
              <Badge variant="outline">{project.base_branch}</Badge>
              {project.repos?.length > 0 && (
                <Badge variant="outline" className="text-[10px]">
                  multi-repo
                </Badge>
              )}
            </div>
            <div className="truncate text-xs text-muted-foreground">
              {project.git_url}
            </div>
            {project.default_workflow && (
              <div className="truncate text-[11px] text-muted-foreground">
                default: {project.default_workflow}
              </div>
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
          <IconChevronRight className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}

/** "Register project" button that opens the blank project form in a dialog,
 *  then lands on the new project's page to finish setting it up. */
function RegisterProjectDialog() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
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
            Credentials page. Linear, credentials and environment variables are
            set up on the project&rsquo;s page afterwards.
          </DialogDescription>
        </DialogHeader>
        <ProjectForm
          onSaved={(name) => {
            setOpen(false);
            navigate(`/settings/projects/${encodeURIComponent(name)}`);
          }}
        />
      </DialogContent>
    </Dialog>
  );
}
