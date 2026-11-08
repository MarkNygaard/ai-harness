import { Link, useNavigate, useParams } from "react-router-dom";
import {
  IconArrowLeft,
  IconBolt,
  IconBraces,
  IconBulb,
  IconDatabase,
  IconExternalLink,
  IconFolderCog,
  IconKey,
  IconSettings,
  IconTrash,
} from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ProjectForm } from "@/components/projects/ProjectForm";
import { ProjectLinearPanel } from "@/components/projects/ProjectLinear";
import { ProjectCredentialsPanel } from "@/components/projects/ProjectCredentials";
import { ProjectEnvPanel } from "@/components/projects/ProjectEnv";
import { ProjectCachePanel } from "@/components/projects/ProjectCache";
import { ProjectFactsPanel } from "@/components/projects/ProjectFacts";
import { useDeleteProject, useProjects } from "@/lib/projects";
import type { Project } from "@/types/project";

const TABS = [
  { value: "general", label: "General", icon: IconSettings },
  { value: "linear", label: "Linear", icon: IconBolt },
  { value: "credentials", label: "Credentials", icon: IconKey },
  { value: "environment", label: "Environment", icon: IconBraces },
  { value: "facts", label: "Learned facts", icon: IconBulb },
  { value: "cache", label: "Cache", icon: IconDatabase },
] as const;
type Tab = (typeof TABS)[number]["value"];

const isTab = (t: string | undefined): t is Tab =>
  TABS.some((x) => x.value === t);

const PROJECTS = "/settings/projects";
const projectPath = (name: string, tab?: Tab) =>
  `${PROJECTS}/${encodeURIComponent(name)}${tab && tab !== "general" ? `/${tab}` : ""}`;

/**
 * One project's settings, a tab per concern.
 *
 * The tab is part of the URL (`/settings/projects/<name>/<tab>`), so a link can
 * land on the Linear bindings or the env vars directly, and reload keeps you
 * where you were.
 */
export function ProjectPage() {
  const { name = "", tab } = useParams();
  const navigate = useNavigate();
  const projects = useProjects();
  const project = projects.data?.find((p) => p.name === name);
  const current: Tab = isTab(tab) ? tab : "general";

  return (
    <SettingsShell title={name} parents={[{ label: "Projects", to: PROJECTS }]}>
      <div className="mx-auto flex max-w-4xl flex-col gap-5 p-6">
        {projects.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {projects.isError && (
          <p className="text-sm text-destructive">
            Failed to load projects: {projects.error.message}
          </p>
        )}
        {projects.data && !project && (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm text-muted-foreground">
              There is no project named{" "}
              <span className="font-mono text-foreground">{name}</span>. It may
              have been deregistered — look under Recently deleted.
            </p>
            <Button variant="outline" size="sm" render={<Link to={PROJECTS} />}>
              <IconArrowLeft className="size-4" /> All projects
            </Button>
          </div>
        )}

        {project && (
          <>
            <ProjectHeader project={project} />
            <Tabs
              value={current}
              onValueChange={(v) =>
                navigate(projectPath(project.name, v as Tab), { replace: true })
              }
              className="gap-5"
            >
              {/* The line list sits on a full-width rule; scrolls sideways
                  instead of wrapping on a narrow screen. */}
              <div className="-mx-1 overflow-x-auto border-b border-border px-1">
                <TabsList variant="line" className="pb-1">
                  {TABS.map(({ value, label, icon: Icon }) => (
                    <TabsTrigger key={value} value={value} className="px-2.5">
                      <Icon className="size-4" /> {label}
                    </TabsTrigger>
                  ))}
                </TabsList>
              </div>

              <TabsContent value="general" className="flex flex-col gap-8">
                <ProjectForm key={project.name} initial={project} />
                <DangerZone project={project.name} />
              </TabsContent>
              <TabsContent value="linear">
                <ProjectLinearPanel project={project.name} />
              </TabsContent>
              <TabsContent value="credentials">
                <ProjectCredentialsPanel project={project.name} />
              </TabsContent>
              <TabsContent value="environment">
                <ProjectEnvPanel project={project.name} />
              </TabsContent>
              <TabsContent value="facts">
                <ProjectFactsPanel project={project.name} />
              </TabsContent>
              <TabsContent value="cache">
                <ProjectCachePanel project={project} />
              </TabsContent>
            </Tabs>
          </>
        )}
      </div>
    </SettingsShell>
  );
}

function ProjectHeader({ project }: { project: Project }) {
  return (
    <div className="flex items-start gap-3">
      <IconFolderCog className="mt-0.5 size-6 shrink-0 text-accent-orange" />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="truncate font-mono text-lg font-semibold">
            {project.name}
          </h1>
          <Badge variant="outline">{project.base_branch}</Badge>
          {project.repos?.length > 0 && (
            <Badge variant="outline">multi-repo</Badge>
          )}
        </div>
        <div className="truncate text-sm text-muted-foreground">
          {project.git_url}
        </div>
        {project.external_url && (
          <a
            href={project.external_url}
            target="_blank"
            rel="noreferrer"
            className="flex w-fit items-center gap-1 truncate text-xs text-accent-orange hover:underline"
          >
            {project.external_url}
            <IconExternalLink className="size-3 shrink-0" />
          </a>
        )}
      </div>
    </div>
  );
}

/** Deregister, set apart at the foot of General so it isn't hit by accident. */
function DangerZone({ project }: { project: string }) {
  const del = useDeleteProject();
  const navigate = useNavigate();
  return (
    <section className="flex flex-col gap-2 rounded-md border border-destructive/40 p-4">
      <h2 className="text-sm font-semibold">Deregister project</h2>
      <p className="text-xs text-muted-foreground">
        Removes the project and its checkout. It stays under Recently deleted
        for 14 days, where restoring it clones the repo again.
      </p>
      <div className="flex items-center gap-2">
        <Button
          variant="destructive"
          size="sm"
          disabled={del.isPending}
          onClick={() => {
            if (confirm(`Deregister "${project}"?`)) {
              del.mutate(project, {
                onSuccess: () => navigate(PROJECTS, { replace: true }),
              });
            }
          }}
        >
          <IconTrash className="size-3.5" />
          {del.isPending ? "Deregistering…" : "Deregister"}
        </Button>
        {del.isError && (
          <span className="text-xs text-destructive">{del.error.message}</span>
        )}
      </div>
    </section>
  );
}
