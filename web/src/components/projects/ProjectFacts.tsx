import { useState } from "react";
import { IconPencil, IconPlus, IconTrash } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  type ProjectFact,
  useAddProjectFact,
  useProjectFacts,
  useRemoveProjectFact,
  useUpdateProjectFact,
} from "@/lib/project-facts";

/**
 * What the project's runs kept failing on, written down. Live facts open
 * every prompt of every run in the project; forming ones wait until enough
 * runs back them. A fact removed here is retired, so its failures aren't
 * learned again.
 */
export function ProjectFactsPanel({ project }: { project: string }) {
  const facts = useProjectFacts(project);
  const [showRetired, setShowRetired] = useState(false);
  const all = facts.data ?? [];
  const live = all.filter((f) => f.status === "active");
  const forming = all.filter((f) => f.status === "forming");
  const retired = all.filter((f) => f.status === "retired");

  return (
    <div className="flex flex-col gap-5">
      <p className="text-sm text-muted-foreground">
        When runs in this project keep failing the same way, the harness writes
        down what would have prevented it. Live facts are added to the top of
        every prompt in every run here. A learned fact goes live once its
        failure has shown up in enough runs, and is retired if the failure keeps
        happening anyway.
      </p>

      <AddFact project={project} />

      {facts.isLoading && (
        <p className="text-xs text-muted-foreground">Loading…</p>
      )}
      {facts.isError && (
        <p className="text-xs text-destructive">
          Failed to load facts: {facts.error.message}
        </p>
      )}

      {facts.data && (
        <>
          <FactList
            title="Live"
            empty="None yet. Facts appear here once runs have failed the same way a few times."
            facts={live}
            project={project}
          />
          {forming.length > 0 && (
            <FactList title="Forming" facts={forming} project={project} />
          )}
          {retired.length > 0 && (
            <div className="flex flex-col gap-2">
              <Button
                variant="ghost"
                size="sm"
                className="w-fit"
                onClick={() => setShowRetired((s) => !s)}
              >
                {showRetired ? "Hide" : "Show"} retired ({retired.length})
              </Button>
              {showRetired && (
                <FactList title="Retired" facts={retired} project={project} />
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function AddFact({ project }: { project: string }) {
  const add = useAddProjectFact(project);
  const [text, setText] = useState("");
  return (
    <div className="flex flex-col gap-2">
      <Textarea
        placeholder="Add a fact by hand, e.g. Run tests with `bun run test`, not `bun test`."
        value={text}
        rows={2}
        onChange={(e) => {
          setText(e.target.value);
          add.reset();
        }}
      />
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          disabled={!text.trim() || add.isPending}
          onClick={() => add.mutate(text, { onSuccess: () => setText("") })}
        >
          <IconPlus className="size-3.5" />
          {add.isPending ? "Adding…" : "Add fact"}
        </Button>
        {add.isError && (
          <span className="text-xs text-destructive">{add.error.message}</span>
        )}
      </div>
    </div>
  );
}

function FactList({
  title,
  facts,
  project,
  empty,
}: {
  title: string;
  facts: ProjectFact[];
  project: string;
  empty?: string;
}) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold">{title}</h2>
      {facts.length === 0 && empty && (
        <p className="text-xs text-muted-foreground">{empty}</p>
      )}
      <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
        {facts.map((f) => (
          <FactRow key={f.id} fact={f} project={project} />
        ))}
      </ul>
    </section>
  );
}

function FactRow({ fact, project }: { fact: ProjectFact; project: string }) {
  const update = useUpdateProjectFact(project);
  const remove = useRemoveProjectFact(project);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(fact.text);
  const retired = fact.status === "retired";

  return (
    <li className="flex flex-col gap-2 p-3">
      {editing ? (
        <>
          <Textarea
            value={text}
            rows={2}
            onChange={(e) => setText(e.target.value)}
          />
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              disabled={!text.trim() || update.isPending}
              onClick={() =>
                update.mutate(
                  { id: fact.id, text },
                  { onSuccess: () => setEditing(false) },
                )
              }
            >
              {update.isPending ? "Saving…" : "Save"}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setText(fact.text);
                setEditing(false);
              }}
            >
              Cancel
            </Button>
            {update.isError && (
              <span className="text-xs text-destructive">
                {update.error.message}
              </span>
            )}
          </div>
        </>
      ) : (
        <div className="flex items-start gap-2">
          <p
            className={`flex-1 text-sm ${retired ? "text-muted-foreground" : ""}`}
          >
            {fact.text}
          </p>
          {!retired && (
            <div className="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="icon-sm"
                title="Edit"
                onClick={() => setEditing(true)}
              >
                <IconPencil className="size-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                title="Remove"
                disabled={remove.isPending}
                onClick={() => {
                  if (confirm("Remove this fact? It won't be learned again."))
                    remove.mutate(fact.id);
                }}
              >
                <IconTrash className="size-3.5" />
              </Button>
            </div>
          )}
        </div>
      )}
      <FactMeta fact={fact} />
      {remove.isError && (
        <span className="text-xs text-destructive">{remove.error.message}</span>
      )}
    </li>
  );
}

function FactMeta({ fact }: { fact: ProjectFact }) {
  const learned = fact.source === "learned";
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
      <Badge variant="outline">{learned ? "learned" : "added by hand"}</Badge>
      {learned && fact.status === "forming" && (
        <span>
          seen in {fact.runs} of the {fact.needed} runs it needs
        </span>
      )}
      {learned && fact.status === "active" && (
        <span>
          {fact.runs} {fact.runs === 1 ? "run" : "runs"} hit this in the last 30
          days
        </span>
      )}
      {fact.status === "retired" && fact.retire_reason && (
        <span>Retired: {fact.retire_reason}</span>
      )}
    </div>
  );
}
