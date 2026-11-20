// Shared contract: pick running applications (and optionally specific windows) from the current
// desktop. Used by the workspace page ("Add applications", "Update from current desktop").
//
// Behaviour:
// - Opening the picker scans the desktop again unless the last scan is only a few seconds old,
//   so apps opened a moment ago are listed. Suggested apps start checked, as in Explore.
// - Confirming calls `onConfirm` with the selections and closes the picker once it resolves; if
//   it throws/rejects, the picker stays open and shows the error as a toast.
// - "Choose an app manually…" (only with `onManualEntry`) opens the system file picker, turns
//   the chosen executable or app bundle into an entry, passes it to `onManualEntry` and closes.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { AppWindow, FolderOpen, MonitorOff, RefreshCw } from "lucide-react";
import type { ApplicationEntry, CaptureSelection, DiscoveredApplication } from "@/types";
import { Button, Dialog, EmptyState, IconButton, RelativeTime, useToast } from "@/components/ui";
import { useDesktopSnapshot } from "@/hooks/useDesktopSnapshot";
import { usePlatformInfo } from "@/hooks/usePlatformInfo";
import { api, errorMessage } from "@/lib/api";
import { formatCount } from "@/lib/format";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { AppList, AppListSkeleton } from "./AppList";
import { desktopNotes } from "./notices";
import { useAppSelection } from "./useAppSelection";
import styles from "./AppPickerDialog.module.css";

export interface AppPickerDialogProps {
  open: boolean;
  onClose: () => void;
  /** e.g. "Add applications" or "Update from current desktop". */
  title: string;
  /** Primary button label; the picker may append a count ("Add 3 apps"). */
  confirmLabel: string;
  /** Called with the chosen applications/windows; the caller decides what to do with them. */
  onConfirm: (selections: CaptureSelection[]) => Promise<void> | void;
  /** When provided, the picker offers "Choose an app manually…" (executable / app bundle). */
  onManualEntry?: (entry: ApplicationEntry) => void;
}

/** A scan younger than this is reused when the picker opens. */
const FRESH_MS = 3_000;
const NO_APPS: DiscoveredApplication[] = [];

/** "Add applications" → "Couldn't add applications". */
function failureTitle(title: string): string {
  const text = title.trim();
  return text ? `Couldn't ${text[0].toLowerCase()}${text.slice(1)}` : "Something went wrong";
}

