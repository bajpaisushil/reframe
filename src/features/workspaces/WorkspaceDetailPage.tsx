import { LayoutGrid } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState, WorkspaceIcon } from "@/components/ui";
import { useWorkspace } from "@/hooks/useWorkspaces";

/** One workspace: its apps, layout and restore options. PLACEHOLDER (workspaces work). */
export function WorkspaceDetailPage({ id }: { id: string }) {
  const { data: workspace } = useWorkspace(id);
  return (
    <Page>
      <PageHeader
        back={{ label: "Workspaces", to: { name: "workspaces" } }}
        leading={workspace && <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={44} />}
        title={workspace?.name ?? "Workspace"}
        subtitle={workspace?.description || "Apps, windows and how this workspace restores."}
      />
      <EmptyState
        icon={LayoutGrid}
        title="Workspace details are coming next"
        text="Soon you'll see and edit this workspace's apps, window layout and restore options here."
      />
    </Page>
  );
}
