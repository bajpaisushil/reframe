// Root component: desktop check, providers, shell and route table.
//
// Adding a page: add the route to `Route` in src/lib/router.tsx, then render it in `RouteView`.
// Pages compose <Page> + <PageHeader> from src/components/layout/Page.tsx and UI primitives from
// src/components/ui.

import { Compass } from "lucide-react";
import { AppShell } from "@/components/layout/AppShell";
import { DesktopOnly } from "@/components/layout/DesktopOnly";
import { ErrorBoundary } from "@/components/layout/ErrorBoundary";
import { Page, PageHeader } from "@/components/layout/Page";
import { ThemeProvider } from "@/components/layout/ThemeProvider";
import { Button, EmptyState, ToastProvider } from "@/components/ui";
import { ActivityPage } from "@/features/activity/ActivityPage";
import { DashboardPage } from "@/features/dashboard/DashboardPage";
import { ExplorePage } from "@/features/desktop-explorer/ExplorePage";
import { ImportPage } from "@/features/portability/ImportPage";
import { RestorePage } from "@/features/restoration/RestorePage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { WorkspaceDetailPage } from "@/features/workspaces/WorkspaceDetailPage";
import { WorkspacesPage } from "@/features/workspaces/WorkspacesPage";
import { isTauri } from "@/lib/api";
import { navigate, useRoute, type Route } from "@/lib/router";

function renderRoute(route: Route | null) {
  if (!route) return <NotFound />;
  switch (route.name) {
    case "home":
      return <DashboardPage />;
    case "explore":
      return <ExplorePage />;
    case "workspaces":
      return <WorkspacesPage />;
    case "workspace":
      return <WorkspaceDetailPage id={route.id} />;
    case "restore":
      return <RestorePage runId={route.runId} />;
    case "activity":
      return <ActivityPage />;
    case "settings":
      return <SettingsPage />;
    case "import":
      return <ImportPage />;
  }
}

function RouteView() {
  const { route, path } = useRoute();
  return (
    <ErrorBoundary resetKeys={[path]} onGoHome={() => navigate({ name: "home" })}>
      {renderRoute(route)}
    </ErrorBoundary>
  );
}

function NotFound() {
  return (
    <Page>
      <PageHeader title="Page not found" subtitle="This link doesn't lead anywhere in Reframe." />
      <EmptyState
        icon={Compass}
        title="Nothing here"
        text="The page may have moved, or the workspace it belonged to was deleted."
        action={
          <Button variant="primary" onClick={() => navigate({ name: "home" }, { replace: true })}>
            Go to Home
          </Button>
        }
      />
    </Page>
  );
}

export function App() {
  if (!isTauri()) return <DesktopOnly />;
  return (
    <ErrorBoundary variant="app">
      <ThemeProvider>
        <ToastProvider>
          <AppShell>
            <RouteView />
          </AppShell>
        </ToastProvider>
      </ThemeProvider>
    </ErrorBoundary>
  );
}
