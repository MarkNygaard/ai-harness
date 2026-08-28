import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  addEdge,
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  Check,
  Loader2,
  RotateCcw,
  Save,
  FileText,
  Settings2,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  useCatalog,
  useResetWorkflow,
  useSaveWorkflow,
  useValidateWorkflow,
  useWorkflowSource,
} from "@/lib/authoring";
import { fromYaml, toYaml } from "@/lib/workflow-yaml";
import type {
  EditorNode,
  NodeKindId,
  PrebuiltStep,
  WorkflowUi,
} from "@/types/authoring";
import { EditorNode as EditorNodeView } from "@/components/editor/EditorNode";
import { EditorActionsContext } from "@/components/editor/context";
import { Palette } from "@/components/editor/Palette";
import { PropertiesDrawer } from "@/components/editor/PropertiesDrawer";
import { WorkflowDescriptionDrawer } from "@/components/editor/WorkflowDescriptionDrawer";
import { WorkflowSettingsDrawer } from "@/components/editor/WorkflowSettingsDrawer";
import {
  type EditorNodeData,
  fromGraph,
  layout,
  makeNode,
  toGraph,
} from "@/components/editor/graph";
import { emptyNode, prebuiltNode } from "@/lib/workflow-yaml";
import { useTheme } from "@/lib/theme";

const nodeTypes: NodeTypes = { editor: EditorNodeView };

interface Meta {
  name: string;
  description?: string;
  provider?: string;
  model?: string;
  ui?: WorkflowUi | null;
}

