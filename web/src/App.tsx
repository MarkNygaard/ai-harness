import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Navigate, Route, Routes } from "react-router-dom";
import { DashboardPage } from "./routes/dashboard/DashboardPage";
import { RunsPage } from "./routes/runs/RunsPage";
import { RunDetailPage } from "./routes/runs/RunDetailPage";
import { RunPairComparisonPage } from "./routes/runs/RunPairComparisonPage";
import { AbTestsPage } from "./routes/runs/AbTestsPage";
import { WorkflowReportsPage } from "./routes/runs/WorkflowReportsPage";
import { WorkflowEditor } from "./routes/editor/WorkflowEditor";
import { WorkflowsList } from "./routes/editor/WorkflowsList";
import { ProjectsPage } from "./routes/projects/ProjectsPage";
import { CredentialsPage } from "./routes/credentials/CredentialsPage";
import { CategoriesPage } from "./routes/categories/CategoriesPage";
import { PreferencesPage } from "./routes/settings/PreferencesPage";
import { EditorConnectionPage } from "./routes/settings/EditorConnectionPage";
import { MembersPage } from "./routes/settings/MembersPage";
import { TooltipProvider } from "./components/ui/tooltip";
import { TokenPrompt } from "./components/TokenPrompt";
import { LoginPage, SetupPage } from "./routes/auth/SignInPage";
import { RequireSignIn } from "./components/RequireSignIn";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: 5000,
      refetchIntervalInBackground: true,
      retry: 3,
      staleTime: 0,
    },
  },
});

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <Routes>
          {/* Outside the gate: they are how you get through it. */}
          <Route path="/login" element={<LoginPage />} />
          <Route path="/setup" element={<SetupPage />} />
          <Route path="/" element={<DashboardPage />} />
          <Route path="/runs" element={<RunsPage />} />
          <Route path="/ab" element={<AbTestsPage />} />
          <Route path="/reports/:workflow" element={<WorkflowReportsPage />} />
          {/* Static segment before `:id` so it isn't captured as a run id. */}
          <Route
            path="/runs/pair/:pairId"
            element={<RunPairComparisonPage />}
          />
          <Route path="/runs/:id" element={<RunDetailPage />} />
          {/* The workflow editor keeps the app frame and its own URL: it is a
              full-canvas view, not a settings form, and run pages link into it. */}
          <Route path="/editor/new" element={<WorkflowEditor />} />
          <Route path="/editor/:name" element={<WorkflowEditor />} />

          {/* ── Settings ─────────────────────────────────────────────────── */}
          <Route
            path="/settings"
            element={<Navigate to="/settings/preferences" replace />}
          />
          <Route path="/settings/preferences" element={<PreferencesPage />} />
          <Route path="/settings/mcp" element={<EditorConnectionPage />} />
          <Route path="/settings/members" element={<MembersPage />} />
          <Route path="/settings/credentials" element={<CredentialsPage />} />
          <Route path="/settings/projects" element={<ProjectsPage />} />
          <Route path="/settings/workflows" element={<WorkflowsList />} />
          <Route path="/settings/categories" element={<CategoriesPage />} />

          {/* These were live URLs before Settings existed, and `/editor` is
              linked from run pages — redirect rather than 404. */}
          <Route
            path="/projects"
            element={<Navigate to="/settings/projects" replace />}
          />
          <Route
            path="/editor"
            element={<Navigate to="/settings/workflows" replace />}
          />
          <Route
            path="/credentials"
            element={<Navigate to="/settings/credentials" replace />}
          />
          <Route
            path="/categories"
            element={<Navigate to="/settings/categories" replace />}
          />
        </Routes>
        <RequireSignIn />
        <TokenPrompt />
      </TooltipProvider>
    </QueryClientProvider>
  );
}
