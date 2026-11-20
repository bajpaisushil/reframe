import { Settings } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState } from "@/components/ui";

/** App settings. PLACEHOLDER (settings work). */
export function SettingsPage() {
  return (
    <Page>
      <PageHeader title="Settings" subtitle="Startup, appearance and what Reframe captures." />
      <EmptyState
        icon={Settings}
        title="Settings are coming next"
        text="Until then, you can switch between light and dark from the appearance button at the bottom of the sidebar."
      />
    </Page>
  );
}
