import { CircleX, RotateCcw } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { EmptyState, RunStatusPill } from "@/components/ui";
import { useLiveRun } from "@/hooks/useActiveRuns";
import { useQuery } from "@/hooks/useQuery";
import { api, errorMessage } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

/** Live progress of one restore run. PLACEHOLDER (restoration work). */
export function RestorePage({ runId }: { runId: string }) {
  const live = useLiveRun(runId);
  // Runs this session has not seen live (opened later, or from the tray) come from the backend.
  const stored = useQuery(queryKeys.restoreRun(runId), () => api.restoreRun(runId), { enabled: live === null });
  const run = live ?? stored.data ?? null;
  const failed = run?.status === "failed" || run?.status === "partiallyCompleted";
  return (
    <Page>
      <PageHeader
        back={{ label: "Home", to: { name: "home" } }}
        title={run ? run.workspaceName : "Restore"}
        subtitle={run ? <RunStatusPill status={run.status} /> : "Bringing your workspace back."}
      />
      {!run && stored.error ? (
        <EmptyState icon={CircleX} tone="danger" title="Couldn't load this restore" text={errorMessage(stored.error)} />
      ) : failed && run.messages.length > 0 ? (
        <EmptyState icon={CircleX} tone="danger" title="This restore didn't finish" text={run.messages.join(" ")} />
      ) : (
        <EmptyState
          icon={RotateCcw}
          title="Live restore progress is coming next"
          text="Soon you'll follow each app here as Reframe reopens and arranges it."
        />
      )}
    </Page>
  );
}
