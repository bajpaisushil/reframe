import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ArrowUpRight,
  Ban,
  CircleCheck,
  CircleExclamationPoint,
  CirclePause,
  CircleX,
  RotateCcw,
  SearchX,
} from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import {
  AppIcon,
  Badge,
  Button,
  EmptyState,
  Icon,
  ProgressBar,
  RunStatusPill,
  SectionHeader,
  Skeleton,
  Spinner,
  StatusPill,
  WorkspaceIcon,
  useToast,
  type LucideIcon,
} from "@/components/ui";
import { useActiveRuns, useLiveRun } from "@/hooks/useActiveRuns";
import { useWorkspace } from "@/hooks/useWorkspaces";
import { api, errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { formatDuration } from "@/lib/format";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { back, navigate } from "@/lib/router";
import type { CloseOutcome, CloseStatus, DesktopSnapshot, EntryProgress, RestoreRun, Workspace } from "@/types";
import { MODE_LABEL } from "./modes";
import { Notice } from "./Notice";
import {
  CLOSE_STATUS_COPY,
  STILL_OPEN_GUIDANCE,
  currentActivity,
  displayMessage,
  entryDetail,
  entryDurationMs,
  entryIconLookup,
  formatClock,
  isDisabledEntry,
  isInterruptedRun,
  maxAttemptsFor,
  retryLabel,
  retryableEntryIds,
  runCounts,
  runElapsedMs,
  runSummary,
  type SummaryTone,
} from "./runState";
import { windowLookup, windowLookups } from "./switchPlan";
import { useRestoreRun } from "./useRestoreRun";
import { useTicker } from "./useTicker";
import styles from "./RestorePage.module.css";

/** Live progress of one restore run (route `restore/:runId`). */
export function RestorePage({ runId }: { runId: string }) {
  const toast = useToast();
  const { run: loadedRun, error, loaded } = useRestoreRun(runId);
  // A run this session already follows shows at once, before the page's own copy loads.
  const live = useLiveRun(runId);
  const run = loadedRun ?? live;
  const { running, ready, error: activeError } = useActiveRuns();
  const workspace = useWorkspace(run?.workspaceId ?? "");
  const workspaceData = workspace.data && workspace.data.id === run?.workspaceId ? workspace.data : null;

  const isRunning = run?.status === "running";
  const now = useTicker(isRunning);
  const activeIds = useMemo(() => new Set(running.map((r) => r.id)), [running]);
  const interrupted = run ? isInterruptedRun(run, activeIds, ready && !activeError, now) : false;
  const inProgress = isRunning && !interrupted;

  const [cancelling, setCancelling] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    if (!isRunning) setCancelling(false);
  }, [isRunning]);

  // The engine announces the end of a run just before it records the active workspace, the
  // last-restored time and the activity entry, so refresh those once more a moment later.
  const sawRunning = useRef(false);
  const workspaceId = run?.workspaceId;
  useEffect(() => {
    if (!workspaceId) return;
    if (isRunning) {
      sawRunning.current = true;
      return;
    }
    if (!sawRunning.current) return;
    sawRunning.current = false;
    const refresh = () =>
      queryClient.invalidate(
        queryKeys.workspaces,
        queryKeys.activeWorkspace,
        queryKeys.activityAll,
        queryKeys.restoreHistory,
        queryKeys.workspace(workspaceId),
      );
    // Not cleared on unmount: it only refreshes shared caches.
    setTimeout(refresh, 500);
    setTimeout(refresh, 2000);
  }, [isRunning, workspaceId]);

  if (!run) {
    if (loaded && error) return <RunNotFound message={errorMessage(error)} />;
    return <RestoreSkeleton />;
  }

  const cancel = async () => {
    setCancelling(true);
    try {
      await api.cancelRestore(run.id);
    } catch (e) {
      if (mounted.current) setCancelling(false);
      toast.error("Couldn't cancel the restore", errorMessage(e));
    }
  };

  /** Start a follow-up run for some entries (or all with `null`) and show it instead of this one. */
  const restartWith = async (entryIds: string[] | null) => {
    setRestarting(true);
    try {
      const nextId = await api.startRestore({ workspaceId: run.workspaceId, mode: run.mode, entryIds, closeWindowIds: [] });
      navigate({ name: "restore", runId: nextId }, { replace: true });
    } catch (e) {
      toast.error("Couldn't start the restore", errorMessage(e));
    } finally {
      if (mounted.current) setRestarting(false);
    }
  };

  const elapsed = runElapsedMs(run, now);
  const workspaceRoute = workspaceData ? ({ name: "workspace", id: workspaceData.id } as const) : null;

  return (
    <Page>
      <PageHeader
        back={workspaceRoute ? { label: "Workspace", to: workspaceRoute } : { label: "Home", to: { name: "home" } }}
        leading={<WorkspaceIcon emoji={workspaceData?.icon ?? ""} color={workspaceData?.color ?? null} size={44} />}
        title={run.workspaceName}
        subtitle={
          <span className={styles.meta}>
            {interrupted ? (
              <Badge tone="neutral" icon={CirclePause}>
                Interrupted
              </Badge>
            ) : (
              <RunStatusPill status={run.status} />
            )}
            <span className={styles.metaDot} aria-hidden="true">
              ·
            </span>
            <span>{MODE_LABEL[run.mode]}</span>
            {/* An interrupted run never finished, so it has no meaningful duration. */}
            {!interrupted && (
              <>
                <span className={styles.metaDot} aria-hidden="true">
                  ·
                </span>
                <span className="tabular" title={inProgress ? "Time since the restore started" : "How long the restore took"}>
                  {inProgress ? formatClock(elapsed) : formatDuration(elapsed)}
                </span>
              </>
            )}
          </span>
        }
        actions={
          inProgress ? (
            <Button variant="secondary" icon={Ban} loading={cancelling} onClick={() => void cancel()}>
              {cancelling ? "Cancelling…" : "Cancel restore"}
            </Button>
          ) : undefined
        }
      />

      <div className={styles.stack}>
        {inProgress ? (
          <ProgressPanel run={run} />
        ) : interrupted ? (
          <SummaryPanel
            tone="neutral"
            icon={CirclePause}
            title="This restore was interrupted"
            detail="Reframe stopped before it finished. Apps that opened are still open."
            actions={
              <>
                <Button variant="secondary" icon={RotateCcw} loading={restarting} onClick={() => void restartWith(null)}>
                  Restore again
                </Button>
                <Button variant="primary" onClick={() => back({ name: "home" })}>
                  Done
                </Button>
              </>
            }
          />
        ) : (
          <FinishedSummary
            run={run}
            elapsed={elapsed}
            workspace={workspaceData}
            restarting={restarting}
            onRetry={(ids) => void restartWith(ids)}
          />
        )}

        {run.messages.length > 0 && (
          <Notice
            tone={run.status === "failed" ? "danger" : "warning"}
            title={run.status === "failed" ? "Why this restore stopped" : "Good to know"}
            lines={run.messages.map((m) => displayMessage(m) ?? m)}
          />
        )}

        {run.closedWindows.length > 0 && <ClosingSection outcomes={run.closedWindows} />}

        <EntriesSection run={run} workspace={workspaceData} />
      </div>
    </Page>
  );
}

