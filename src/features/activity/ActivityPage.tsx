import { Activity } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState } from "@/components/ui";

/** Full activity history. PLACEHOLDER (activity work). */
export function ActivityPage() {
  return (
    <Page>
      <PageHeader title="Activity" subtitle="Everything Reframe has done, newest first." />
      <EmptyState
        icon={Activity}
        title="The full activity history is coming next"
        text="Your most recent activity is already on Home."
      />
    </Page>
  );
}
