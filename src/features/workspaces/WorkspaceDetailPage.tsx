import { LayoutGrid, RefreshCw } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { Button, EmptyState, Skeleton } from "@/components/ui";
import { useWorkspace } from "@/hooks/useWorkspaces";
import { errorMessage, toApiError } from "@/lib/api";
import { navigate } from "@/lib/router";
import { WorkspaceEditor } from "./detail/WorkspaceEditor";
import styles from "./WorkspaceDetailPage.module.css";

/** One workspace: readiness, its apps in launch order, how it restores and its recent restores. */
export function WorkspaceDetailPage({ id }: { id: string }) {
  const query = useWorkspace(id);

  if (query.data) return <WorkspaceEditor key={id} workspace={query.data} />;

  if (query.error) {
    const notFound = toApiError(query.error).kind === "notFound";
    return (
      <Page>
        <PageHeader
          back={{ label: "Workspaces", to: { name: "workspaces" } }}
          title={notFound ? "Workspace not found" : "Couldn't open this workspace"}
          subtitle={notFound ? "It may have been deleted." : "Something went wrong while loading it."}
        />
        <EmptyState
          icon={LayoutGrid}
          tone={notFound ? "default" : "danger"}
          title={notFound ? "This workspace doesn't exist anymore" : "The workspace couldn't be loaded"}
          text={notFound ? "Pick another one from your workspaces." : errorMessage(query.error)}
          action={
            notFound ? (
              <Button variant="primary" onClick={() => navigate({ name: "workspaces" }, { replace: true })}>
                Go to workspaces
              </Button>
            ) : (
              <Button variant="secondary" icon={RefreshCw} onClick={() => void query.refresh()}>
                Try again
              </Button>
            )
          }
        />
      </Page>
    );
  }

  return (
    <Page>
      <div className={styles.skeleton} aria-busy="true" aria-label="Loading workspace">
        <Skeleton width={84} height={12} text />
        <div className={styles.skeletonHeader}>
          <Skeleton width={56} height={56} radius={15} />
          <div className={styles.skeletonTitles}>
            <Skeleton width={240} height={22} text />
            <Skeleton width={320} height={12} text />
          </div>
        </div>
        <Skeleton height={72} radius={10} />
        <div className={styles.skeletonList}>
          {Array.from({ length: 4 }, (_, index) => (
            <div key={index} className={styles.skeletonRow}>
              <Skeleton width={28} height={28} radius={7} />
              <Skeleton width="30%" height={12} text />
            </div>
          ))}
        </div>
      </div>
    </Page>
  );
}