// ---------------------------------------------------------------------------------------------
// Progress & summary
// ---------------------------------------------------------------------------------------------

function ProgressPanel({ run }: { run: RestoreRun }) {
  const counts = runCounts(run);
  const valueText = `${counts.done} of ${counts.total} apps`;
  return (
    <section className={styles.panel} aria-label="Progress">
      <div className={styles.progressTop}>
        <span className={styles.progressActivity} aria-live="polite">
          <Spinner size={16} className={styles.progressSpinner} />
          {currentActivity(run)}
        </span>
        <span className={styles.progressCount}>{valueText}</span>
      </div>
      <ProgressBar
        value={counts.total > 0 ? counts.done : undefined}
        max={Math.max(counts.total, 1)}
        label={`Restoring ${run.workspaceName}`}
        valueText={valueText}
      />
    </section>
  );
}

const SUMMARY_ICON: Record<SummaryTone, LucideIcon> = {
  success: CircleCheck,
  warning: CircleExclamationPoint,
  danger: CircleX,
  neutral: Ban,
};

interface FinishedSummaryProps {
  run: RestoreRun;
  elapsed: number;
  workspace: Workspace | null;
  restarting: boolean;
  onRetry: (entryIds: string[]) => void;
}

function FinishedSummary({ run, elapsed, workspace, restarting, onRetry }: FinishedSummaryProps) {
  const summary = runSummary(run);
  const retryIds = retryableEntryIds(run);
  // Retrying is the natural next step after a partial or failed restore; otherwise "Done" is.
  const retryFirst = retryIds.length > 0 && (run.status === "partiallyCompleted" || run.status === "failed");
  const took = elapsed > 0 ? `Took ${formatDuration(elapsed)}.` : null;
  return (
    <SummaryPanel
      tone={summary.tone}
      icon={SUMMARY_ICON[summary.tone]}
      title={summary.title}
      detail={
        <>
          {summary.detail}
          {took && <span className={styles.summaryTook}> {took}</span>}
        </>
      }
      actions={
        <>
          {retryIds.length > 0 && (
            <Button
              variant={retryFirst ? "primary" : "secondary"}
              icon={RotateCcw}
              loading={restarting}
              onClick={() => onRetry(retryIds)}
            >
              {retryLabel(run)}
            </Button>
          )}
          {workspace && (
            <Button variant="secondary" iconEnd={ArrowUpRight} onClick={() => navigate({ name: "workspace", id: workspace.id })}>
              Open workspace
            </Button>
          )}
          <Button variant={retryFirst ? "secondary" : "primary"} onClick={() => back({ name: "home" })}>
            Done
          </Button>
        </>
      }
    />
  );
}

