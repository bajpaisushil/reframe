// Shared contract: restore / switch to a workspace, letting the user choose Replace, Merge or
// Additive and (for Replace) confirm which windows may be closed.
//
// The plan is recomputed whenever the mode changes (it reflects what is running right now), and
// nothing is closed unless the user leaves its box checked.

import { useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { ArrowRight, RefreshCw, ShieldCheck } from "lucide-react";
import {
  AppIcon,
  Button,
  Checkbox,
  Dialog,
  Icon,
  SegmentedControl,
  Skeleton,
  WorkspaceIcon,
  useToast,
} from "@/components/ui";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useSettings } from "@/hooks/useSettings";
import { useActiveWorkspaceId, useWorkspace } from "@/hooks/useWorkspaces";
import { api, errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import type { CloseCandidate, DesktopSnapshot, IconLookup, SwitchMode, SwitchPlan } from "@/types";
import { SWITCH_MODES, modeCopy } from "./modes";
import { Notice } from "./Notice";
import { displayMessage } from "./runState";
import {
  isCloseSelected,
  isEmptyPlan,
  selectedCloseIds,
  setAllClose,
  sortCloseCandidates,
  windowLookup,
  windowLookups,
  workspaceEntryLookup,
  type CloseOverrides,
} from "./switchPlan";
import styles from "./SwitchDialog.module.css";

export interface SwitchDialogProps {
  open: boolean;
  onClose: () => void;
  workspace: { id: string; name: string; icon: string; color: string | null };
  /** Pre-selected mode (defaults to the user's default switch mode from settings). */
  defaultMode?: SwitchMode;
}

type PlanState =
  | { status: "loading" }
  | { status: "ready"; plan: SwitchPlan }
  | { status: "error"; error: unknown };

const MODE_OPTIONS = SWITCH_MODES.map((m) => ({ value: m.value, label: m.label }));

export function SwitchDialog({ open, onClose, workspace, defaultMode }: SwitchDialogProps) {
  const toast = useToast();
  const { settings } = useSettings();
  const activeId = useActiveWorkspaceId();
  const details = useWorkspace(open ? workspace.id : "");
  const { running } = useActiveRuns();
  const initialMode: SwitchMode = defaultMode ?? settings?.defaultSwitchMode ?? "merge";

  const [mode, setMode] = useState<SwitchMode>(initialMode);
  const [overrides, setOverrides] = useState<CloseOverrides>({});
  const [planState, setPlanState] = useState<PlanState>({ status: "loading" });
  const [reloadKey, setReloadKey] = useState(0);
  const [starting, setStarting] = useState(false);

  // Fresh choices every time the dialog opens (adjusted during render so the first plan request
  // already uses the right mode).
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setMode(initialMode);
      setOverrides({});
      setPlanState({ status: "loading" });
      setStarting(false);
    }
  }

  // Start on the selected mode (arrow keys then change it), not the first segment.
  const modesRef = useRef<HTMLDivElement>(null);
  const initialFocusRef = useMemo(
    () => ({
      get current(): HTMLElement | null {
        return modesRef.current?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]') ?? null;
      },
    }),
    [],
  );

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setPlanState({ status: "loading" });
    api.planSwitch(workspace.id, mode).then(
      (plan) => {
        if (!cancelled) setPlanState({ status: "ready", plan });
      },
      (error: unknown) => {
        if (!cancelled) setPlanState({ status: "error", error });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, workspace.id, mode, reloadKey]);

  const activeRun = running.find((run) => run.workspaceId === workspace.id) ?? null;
  const isSwitch = !!activeId.data && activeId.data !== workspace.id;
  const verb = isSwitch ? "Switch" : "Restore";
  const plan = planState.status === "ready" ? planState.plan : null;
  const closeIds = selectedCloseIds(plan, overrides);

  // Icons for running windows come from a desktop scan the app already made, if any.
  const snapshot = queryClient.getState<DesktopSnapshot>(queryKeys.desktopSnapshot).data;
  const lookups = useMemo(() => windowLookups(snapshot), [snapshot]);

  const start = async () => {
    if (!plan || starting) return;
    setStarting(true);
    try {
      const runId = await api.startRestore({
        workspaceId: workspace.id,
        mode: plan.mode,
        entryIds: null,
        closeWindowIds: selectedCloseIds(plan, overrides),
      });
      onClose();
      navigate({ name: "restore", runId });
    } catch (error) {
      toast.error(isSwitch ? `Couldn't switch to ${workspace.name}` : `Couldn't restore ${workspace.name}`, errorMessage(error));
    } finally {
      if (mounted.current) setStarting(false);
    }
  };

  const viewProgress = () => {
    if (!activeRun) return;
    onClose();
    navigate({ name: "restore", runId: activeRun.id });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      dismissible={!starting}
      initialFocusRef={initialFocusRef}
      title={
        <span className={styles.title}>
          <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={28} />
          <span className={styles.titleText}>
            {isSwitch ? "Switch to" : "Restore"} {workspace.name}
          </span>
        </span>
      }
      footer={
        <>
          <span className={styles.footerSummary} aria-live="polite">
            {plan && !activeRun ? planSummary(plan, closeIds.length) : ""}
          </span>
          <Button variant="secondary" onClick={onClose} disabled={starting}>
            Cancel
          </Button>
          {activeRun ? (
            <Button variant="primary" iconEnd={ArrowRight} onClick={viewProgress}>
              View progress
            </Button>
          ) : (
            <Button
              variant="primary"
              loading={starting}
              disabled={!plan || isEmptyPlan(plan)}
              onClick={() => void start()}
            >
              {verb}
            </Button>
          )}
        </>
      }
    >
      <div className={styles.body}>
        <div className={styles.modes} ref={modesRef}>
          <SegmentedControl
            aria-label="How to restore"
            value={mode}
            onChange={setMode}
            options={MODE_OPTIONS}
            fullWidth
          />
          <p className={styles.modeSummary}>{modeCopy(mode).summary}</p>
        </div>

        {activeRun && (
          <Notice tone="info" title={`${workspace.name} is being restored right now`} size="sm">
            Follow its progress, or start again once it has finished.
          </Notice>
        )}

        {planState.status === "loading" ? (
          <PlanSkeleton />
        ) : planState.status === "error" ? (
          <Notice
            tone="danger"
            size="sm"
            title="Couldn't check what's running"
            action={
              <Button size="sm" variant="secondary" icon={RefreshCw} onClick={() => setReloadKey((n) => n + 1)}>
                Try again
              </Button>
            }
          >
            {errorMessage(planState.error)}
          </Notice>
        ) : (
          <PlanSections
            plan={planState.plan}
            overrides={overrides}
            onOverrides={setOverrides}
            entryLookup={(entryId, appId) => workspaceEntryLookup(details.data, entryId, appId)}
            windowIcon={(windowId, appId, appName) => windowLookup(lookups, windowId, appId, appName)}
          />
        )}
      </div>
    </Dialog>
  );
}

