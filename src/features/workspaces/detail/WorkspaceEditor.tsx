import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import { Activity, Copy, Ellipsis, FileUp, MonitorUp, Pencil, RotateCcw, Trash } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { Badge, Button, ConfirmDialog, IconButton, Menu, RelativeTime, Spinner, WorkspaceIcon, useToast } from "@/components/ui";
import { AppPickerDialog } from "@/features/desktop-explorer/AppPickerDialog";
import { SwitchDialog } from "@/features/restoration/SwitchDialog";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useActiveWorkspaceId } from "@/hooks/useWorkspaces";
import { api, errorMessage, toApiError } from "@/lib/api";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import { hasPrimaryModifier } from "@/lib/platform";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import type { ApiError, ApplicationEntry, CaptureSelection, LaunchConfig, LaunchTarget, Workspace } from "@/types";
import { parentFolder, pickFile } from "../components/pickers";
import { healthKey, useWorkspaceHealth } from "../hooks/queries";
import { useLeaveGuard } from "../hooks/useLeaveGuard";
import { useWorkspaceActions, type WorkspaceRef } from "../hooks/useWorkspaceActions";
import { draftReducer, initDraft, isDirty, prepareForSave, sameEntriesIgnoringOrder, type DetailsPatch, type DraftAction } from "../model/draft";
import { launchFromTarget, launchTargetText, withLaunchTarget } from "../model/entries";
import { ApplicationList } from "./ApplicationList";
import { EditDetailsDialog } from "./EditDetailsDialog";
import { LeaveDialog } from "./LeaveDialog";
import { ReadinessPanel } from "./ReadinessPanel";
import { RecentRestores } from "./RecentRestores";
import { RestoreSettings } from "./RestoreSettings";
import { UnsavedBar } from "./UnsavedBar";
import styles from "./WorkspaceEditor.module.css";

/** Actions that work on the saved workspace, so pending edits are saved first. */
type SavedAction = "restore" | "duplicate" | "export" | "update";

const SAVED_ACTION_COPY: Record<SavedAction, string> = {
  restore: "Restoring",
  duplicate: "Duplicating",
  export: "Exporting",
  update: "Updating from the desktop",
};

type SaveResult = { ok: true; workspace: Workspace } | { ok: false; error: ApiError };