interface SummaryPanelProps {
  tone: SummaryTone;
  icon: LucideIcon;
  title: string;
  detail: ReactNode;
  actions: ReactNode;
}

function SummaryPanel({ tone, icon, title, detail, actions }: SummaryPanelProps) {
  return (
    <section className={cx(styles.panel, styles.summary)} aria-labelledby="restore-summary-title">
      <span className={cx(styles.summaryIcon, styles[tone])} aria-hidden="true">
        <Icon icon={icon} size={20} />
      </span>
      <div className={styles.summaryText}>
        <h2 id="restore-summary-title" className={styles.summaryTitle}>
          {title}
        </h2>
        <p className={styles.summaryDetail}>{detail}</p>
      </div>
      <div className={styles.summaryActions}>{actions}</div>
    </section>
  );
}

// ---------------------------------------------------------------------------------------------
// Closing apps (Replace)
// ---------------------------------------------------------------------------------------------

function ClosingSection({ outcomes }: { outcomes: readonly CloseOutcome[] }) {
  // Real icons when the app already scanned the desktop; initials otherwise.
  const snapshot = queryClient.getState<DesktopSnapshot>(queryKeys.desktopSnapshot).data;
  const lookups = useMemo(() => windowLookups(snapshot), [snapshot]);
  return (
    <section aria-labelledby="restore-closing">
      <SectionHeader
        id="restore-closing"
        title="Closing apps"
        count={outcomes.length}
        description="Apps were asked to close normally and could ask you to save your work. Nothing is ever force-quit."
      />
      <ul className={styles.list}>
        {outcomes.map((outcome) => (
          <li key={outcome.windowId} className={styles.row}>
            <AppIcon lookup={windowLookup(lookups, outcome.windowId, outcome.appName, outcome.appName)} name={outcome.appName} size={28} />
            <div className={styles.rowText}>
              <span className={styles.rowName}>{outcome.appName}</span>
              {outcome.title && outcome.title.toLowerCase() !== outcome.appName.toLowerCase() && (
                <span className={styles.rowDetail}>{outcome.title}</span>
              )}
              {outcome.status === "stillOpen" && <span className={styles.guidance}>{STILL_OPEN_GUIDANCE}</span>}
              {outcome.status === "failed" && outcome.message && (
                <span className={cx(styles.rowDetail, styles.rowDetailDanger)}>{displayMessage(outcome.message)}</span>
              )}
            </div>
            <CloseStatusBadge status={outcome.status} />
            <span className={styles.rowDuration} />
          </li>
        ))}
      </ul>
    </section>
  );
}

const CLOSE_ICONS: Record<Exclude<CloseStatus, "requested">, LucideIcon> = {
  closed: CircleCheck,
  stillOpen: CirclePause,
  failed: CircleX,
};

