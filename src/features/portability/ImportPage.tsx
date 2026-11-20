import { Import } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState } from "@/components/ui";

/** Import a workspace file. PLACEHOLDER (portability work). */
export function ImportPage() {
  return (
    <Page>
      <PageHeader
        back={{ label: "Workspaces", to: { name: "workspaces" } }}
        title="Import a workspace"
        subtitle="Bring in a workspace exported from another computer."
      />
      <EmptyState
        icon={Import}
        title="Importing is coming next"
        text="Soon you'll open a Reframe workspace file, review it and match its apps to this computer."
      />
    </Page>
  );
}