/** The workspace page once the workspace is loaded: a local draft edited in place, saved explicitly. */
export function WorkspaceEditor({ workspace }: { workspace: Workspace }) {
  const id = workspace.id;
  const toast = useToast();
  const [state, dispatch] = useReducer(draftReducer, workspace, initDraft);
  const latest = useRef(state);
  useLayoutEffect(() => {
    latest.current = state;
  });

  // Fresh backend data (a refetch, a finished restore, our own save) is the new saved version.
  useEffect(() => {
    dispatch({ type: "load", workspace });
  }, [workspace]);

  const { draft, saved } = state;
  const dirty = isDirty(state);

  const health = useWorkspaceHealth(id);
  const healthById = useMemo(() => new Map((health.data?.entries ?? []).map((entry) => [entry.entryId, entry])), [health.data]);
  const activeWorkspaceId = useActiveWorkspaceId();
  const { running } = useActiveRuns();
  const activeRun = running.find((run) => run.workspaceId === id) ?? null;

  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<ApiError | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [switchOpen, setSwitchOpen] = useState(false);
  const [picker, setPicker] = useState<{ open: boolean; mode: "add" | "update" }>({ open: false, mode: "add" });
  const [savedAction, setSavedAction] = useState<{ open: boolean; action: SavedAction }>({ open: false, action: "restore" });
  const [choosing, setChoosing] = useState(false);
  const [fixingId, setFixingId] = useState<string | null>(null);
  const [highlightIds, setHighlightIds] = useState<ReadonlySet<string>>(() => new Set());

  const guard = useLeaveGuard(dirty);
  const actions = useWorkspaceActions({
    onDuplicated: (copy) => {
      guard.allowNext();
      navigate({ name: "workspace", id: copy.id });
    },
    onDeleted: () => {
      guard.allowNext();
      navigate({ name: "workspaces" }, { replace: true });
    },
  });

  // ---- Saving ---------------------------------------------------------------------------------

  const save = useCallback(
    async (draftToSave?: Workspace): Promise<SaveResult> => {
      const submitted = draftToSave ?? latest.current.draft;
      setSaving(true);
      setSaveError(null);
      try {
        const result = await api.updateWorkspace(prepareForSave(submitted));
        dispatch({ type: "saved", workspace: result, submitted });
        queryClient.setData(queryKeys.workspace(id), result);
        queryClient.invalidate(queryKeys.workspaces, queryKeys.activityAll, healthKey(id));
        return { ok: true, workspace: result };
      } catch (error) {
        const apiError = toApiError(error);
        setSaveError(apiError);
        return { ok: false, error: apiError };
      } finally {
        setSaving(false);
      }
    },
    [id],
  );

  const saveNow = useCallback(async () => {
    const result = await save();
    if (result.ok) toast.success("Changes saved");
  }, [save, toast]);

  const discard = () => {
    dispatch({ type: "discard" });
    setSaveError(null);
  };

  // A refused save stops being relevant as soon as the draft changes again.
  useEffect(() => {
    setSaveError(null);
  }, [state.draft]);

  // Ctrl/⌘+S saves.
  const shortcut = useRef({ saveNow, saving });
  useLayoutEffect(() => {
    shortcut.current = { saveNow, saving };
  });
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!hasPrimaryModifier(event) || event.shiftKey || event.altKey || event.key.toLowerCase() !== "s") return;
      if (document.getElementById("root")?.inert) return;
      event.preventDefault();
      if (isDirty(latest.current) && !shortcut.current.saving) void shortcut.current.saveNow();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // Apply an edit and save right away (fixes from the readiness report, the details dialog).
  const applyAndSave = async (action: DraftAction): Promise<SaveResult> => {
    const next = draftReducer(latest.current, action);
    dispatch(action);
    return save(next.draft);
  };

  // ---- Actions on the saved workspace ---------------------------------------------------------

  const runSavedAction = (action: SavedAction, target: WorkspaceRef) => {
    switch (action) {
      case "restore":
        setSwitchOpen(true);
        break;
      case "duplicate":
        void actions.duplicate(target);
        break;
      case "export":
        void actions.exportToFile(target);
        break;
      case "update":
        setPicker({ open: true, mode: "update" });
        break;
    }
  };

  const requestSavedAction = (action: SavedAction) => {
    if (isDirty(latest.current)) setSavedAction({ open: true, action });
    else runSavedAction(action, { id, name: latest.current.saved.name });
  };

  // ---- Fixing apps that can't start -----------------------------------------------------------

  const applyLaunchFix = async (entry: ApplicationEntry, launch: Pick<LaunchConfig, "executable" | "package">) => {
    const result = await applyAndSave({ type: "updateEntry", entryId: entry.id, update: (e) => withLaunchTarget(e, launch) });
    if (result.ok) toast.success(`${entry.displayName} updated`, `It will start from ${launchTargetText(launch) ?? "the chosen program"}.`);
  };

  const locate = async (entry: ApplicationEntry) => {
    let path: string | null;
    try {
      path = await pickFile(`Locate ${entry.displayName}`, parentFolder(entry.launch.executable));
    } catch (error) {
      toast.error("Couldn't open the file picker", errorMessage(error));
      return;
    }
    if (!path) return;
    setFixingId(entry.id);
    try {
      const resolved = await api.createManualEntry({ kind: "executable", path });
      await applyLaunchFix(entry, resolved.launch);
    } catch (error) {
      toast.error("Reframe can't start that file", errorMessage(error));
    } finally {
      setFixingId(null);
    }
  };

  const applyFoundLocation = async (entry: ApplicationEntry, target: LaunchTarget) => {
    const launch = launchFromTarget(target);
    if (!launch) return;
    setFixingId(entry.id);
    try {
      await applyLaunchFix(entry, launch);
    } finally {
      setFixingId(null);
    }
  };

  // ---- Adding apps ----------------------------------------------------------------------------

  // New rows are highlighted briefly (and scrolled to); the unsaved bar says the rest.
  const highlightTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(highlightTimer.current), []);

  const addEntries = (
    entries: readonly ApplicationEntry[],
    warnings: readonly string[] = [],
    monitors?: Workspace["monitors"],
  ) => {
    if (entries.length === 0) {
      toast.error("Nothing was added", warnings.join(" ") || "None of the chosen apps could be captured.");
      return;
    }
    dispatch({ type: "addEntries", entries, monitors });
    setHighlightIds(new Set(entries.map((entry) => entry.id)));
    clearTimeout(highlightTimer.current);
    highlightTimer.current = setTimeout(() => setHighlightIds(new Set()), 2400);
    if (warnings.length > 0) {
      const title = entries.length === 1 ? `Added ${entries[0].displayName}` : `Added ${pluralize(entries.length, "app")}`;
      toast.show({ tone: "info", title, description: warnings.join(" "), duration: 8000 });
    }
  };

  const chooseApp = async () => {
    let path: string | null;
    try {
      path = await pickFile("Choose an app to add");
    } catch (error) {
      toast.error("Couldn't open the file picker", errorMessage(error));
      return;
    }
    if (!path) return;
    setChoosing(true);
    try {
      const entry = await api.createManualEntry({ kind: "executable", path });
      addEntries([entry]);
    } catch (error) {
      toast.error("Reframe can't add that app", errorMessage(error));
    } finally {
      setChoosing(false);
    }
  };

  const onPickerConfirm = async (selections: CaptureSelection[]) => {
    if (picker.mode === "add") {
      try {
        const result = await api.captureApplications(selections);
        setPicker((p) => ({ ...p, open: false }));
        addEntries(result.entries, result.warnings, result.monitors);
      } catch (error) {
        toast.error("Couldn't add those apps", errorMessage(error));
      }
      return;
    }
    try {
      const result = await api.recaptureWorkspace(id, selections);
      dispatch({ type: "load", workspace: result.workspace });
      queryClient.setData(queryKeys.workspace(id), result.workspace);
      queryClient.invalidate(queryKeys.workspaces, queryKeys.activityAll, healthKey(id));
      setPicker((p) => ({ ...p, open: false }));
      toast.show({
        tone: "success",
        title: "Updated from your desktop",
        description:
          result.warnings.length > 0
            ? result.warnings.join(" ")
            : `${result.workspace.name} now has ${pluralize(result.workspace.applications.length, "app")}.`,
      });
    } catch (error) {
      toast.error("Couldn't update from the desktop", errorMessage(error));
    }
  };

  // ---- Render ---------------------------------------------------------------------------------

  const isActive = activeWorkspaceId.data === id;
  const hasEnabledApps = draft.applications.some((entry) => entry.enabled);
  const showBar = dirty || saveError !== null;

  const menuItems = [
    {
      label: "Update from current desktop",
      description: "Replace the apps with what's open now",
      icon: MonitorUp,
      onSelect: () => requestSavedAction("update"),
    },
    { type: "separator" as const },
    { label: "Duplicate", icon: Copy, onSelect: () => requestSavedAction("duplicate") },
    { label: "Export…", icon: FileUp, onSelect: () => requestSavedAction("export") },
    { type: "separator" as const },
    { label: "Delete workspace…", icon: Trash, danger: true, onSelect: () => actions.requestDelete({ id, name: saved.name }) },
  ];

  return (
    <Page className={cx(showBar && styles.withBar)}>
      <PageHeader
        back={{ label: "Workspaces", to: { name: "workspaces" } }}
        leading={<WorkspaceIcon emoji={draft.icon} color={draft.color} size={56} />}
        title={draft.name.trim() || "Untitled workspace"}
        subtitle={
          <>
            {draft.description.trim() && <span className={styles.description}>{draft.description}</span>}
            <span className={styles.meta}>
              {activeRun ? (
                <Badge tone="accent" className={styles.metaBadge}>
                  <Spinner size={12} />
                  Restoring now
                </Badge>
              ) : (
                isActive && (
                  <Badge tone="accent" dot className={styles.metaBadge}>
                    Active
                  </Badge>
                )
              )}
              <span className={styles.metaItem}>{pluralize(draft.applications.length, "app")}</span>
              <span className={styles.metaItem}>
                <RelativeTime value={saved.lastRestoredAt} prefix="Restored " fallback="Never restored" />
              </span>
              <span className={styles.metaItem}>
                <RelativeTime value={saved.updatedAt} prefix="Edited " />
              </span>
            </span>
          </>
        }
        actions={
          <>
            <Button variant="secondary" icon={Pencil} onClick={() => setDetailsOpen(true)}>
              Edit details
            </Button>
            {activeRun ? (
              <Button variant="primary" icon={Activity} onClick={() => navigate({ name: "restore", runId: activeRun.id })}>
                View restore
              </Button>
            ) : (
              <Button
                variant="primary"
                icon={RotateCcw}
                disabled={!hasEnabledApps}
                title={hasEnabledApps ? undefined : "Add or turn on an app to restore this workspace"}
                onClick={() => requestSavedAction("restore")}
              >
                Restore…
              </Button>
            )}
            <Menu
              aria-label="Workspace actions"
              minWidth={250}
              items={menuItems}
              trigger={(props) => <IconButton {...props} variant="secondary" icon={Ellipsis} aria-label="More actions" />}
            />
          </>
        }
      />

      <div className={styles.sections}>
        {/* Readiness describes saved apps; until there are some, the empty list says it all. */}
        {saved.applications.length > 0 && (
          <ReadinessPanel
            health={health.data}
            error={health.error}
            isFetching={health.isFetching}
            onRefresh={() => void health.refresh()}
            applications={saved.applications}
            stale={!sameEntriesIgnoringOrder(draft.applications, saved.applications)}
            onLocate={(entry) => void locate(entry)}
            onUseFound={(entry, target) => void applyFoundLocation(entry, target)}
            fixingId={fixingId}
          />
        )}

        <ApplicationList
          entries={draft.applications}
          savedEntries={saved.applications}
          monitors={draft.monitors}
          restore={draft.restore}
          healthById={healthById}
          dispatch={dispatch}
          expandedId={expandedId}
          onExpandedChange={setExpandedId}
          highlightIds={highlightIds}
          onAddFromDesktop={() => setPicker({ open: true, mode: "add" })}
          onChooseApp={() => void chooseApp()}
          choosing={choosing}
        />

        <div className={styles.columns}>
          <RestoreSettings restore={draft.restore} onChange={(patch) => dispatch({ type: "setRestore", patch })} />
          <RecentRestores workspaceId={id} />
        </div>
      </div>

      {showBar && (
        <UnsavedBar dirty={dirty} saving={saving} error={saveError} onSave={() => void saveNow()} onDiscard={discard} />
      )}

      <EditDetailsDialog
        open={detailsOpen}
        onClose={() => setDetailsOpen(false)}
        initial={{ name: draft.name, icon: draft.icon, color: draft.color, description: draft.description }}
        onSave={async (patch: Required<DetailsPatch>) => {
          const result = await applyAndSave({ type: "setDetails", patch });
          if (!result.ok) throw result.error;
          toast.success("Details saved");
        }}
      />

      <SwitchDialog
        open={switchOpen}
        onClose={() => setSwitchOpen(false)}
        workspace={{ id, name: saved.name, icon: saved.icon, color: saved.color }}
        defaultMode={saved.restore.defaultMode}
      />

      <AppPickerDialog
        open={picker.open}
        onClose={() => setPicker((p) => ({ ...p, open: false }))}
        title={picker.mode === "add" ? "Add applications" : "Update from current desktop"}
        confirmLabel={picker.mode === "add" ? "Add" : "Update workspace"}
        onConfirm={onPickerConfirm}
        onManualEntry={picker.mode === "add" ? (entry) => addEntries([entry]) : undefined}
      />

      <ConfirmDialog
        open={savedAction.open}
        onClose={() => setSavedAction((s) => ({ ...s, open: false }))}
        title="Save your changes first?"
        description={`${SAVED_ACTION_COPY[savedAction.action]} uses the saved version of ${draft.name.trim() || "this workspace"}.`}
        confirmLabel="Save and continue"
        onConfirm={async () => {
          const result = await save();
          if (!result.ok) throw result.error;
          runSavedAction(savedAction.action, { id, name: result.workspace.name });
        }}
      />

      <LeaveDialog
        open={guard.blocked}
        workspaceName={draft.name.trim() || "this workspace"}
        onStay={guard.stay}
        onDiscard={() => {
          dispatch({ type: "discard" });
          guard.proceed();
        }}
        onSave={async () => {
          const result = await save();
          if (result.ok) guard.proceed();
          else guard.stay();
          return result.ok;
        }}
      />

      {actions.dialogs}
    </Page>
  );
}