/** "2 to open · 1 to arrange · 1 to close" for the footer. */
function planSummary(plan: SwitchPlan, closing: number): string {
  const parts: string[] = [];
  if (plan.launch.length > 0) parts.push(`${plan.launch.length} to open`);
  if (plan.reuse.length > 0) parts.push(`${plan.reuse.length} to arrange`);
  if (plan.mode === "replace" && closing > 0) parts.push(`${closing} to close`);
  return parts.join(" · ");
}

interface PlanSectionsProps {
  plan: SwitchPlan;
  overrides: CloseOverrides;
  onOverrides: (next: CloseOverrides) => void;
  entryLookup: (entryId: string, appId: string) => IconLookup;
  windowIcon: (windowId: string, appId: string, appName: string) => IconLookup;
}

function PlanSections({ plan, overrides, onOverrides, entryLookup, windowIcon }: PlanSectionsProps) {
  const candidates = useMemo(() => sortCloseCandidates(plan.close), [plan.close]);
  const selectedCount = candidates.filter((c) => isCloseSelected(c, overrides)).length;
  const allSelected = candidates.length > 0 && selectedCount === candidates.length;

  return (
    <>
      {isEmptyPlan(plan) && (
        <Notice tone="info" size="sm" title="Nothing to restore">
          Every app in this workspace is turned off. Turn some on from the workspace page first.
        </Notice>
      )}

      {plan.launch.length > 0 && (
        <PlanSection title="Will open" count={plan.launch.length}>
          {plan.launch.map((item) => (
            <PlanRow
              key={item.entryId}
              icon={<AppIcon lookup={entryLookup(item.entryId, item.appId)} name={item.displayName} size={20} />}
              name={item.displayName}
              // In Additive every app opens for the same reason, already explained above.
              detail={plan.mode === "additive" ? null : displayMessage(item.reason)}
            />
          ))}
        </PlanSection>
      )}

      {plan.reuse.length > 0 && (
        <PlanSection title="Already open — will be arranged" count={plan.reuse.length}>
          {plan.reuse.map((item) => (
            <PlanRow
              key={item.entryId}
              icon={<AppIcon lookup={entryLookup(item.entryId, item.appId)} name={item.displayName} size={20} />}
              name={item.displayName}
              detail={item.windowIds.length > 0 ? pluralize(item.windowIds.length, "window") : null}
            />
          ))}
        </PlanSection>
      )}

      {plan.mode === "replace" && (
        <PlanSection
          title="Will ask to close"
          count={candidates.length > 0 ? `${selectedCount} of ${candidates.length}` : undefined}
          action={
            candidates.length > 1 ? (
              <button
                type="button"
                className={styles.textButton}
                onClick={() => onOverrides(setAllClose(candidates, !allSelected))}
              >
                {allSelected ? "Clear all" : "Select all"}
              </button>
            ) : undefined
          }
          footer={
            <p className={styles.closeNote}>
              <Icon icon={ShieldCheck} size={14} className={styles.closeNoteIcon} />
              Apps are asked to close normally and can ask you to save your work. Reframe never force-quits anything.
            </p>
          }
        >
          {candidates.length === 0 ? (
            <p className={styles.emptyRow}>No other windows are open, so nothing will be closed.</p>
          ) : (
            candidates.map((candidate) => (
              <CloseRow
                key={candidate.windowId}
                candidate={candidate}
                checked={isCloseSelected(candidate, overrides)}
                onChange={(checked) => onOverrides({ ...overrides, [candidate.windowId]: checked })}
                lookup={windowIcon(candidate.windowId, candidate.appId, candidate.appName)}
              />
            ))
          )}
        </PlanSection>
      )}

      {plan.warnings.length > 0 && (
        <Notice
          tone="warning"
          size="sm"
          title={plan.warnings.length === 1 ? "One thing to know" : `${plan.warnings.length} things to know`}
          lines={plan.warnings.map((w) => displayMessage(w) ?? w)}
        />
      )}
    </>
  );
}

