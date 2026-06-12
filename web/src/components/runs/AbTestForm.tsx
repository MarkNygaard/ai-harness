import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { GitCompare } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useCreateRunPair, useWorkflowModels } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import {
  useCatalog,
  useProjectWorkflows,
  useWorkflowList,
} from "@/lib/authoring";
import type { ModelRef } from "@/types/run";

const modelLabel = (m: ModelRef) => `${m.provider} / ${m.model}`;

/**
 * A/B test trigger: run the same task twice, swapping the chosen step's model
 * for a challenger. Arm A keeps the current model (baseline); arm B uses the
 * challenger. The "step under test" list is the workflow's own model pairs.
 */
export function AbTestForm() {
  const navigate = useNavigate();
  const pair = useCreateRunPair();
  const projects = useProjects();
  const [open, setOpen] = useState(false);
  const [project, setProject] = useState("");
  const [workflow, setWorkflow] = useState("idea-to-pr");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [swapIdx, setSwapIdx] = useState(0);
  const [challengerProvider, setChallengerProvider] = useState("");
  const [challengerModel, setChallengerModel] = useState("");

  // Available workflows for the picker (project-scoped when one is selected).
  const globalWorkflows = useWorkflowList();
  const projectWorkflows = useProjectWorkflows(project || null);
  const workflows =
    (project ? projectWorkflows.data : globalWorkflows.data) ?? [];

  // The workflow's distinct provider+model pairs — the swap candidates.
  const models = useWorkflowModels(open && workflow ? workflow : null, project);
  const pairs = models.data ?? [];
  const swapFrom = pairs[swapIdx];

  // Credential-gated provider/model catalog — the challenger picks from what's
  // actually runnable (same source as the editor).
  const catalog = useCatalog();
  const providers = catalog.data?.providers ?? [];
  const challengerModels =
    providers.find((p) => p.id === challengerProvider)?.models ?? [];
  const challenger: ModelRef = {
    provider: challengerProvider,
    model: challengerModel,
  };

  function onProjectChange(name: string) {
    setProject(name);
    const def = projects.data?.find((p) => p.name === name)?.default_workflow;
    if (def) setWorkflow(def);
  }

  // Picking a provider defaults the model to its first listed model.
  function onChallengerProviderChange(id: string) {
    setChallengerProvider(id);
    setChallengerModel(providers.find((p) => p.id === id)?.models[0] ?? "");
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!swapFrom) return;
    pair.mutate(
      {
        workflow,
        project: project || undefined,
        title: title.trim() || undefined,
        description: description.trim() || undefined,
        real: true,
        swap_from: swapFrom,
        variant_a: swapFrom, // arm A = baseline (current model)
        variant_b: challenger,
      },
      { onSuccess: (res) => navigate(`/runs/pair/${res.pair_id}`) },
    );
  }

  const canSubmit =
    !pair.isPending &&
    !!project &&
    !!workflow.trim() &&
    !!swapFrom &&
    !!challengerProvider &&
    !!challengerModel;

  if (!open) {
    return (
      <Button
        variant="outline"
        onClick={() => setOpen(true)}
        className="self-start"
      >
        <GitCompare className="h-3.5 w-3.5" />
        New A/B test
      </Button>
    );
  }

  return (
    <Card>
      <CardContent className="py-4">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              New A/B test
            </h2>
            <button
              type="button"
              onClick={() => setOpen(false)}
              className="text-[11px] text-muted-foreground hover:text-foreground"
            >
              cancel
            </button>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Project
            </label>
            <select
              value={project}
              onChange={(e) => onProjectChange(e.target.value)}
              className="h-8 rounded-md border border-input bg-transparent px-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="" disabled>
                Select a project…
              </option>
              {projects.data?.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Workflow
            </label>
            <Select
              value={workflow}
              onValueChange={(v) => v != null && setWorkflow(v)}
            >
              <SelectTrigger className="h-8 w-full text-[12px]">
                <SelectValue placeholder="Select a workflow…" />
              </SelectTrigger>
              <SelectContent>
                {workflows.map((w) => (
                  <SelectItem key={w.name} value={w.name}>
                    {w.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Step under test (swap from) — arm A keeps this
            </label>
            {models.isLoading ? (
              <span className="text-[11px] text-muted-foreground">
                Loading workflow models…
              </span>
            ) : models.isError ? (
              <span className="text-[11px] text-destructive">
                {models.error.message}
              </span>
            ) : pairs.length === 0 ? (
              <span className="text-[11px] text-muted-foreground">
                No model pairs found for this workflow.
              </span>
            ) : (
              <select
                value={swapIdx}
                onChange={(e) => setSwapIdx(Number(e.target.value))}
                className="h-8 rounded-md border border-input bg-transparent px-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              >
                {pairs.map((m, i) => (
                  <option key={modelLabel(m)} value={i}>
                    {modelLabel(m)}
                  </option>
                ))}
              </select>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Test against (arm B challenger)
            </label>
            {catalog.isLoading ? (
              <span className="text-[11px] text-muted-foreground">
                Loading providers…
              </span>
            ) : providers.length === 0 ? (
              <span className="text-[11px] text-muted-foreground">
                No connected providers —{" "}
                <Link
                  to="/credentials"
                  className="text-accent-orange hover:underline"
                >
                  connect one
                </Link>{" "}
                to test against.
              </span>
            ) : (
              <div className="flex gap-2">
                <select
                  value={challengerProvider}
                  onChange={(e) => onChallengerProviderChange(e.target.value)}
                  className="h-8 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
                >
                  <option value="" disabled>
                    Provider…
                  </option>
                  {providers.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.label}
                    </option>
                  ))}
                </select>
                <select
                  value={challengerModel}
                  onChange={(e) => setChallengerModel(e.target.value)}
                  disabled={!challengerProvider}
                  className="h-8 flex-2 rounded-md border border-input bg-transparent px-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring disabled:opacity-50"
                >
                  <option value="" disabled>
                    Model…
                  </option>
                  {challengerModels.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              </div>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Title
            </label>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="short task name"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Description (the task spec — identical for both arms)
            </label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={4}
              placeholder="Describe the work fully. Both arms run this same task."
              className="rounded-md border border-input bg-transparent p-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex items-center justify-end gap-3">
            <Button type="submit" disabled={!canSubmit}>
              <GitCompare className="h-3.5 w-3.5" />
              {pair.isPending ? "Starting…" : "Start A/B pair"}
            </Button>
          </div>
          {pair.isError && (
            <p className="text-xs text-destructive">{pair.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  );
}
