import { ArrowRight } from "lucide-react";
import { Icon, ProgressBar, Spinner } from "@/components/ui";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { runProgress } from "@/lib/runs";
import { Link } from "@/lib/router";
import type { RestoreRun } from "@/types";
import styles from "./LiveRestoreBanner.module.css";

/** Slim banners for restores in progress, each linking to its progress page. */
export function LiveRestoreBanner() {
  const { running } = useActiveRuns();
  if (running.length === 0) return null;
  return (
    <div className={styles.stack}>
      {running.map((run) => (
        <RunBanner key={run.id} run={run} />
      ))}
    </div>
  );
}

function RunBanner({ run }: { run: RestoreRun }) {
  const { done, total, fraction, current } = runProgress(run);
  const detail = current ? `${current.displayName}…` : total > 0 ? "Getting ready…" : "Starting…";
  const count = total > 0 ? `${done} of ${total} apps` : null;
  return (
    <Link to={{ name: "restore", runId: run.id }} className={styles.banner} aria-label={`Restoring ${run.workspaceName}. View progress`}>
      <span className={styles.spinner}>
        <Spinner size={16} />
      </span>
      <span className={styles.text}>
        <span className={styles.title}>Restoring {run.workspaceName}</span>
        <span className={styles.detail}>
          {detail}
          {count && <span className={styles.count}>{count}</span>}
        </span>
      </span>
      <span className={styles.action}>
        View progress
        <Icon icon={ArrowRight} size={14} />
      </span>
      <ProgressBar
        className={styles.progress}
        size="sm"
        value={total > 0 ? fraction * 100 : undefined}
        label={`Restoring ${run.workspaceName}`}
        valueText={count ?? undefined}
      />
    </Link>
  );
}
