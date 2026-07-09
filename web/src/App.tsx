import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Route, Routes } from "react-router-dom";
import { DashboardPage } from "./routes/dashboard/DashboardPage";
import { RunsPage } from "./routes/runs/RunsPage";
import { RunDetailPage } from "./routes/runs/RunDetailPage";
import { RunPairComparisonPage } from "./routes/runs/RunPairComparisonPage";
import { AbTestsPage } from "./routes/runs/AbTestsPage";
import { GeoAuditsPage } from "./routes/runs/GeoAuditsPage";
import { ReviewsPage } from "./routes/runs/ReviewsPage";
import { WorkflowReportsPage } from "./routes/runs/WorkflowReportsPage";
import { WorkflowEditor } from "./routes/editor/WorkflowEditor";
import { WorkflowsList } from "./routes/editor/WorkflowsList";
import { ProjectsPage } from "./routes/projects/ProjectsPage";
import { CredentialsPage } from "./routes/credentials/CredentialsPage";
import { CategoriesPage } from "./routes/categories/CategoriesPage";
import { TooltipProvider } from "./components/ui/tooltip";
import { TokenPrompt } from "./components/TokenPrompt";

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
          <Route path="/" element={<DashboardPage />} />
          <Route path="/runs" element={<RunsPage />} />
          <Route path="/ab" element={<AbTestsPage />} />
          <Route path="/geo" element={<GeoAuditsPage />} />
          <Route path="/reviews" element={<ReviewsPage />} />
          <Route path="/reports/:workflow" element={<WorkflowReportsPage />} />
          {/* Static segment before `:id` so it isn't captured as a run id. */}
          <Route
            path="/runs/pair/:pairId"
            element={<RunPairComparisonPage />}
          />
          <Route path="/runs/:id" element={<RunDetailPage />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/editor" element={<WorkflowsList />} />
          <Route path="/editor/new" element={<WorkflowEditor />} />
          <Route path="/editor/:name" element={<WorkflowEditor />} />
          <Route path="/credentials" element={<CredentialsPage />} />
          <Route path="/categories" element={<CategoriesPage />} />
        </Routes>
        <TokenPrompt />
      </TooltipProvider>
    </QueryClientProvider>
  );
}