function Editor() {
  // React Flow paints its own controls and edges, so it has to be told
  // which theme is active rather than inheriting it from the page.
  const { resolved } = useTheme();
  const { name: routeName = null } = useParams();
  const catalog = useCatalog();
  const source = useWorkflowSource(routeName);
  const validate = useValidateWorkflow();
  const save = useSaveWorkflow();
  const reset = useResetWorkflow();
  const navigate = useNavigate();
  const { screenToFlowPosition } = useReactFlow();

  const [nodes, setNodes, onNodesChange] = useNodesState<Node<EditorNodeData>>(
    [],
  );
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [meta, setMeta] = useState<Meta>({
    name: routeName ?? "untitled-workflow",
  });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // The three right-hand panels (node inspector, description, UI block) are
  // mutually exclusive — only one drawer is ever open.
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [descriptionOpen, setDescriptionOpen] = useState(false);
  const loadedFor = useRef<string | null>(null);

  // Load an existing workflow once its source arrives.
  useEffect(() => {
    if (!source.data || loadedFor.current === source.data.name) return;
    const wf = fromYaml(source.data.yaml);
    const g = toGraph(wf);
    setNodes(g.nodes);
    setEdges(g.edges);
    setMeta({
      name: wf.name,
      description: wf.description,
      provider: wf.provider,
      model: wf.model,
      ui: wf.ui,
    });
    loadedFor.current = source.data.name;
  }, [source.data, setNodes, setEdges]);

  const currentWorkflow = useCallback(
    () => fromGraph(nodes, edges, meta),
    [nodes, edges, meta],
  );

  // Debounced live validation against the server.
  const validateRef = useRef(validate.mutate);
  validateRef.current = validate.mutate;
  useEffect(() => {
    if (nodes.length === 0) return;
    const t = setTimeout(() => {
      validateRef.current(toYaml(currentWorkflow()));
    }, 600);
    return () => clearTimeout(t);
  }, [nodes, edges, meta, currentWorkflow]);

  const onConnect = useCallback(
    (c: Connection) => {
      if (c.source === c.target || !c.source || !c.target) return;
      setEdges((eds) =>
        addEdge(
          {
            ...c,
            id: `${c.source}->${c.target}`,
            style: {
              stroke: "var(--muted-foreground)",
              strokeWidth: 1.5,
              opacity: 0.5,
            },
          },
          eds,
        ),
      );
    },
    [setEdges],
  );

  const uniqueId = useCallback(
    (base: string) => {
      const ids = new Set(nodes.map((n) => n.id));
      if (!ids.has(base)) return base;
      let i = 2;
      while (ids.has(`${base}-${i}`)) i += 1;
      return `${base}-${i}`;
    },
    [nodes],
  );

  const addNode = useCallback(
    (kind: NodeKindId, pos?: { x: number; y: number }) => {
      const id = uniqueId(kind);
      const node = emptyNode(kind, id);
      const position = pos ?? {
        x: 80 + nodes.length * 36,
        y: 100 + nodes.length * 36,
      };
      setNodes((nds) => [...nds, makeNode(node, position.x, position.y)]);
      setSelectedId(id);
    },
    [nodes.length, setNodes, uniqueId],
  );
  const addPrebuiltNode = useCallback(
    (step: PrebuiltStep, pos?: { x: number; y: number }) => {
      const id = uniqueId(step.node.id);
      const node = prebuiltNode(step, id);
      const position = pos ?? {
        x: 80 + nodes.length * 36,
        y: 100 + nodes.length * 36,
      };
      setNodes((nds) => [...nds, makeNode(node, position.x, position.y)]);
      setSelectedId(id);
    },
    [nodes.length, setNodes, uniqueId],
  );
  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      const pos = screenToFlowPosition({ x: e.clientX, y: e.clientY });
      const prebuiltId = e.dataTransfer.getData(
        "application/harness-prebuilt-step",
      );
      if (prebuiltId) {
        const step = catalog.data?.prebuilt_steps?.find(
          (s) => s.id === prebuiltId,
        );
        if (step) addPrebuiltNode(step, pos);
        return;
      }
      const kind = e.dataTransfer.getData(
        "application/harness-node-kind",
      ) as NodeKindId;
      if (kind) addNode(kind, pos);
    },
    [addNode, addPrebuiltNode, catalog.data, screenToFlowPosition],
  );

  const deleteNode = useCallback(
    (id: string) => {
      setNodes((nds) => nds.filter((n) => n.id !== id));
      setEdges((eds) => eds.filter((e) => e.source !== id && e.target !== id));
      setSelectedId((cur) => (cur === id ? null : cur));
    },
    [setNodes, setEdges],
  );

  // Update the selected node; renaming rewrites its edges too.
  const updateNode = useCallback(
    (next: EditorNode) => {
      const oldId = selectedId;
      if (!oldId) return;
      if (next.id !== oldId) {
        setNodes((nds) =>
          nds.map((n) =>
            n.id === oldId ? { ...n, id: next.id, data: { node: next } } : n,
          ),
        );
        setEdges((eds) =>
          eds.map((e) => ({
            ...e,
            source: e.source === oldId ? next.id : e.source,
            target: e.target === oldId ? next.id : e.target,
            id: `${e.source === oldId ? next.id : e.source}->${e.target === oldId ? next.id : e.target}`,
          })),
        );
        setSelectedId(next.id);
      } else {
        setNodes((nds) =>
          nds.map((n) => (n.id === oldId ? { ...n, data: { node: next } } : n)),
        );
      }
    },
    [selectedId, setNodes, setEdges],
  );

  const tidy = useCallback(() => {
    setNodes((nds) => layout(nds, edges));
  }, [edges, setNodes]);

  const doSave = useCallback(() => {
    save.mutate({ name: meta.name, yaml: toYaml(currentWorkflow()) });
  }, [save, meta.name, currentWorkflow]);

  // Discard the project override and reload the bundled default. Clearing
  // `loadedFor` lets the load effect re-run once the refetched source arrives.
  const doReset = useCallback(() => {
    if (!routeName) return;
    if (
      !window.confirm(
        `Reset "${routeName}" to its bundled default? This discards the project copy of this workflow.`,
      )
    )
      return;
    reset.mutate(routeName, {
      onSuccess: () => {
        loadedFor.current = null;
      },
    });
  }, [reset, routeName]);

  // Delete a custom workflow (no bundled default to fall back to). Same DELETE
  // endpoint as reset; afterward there's nothing to load, so leave the editor.
  const doDelete = useCallback(() => {
    if (!routeName) return;
    if (
      !window.confirm(
        `Delete the custom workflow "${routeName}"? This can't be undone.`,
      )
    )
      return;
    reset.mutate(routeName, { onSuccess: () => navigate("/editor") });
  }, [reset, routeName, navigate]);

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === selectedId)?.data.node ?? null,
    [nodes, selectedId],
  );

  const actions = useMemo(
    () => ({ onConfigure: setSelectedId, onDelete: deleteNode, selectedId }),
    [deleteNode, selectedId],
  );

  const editorTitle = (
    <div className="flex min-w-0 items-center gap-2">
      <Link
        to="/editor"
        className="shrink-0 font-semibold text-muted-foreground hover:text-foreground"
      >
        Workflows
      </Link>
      <span className="shrink-0 text-muted-foreground">/</span>
      <input
        className="h-7 w-56 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
        value={meta.name}
        onChange={(e) => setMeta((m) => ({ ...m, name: e.target.value }))}
        placeholder="workflow-name"
      />
      {source.data?.source === "bundled" && (
        <Badge
          variant="outline"
          title="Saving creates a project copy that shadows the bundled default"
        >
          bundled
        </Badge>
      )}
    </div>
  );

  const editorActions = (
    <>
      <ValidationStatus
        pending={validate.isPending}
        valid={validate.data?.valid}
        error={validate.data?.error ?? null}
        count={validate.data?.nodes.length}
      />
      {source.data?.has_bundled_default && source.data.source === "project" && (
        <Button
          variant="outline"
          size="sm"
          onClick={doReset}
          disabled={reset.isPending}
          title="Discard the project copy and revert to the bundled default"
        >
          <RotateCcw className="h-3.5 w-3.5" />
          {reset.isPending ? "Resetting…" : "Reset to default"}
        </Button>
      )}
      {source.data?.source === "project" &&
        !source.data.has_bundled_default && (
          <Button
            variant="outline"
            size="sm"
            onClick={doDelete}
            disabled={reset.isPending}
            title="Delete this custom workflow"
          >
            <Trash2 className="h-3.5 w-3.5" />
            {reset.isPending ? "Deleting…" : "Delete"}
          </Button>
        )}
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setSelectedId(null);
          setSettingsOpen(false);
          setDescriptionOpen((o) => !o);
        }}
        title="Read and edit this workflow's description (shown on its card)"
      >
        <FileText className="h-3.5 w-3.5" />
        Description
      </Button>
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setSelectedId(null);
          setDescriptionOpen(false);
          setSettingsOpen((o) => !o);
        }}
        title="Edit this workflow's nav entry and report tab (the UI block)"
      >
        <Settings2 className="h-3.5 w-3.5" />
        UI
      </Button>
      <Button
        variant="outline"
        size="sm"
        onClick={tidy}
        disabled={nodes.length === 0}
      >
        Tidy
      </Button>
      <Button
        size="sm"
        onClick={doSave}
        disabled={
          save.isPending || validate.data?.valid === false || nodes.length === 0
        }
      >
        <Save className="h-3.5 w-3.5" />
        {save.isPending ? "Saving…" : save.isSuccess ? "Saved" : "Save"}
      </Button>
    </>
  );

  return (
    <AppShell title={editorTitle} actions={editorActions}>
      <div className="flex h-full min-h-0 flex-col">
        {save.isError && (
          <div className="flex-none border-b border-border bg-destructive/10 px-4 py-1.5 text-xs text-destructive">
            {save.error.message}
          </div>
        )}
        <div className="flex min-h-0 flex-1">
          <Palette
            kinds={catalog.data?.node_kinds ?? []}
            prebuilt={catalog.data?.prebuilt_steps ?? []}
            onAdd={(k) => addNode(k)}
            onAddPrebuilt={(s) => addPrebuiltNode(s)}
          />
          <div
            className="min-w-0 flex-1"
            onDrop={onDrop}
            onDragOver={(e) => e.preventDefault()}
          >
            <EditorActionsContext.Provider value={actions}>
              <ReactFlow
                nodes={nodes}
                edges={edges}
                nodeTypes={nodeTypes}
                onNodesChange={onNodesChange}
                onEdgesChange={onEdgesChange}
                onConnect={onConnect}
                onNodeClick={(_e, n) => {
                  setSettingsOpen(false);
                  setSelectedId(n.id);
                }}
                onPaneClick={() => setSelectedId(null)}
                fitView
                colorMode={resolved}
                proOptions={{ hideAttribution: true }}
                className="bg-transparent"
              >
                <Background
                  variant={BackgroundVariant.Dots}
                  gap={20}
                  size={1}
                  color="var(--border)"
                />
                <Controls
                  showInteractive={false}
                  className="border-border! bg-card!"
                />
              </ReactFlow>
            </EditorActionsContext.Provider>
          </div>
          {selectedNode ? (
            <PropertiesDrawer
              node={selectedNode}
              catalog={catalog.data}
              onChange={updateNode}
              onClose={() => setSelectedId(null)}
            />
          ) : descriptionOpen ? (
            <WorkflowDescriptionDrawer
              description={meta.description}
              onChange={(next) => setMeta((m) => ({ ...m, description: next }))}
              onClose={() => setDescriptionOpen(false)}
            />
          ) : settingsOpen ? (
            <WorkflowSettingsDrawer
              ui={meta.ui}
              nodeIds={nodes.map((n) => n.id)}
              onChange={(next) => setMeta((m) => ({ ...m, ui: next }))}
              onClose={() => setSettingsOpen(false)}
            />
          ) : null}
        </div>
      </div>
    </AppShell>
  );
}

function ValidationStatus({
  pending,
  valid,
  error,
  count,
}: {
  pending: boolean;
  valid: boolean | undefined;
  error: string | null;
  count: number | undefined;
}) {
  if (pending) {
    return (
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" /> validating…
      </span>
    );
  }
  if (valid === true) {
    return (
      <span className="flex items-center gap-1 text-xs text-status-success">
        <Check className="h-3.5 w-3.5" /> valid · {count} step
        {count === 1 ? "" : "s"}
      </span>
    );
  }
  if (valid === false) {
    return (
      <span
        className="flex items-center gap-1 truncate text-xs text-destructive"
        title={error ?? ""}
      >
        <TriangleAlert className="h-3.5 w-3.5 shrink-0" /> {error ?? "invalid"}
      </span>
    );
  }
  return null;
}

/** The workflow editor screen (wrapped in its own ReactFlow provider). */
export function WorkflowEditor() {
  return (
    <ReactFlowProvider>
      <Editor />
    </ReactFlowProvider>
  );
}