function CloseStatusBadge({ status }: { status: CloseStatus }) {
  const copy = CLOSE_STATUS_COPY[status];
  if (status === "requested") {
    return (
      <Badge tone={copy.tone} className={styles.badgeSpinner}>
        <Spinner size={12} />
        {copy.label}
      </Badge>
    );
  }
  return (
    <Badge tone={copy.tone} icon={CLOSE_ICONS[status]}>
      {copy.label}
    </Badge>
  );
}

// ---------------------------------------------------------------------------------------------
// Apps
// ---------------------------------------------------------------------------------------------

function EntriesSection({ run, workspace }: { run: RestoreRun; workspace: Workspace | null }) {
  const counts = runCounts(run);
  return (
    <section aria-labelledby="restore-apps">
      <SectionHeader
        id="restore-apps"
        title="Apps"
        count={counts.total}
        description={counts.disabled > 0 ? `${counts.disabled} turned off in this workspace ${counts.disabled === 1 ? "is" : "are"} not restored.` : undefined}
      />
      {run.entries.length === 0 ? (
        <div className={styles.list}>
          <EmptyState size="compact" icon={SearchX} title="No apps in this restore" text="There was nothing to open." />
        </div>
      ) : (
        <ul className={styles.list}>
          {run.entries.map((entry) => (
            <EntryRow key={entry.entryId} entry={entry} workspace={workspace} />
          ))}
        </ul>
      )}
    </section>
  );
}

function EntryRow({ entry, workspace }: { entry: EntryProgress; workspace: Workspace | null }) {
  const detail = entryDetail(entry, maxAttemptsFor(workspace, entry.entryId));
  const duration = entryDurationMs(entry);
  const muted = isDisabledEntry(entry);
  return (
    <li className={cx(styles.row, muted && styles.rowMuted)}>
      <AppIcon lookup={entryIconLookup(workspace, entry)} name={entry.displayName} size={28} />
      <div className={styles.rowText}>
        <span className={styles.rowName}>{entry.displayName}</span>
        {detail && (
          <span className={cx(styles.rowDetail, entry.status === "failed" && styles.rowDetailDanger)} title={detail}>
            {detail}
          </span>
        )}
      </div>
      <StatusPill status={entry.status} />
      <span className={styles.rowDuration}>{duration !== null && duration >= 100 && !muted ? formatDuration(duration) : ""}</span>
    </li>
  );
}

// ---------------------------------------------------------------------------------------------
// Loading & missing
// ---------------------------------------------------------------------------------------------

function RestoreSkeleton() {
  return (
    <Page>
      <div className={styles.skeletonHeader} aria-hidden="true">
        <Skeleton width={44} height={44} radius={11} />
        <div className={styles.skeletonTitles}>
          <Skeleton width={220} height={24} />
          <Skeleton width={160} height={14} text />
        </div>
      </div>
      <div className={styles.stack} aria-hidden="true">
        <div className={styles.panel}>
          <Skeleton width="40%" height={14} text />
          <Skeleton width="100%" height={6} style={{ marginTop: 14 }} />
        </div>
        <ul className={styles.list}>
          {[64, 48, 56].map((width, index) => (
            <li key={index} className={styles.row}>
              <Skeleton width={28} height={28} radius={7} />
              <div className={styles.rowText}>
                <Skeleton width={`${width}%`} height={12} text />
                <Skeleton width={`${width - 20}%`} height={10} text style={{ marginTop: 6 }} />
              </div>
            </li>
          ))}
        </ul>
      </div>
      <span className="sr-only" role="status">
        Loading restore…
      </span>
    </Page>
  );
}

function RunNotFound({ message }: { message: string }) {
  return (
    <Page>
      <PageHeader back={{ label: "Home", to: { name: "home" } }} title="Restore" />
      <EmptyState
        icon={SearchX}
        tone="danger"
        title="Couldn't find this restore"
        text={message}
        action={
          <>
            <Button variant="secondary" onClick={() => navigate({ name: "activity" })}>
              See past restores
            </Button>
            <Button variant="primary" onClick={() => navigate({ name: "home" }, { replace: true })}>
              Go to Home
            </Button>
          </>
        }
      />
    </Page>
  );
}