interface PlanSectionProps {
  title: string;
  count?: number | string;
  action?: ReactNode;
  footer?: ReactNode;
  children: ReactNode;
}

function PlanSection({ title, count, action, footer, children }: PlanSectionProps) {
  return (
    <section className={styles.section} aria-label={title}>
      <div className={styles.sectionHeader}>
        <h3 className={styles.sectionTitle}>
          {title}
          {count !== undefined && <span className={styles.sectionCount}>{count}</span>}
        </h3>
        {action && <div className={styles.sectionAction}>{action}</div>}
      </div>
      <div className={styles.rows}>{children}</div>
      {footer}
    </section>
  );
}

function PlanRow({ icon, name, detail }: { icon: ReactNode; name: string; detail: string | null }) {
  return (
    <div className={styles.row}>
      {icon}
      <span className={styles.rowName}>{name}</span>
      {detail && (
        <span className={styles.rowDetail} title={detail}>
          {detail}
        </span>
      )}
    </div>
  );
}

interface CloseRowProps {
  candidate: CloseCandidate;
  checked: boolean;
  onChange: (checked: boolean) => void;
  lookup: IconLookup;
}

function CloseRow({ candidate, checked, onChange, lookup }: CloseRowProps) {
  // The whole row toggles; clicks on the checkbox itself are handled by its own label.
  const onRowClick = (event: MouseEvent<HTMLDivElement>) => {
    if (event.target instanceof Element && event.target.closest("label")) return;
    onChange(!checked);
  };
  const title = candidate.title.trim();
  return (
    <div className={cx(styles.row, styles.closeRow, !checked && styles.unchecked)} onClick={onRowClick} title={candidate.reason}>
      <Checkbox
        checked={checked}
        onChange={onChange}
        aria-label={`Ask ${candidate.appName} to close “${title}”`}
      />
      <AppIcon lookup={lookup} name={candidate.appName} size={20} />
      <span className={styles.closeText}>
        <span className={styles.rowName}>{candidate.appName}</span>
        {title && title.toLowerCase() !== candidate.appName.toLowerCase() && <span className={styles.closeTitle}>{title}</span>}
      </span>
    </div>
  );
}

function PlanSkeleton() {
  return (
    <div className={styles.section} aria-hidden="true">
      <Skeleton width={96} height={12} text />
      <div className={styles.rows}>
        {[72, 56, 64].map((width, index) => (
          <div key={index} className={styles.row}>
            <Skeleton width={20} height={20} radius={5} />
            <Skeleton width={`${width}%`} height={12} text />
          </div>
        ))}
      </div>
    </div>
  );
}