export function AppPickerDialog({ open, onClose, title, confirmLabel, onConfirm, onManualEntry }: AppPickerDialogProps) {
  const toast = useToast();
  const snapshot = useDesktopSnapshot({ enabled: open });
  const platform = usePlatformInfo();
  const { refresh, updatedAt } = snapshot;
  const data = snapshot.data;
  const apps = data?.applications ?? NO_APPS;
  const selection = useAppSelection(apps);
  const [confirming, setConfirming] = useState(false);
  const [choosingFile, setChoosingFile] = useState(false);
  // Focus lands on the content (not the first button, which is "Scan again"); Tab moves on from there.
  const contentRef = useRef<HTMLDivElement>(null);
  const updatedAtRef = useRef(updatedAt);
  useLayoutEffect(() => {
    updatedAtRef.current = updatedAt;
  });

  const { reset } = selection;
  useEffect(() => {
    if (!open) return;
    reset();
    setConfirming(false);
    setChoosingFile(false);
    // The snapshot hook has already started a scan if its data was stale; otherwise rescan
    // unless the last scan is only a few seconds old.
    const { isFetching } = queryClient.getState(queryKeys.desktopSnapshot);
    if (!isFetching && Date.now() - updatedAtRef.current > FRESH_MS) void refresh();
  }, [open, reset, refresh]);

  const busy = confirming || choosingFile;
  const count = selection.counts.apps;
  const notes = data ? desktopNotes(data) : null;

  const confirm = async () => {
    if (count === 0) return;
    setConfirming(true);
    try {
      await onConfirm(selection.selections);
      onClose();
    } catch (error) {
      toast.error(failureTitle(title), errorMessage(error));
    } finally {
      setConfirming(false);
    }
  };

  const chooseManually = async () => {
    if (!onManualEntry) return;
    const os = platform.data?.os;
    let path: string | null;
    try {
      // macOS app bundles are file packages: the file picker lets you choose them (a folder
      // picker greys them out), and plain executables stay selectable too.
      const picked = await openFileDialog({
        title: "Choose an application",
        multiple: false,
        directory: false,
        defaultPath: os === "macos" ? "/Applications" : undefined,
        filters: os === "windows" ? [{ name: "Applications", extensions: ["exe", "bat", "cmd", "com"] }] : undefined,
      });
      path = typeof picked === "string" ? picked : null;
    } catch (error) {
      toast.error("Couldn't open the file picker", errorMessage(error));
      return;
    }
    if (!path) return;
    setChoosingFile(true);
    try {
      const entry = await api.createManualEntry({ kind: "executable", path });
      onManualEntry(entry);
      onClose();
    } catch (error) {
      toast.error("Couldn't add that app", errorMessage(error));
    } finally {
      setChoosingFile(false);
    }
  };

  const manualButton = onManualEntry && (
    <Button variant="ghost" icon={FolderOpen} loading={choosingFile} disabled={confirming} onClick={() => void chooseManually()}>
      Choose an app manually…
    </Button>
  );

  let body;
  if (!data || !notes) {
    body = snapshot.error ? (
      <EmptyState
        size="compact"
        tone="danger"
        icon={MonitorOff}
        title="Couldn't look at your desktop"
        text={errorMessage(snapshot.error)}
        action={
          <Button size="sm" variant="secondary" icon={RefreshCw} loading={snapshot.isFetching} onClick={() => void refresh()}>
            Try again
          </Button>
        }
      />
    ) : (
      <div role="status" aria-label="Looking at your desktop…">
        <AppListSkeleton rows={4} variant="dialog" />
      </div>
    );
  } else if (notes.blocked) {
    body = (
      <EmptyState
        size="compact"
        icon={MonitorOff}
        title="Reframe can't see app windows here"
        text={onManualEntry ? `${notes.blocked} You can still choose an app manually.` : notes.blocked}
      />
    );
  } else if (apps.length === 0) {
    body = (
      <EmptyState
        size="compact"
        icon={AppWindow}
        title="Nothing's open right now"
        text="Open the app you want to add, then scan again."
        action={
          <Button size="sm" variant="secondary" icon={RefreshCw} loading={snapshot.isFetching} onClick={() => void refresh()}>
            Scan again
          </Button>
        }
      />
    );
  } else {
    body = (
      <AppList
        apps={apps}
        monitors={data.monitors}
        selection={selection}
        variant="dialog"
        toolbar={
          <span className={styles.scan}>
            <span className={styles.scanned}>
              {snapshot.isFetching ? "Scanning…" : <RelativeTime value={data.capturedAt} prefix="Checked " />}
            </span>
            <IconButton
              icon={RefreshCw}
              size="sm"
              aria-label="Scan again"
              tooltip
              loading={snapshot.isFetching}
              onClick={() => void refresh()}
            />
          </span>
        }
      />
    );
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      description="Choose apps that are open right now — expand one to pick specific windows."
      size="lg"
      dismissible={!busy}
      initialFocusRef={contentRef}
      footer={
        <>
          {manualButton && <span className={styles.footerStart}>{manualButton}</span>}
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button
            variant="primary"
            loading={confirming}
            disabled={count === 0 || choosingFile}
            onClick={() => void confirm()}
          >
            {confirmLabel}
            {count > 0 && <span className={styles.count}>{formatCount(count)}</span>}
          </Button>
        </>
      }
    >
      <div ref={contentRef} tabIndex={-1} className={styles.content}>
        {body}
      </div>
    </Dialog>
  );
}
