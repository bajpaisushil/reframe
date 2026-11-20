import { LayoutGrid } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState } from "@/components/ui";

/** All saved workspaces. PLACEHOLDER (workspaces work). */
export function WorkspacesPage() {
  return (
    <Page>
      <PageHeader title="Workspaces" subtitle="Everything you've saved, ready to restore." />
      <EmptyState
        icon={LayoutGrid}
        title="Your workspace library is coming next"
        text="Soon you'll browse, organize and restore all of your saved workspaces here."
      />
    </Page>
  );
}
