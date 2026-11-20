import { Compass } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState } from "@/components/ui";

/** Explore: what is open on the desktop, and what to capture. PLACEHOLDER (desktop-explorer work). */
export function ExplorePage() {
  return (
    <Page>
      <PageHeader title="Explore" subtitle="See the apps and windows that are open right now." />
      <EmptyState
        icon={Compass}
        title="Explore is coming next"
        text="This is where you'll see everything that's open on your desktop and choose what to capture."
      />
    </Page>
  );
}
