import { useEffect, useState } from "react";
import { Button, Dialog, WorkspaceIcon, useToast } from "@/components/ui";
import { useWorkspaces } from "@/hooks/useWorkspaces";
import { api, errorMessage } from "@/lib/api";
import { onRestoreEvent } from "@/lib/events";
import { navigate } from "@/lib/router";
import type { PendingRestore, StartupContext } from "@/types";
import styles from "./StartupRestorePrompt.module.css";

/** How long to wait for an automatic startup restore to begin (the engine waits ~4 s first). */
const AUTO_START_TIMEOUT_MS = 30_000;
const POLL_MS = 2_000;

/**
 * Startup restore, mounted once next to the app shell. When Reframe was opened at login:
 * - "ask before restoring" on: offer to restore the workspace (Merge, which never closes anything);
 * - off: the engine starts the restore itself; follow it by opening its progress page.
 * Either way the pending restore is dismissed so it is handled exactly once.
 */
export function StartupRestorePrompt() {
  const toast = useToast();
  const workspaces = useWorkspaces();
  // The pending restore stays set after closing so the dialog can animate out with its content.
  const [pending, setPending] = useState<PendingRestore | null>(null);
  const [open, setOpen] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;
    let waitingFor: PendingRestore | null = null;
    let deadline = 0;

    const follow = (runId: string) => {
      if (disposed) return;
      disposed = true;
      clearTimeout(pollTimer);
      unlisten?.();
      void api.dismissStartupRestore().catch(() => undefined);
      navigate({ name: "restore", runId });
    };

    const poll = () => {
      pollTimer = setTimeout(async () => {
        if (disposed || !waitingFor) return;
        try {
          const context = await api.startupContext();
          if (disposed) return;
          const runId = context.pendingRestore?.runId;
          if (runId) return follow(runId);
          if (!context.pendingRestore) return; // dismissed elsewhere
        } catch {
          // Try again until the deadline.
        }
        if (Date.now() < deadline) return poll();
        const name = waitingFor.workspaceName;
        disposed = true;
        unlisten?.();
        void api.dismissStartupRestore().catch(() => undefined);
        toast.error(`Couldn't restore ${name} at startup`, "Open the workspace and restore it from there.");
      }, POLL_MS);
    };

    void (async () => {
      // Listen first so the start of an automatic restore is not missed.
      try {
        const stop = await onRestoreEvent((event) => {
          if (event.type === "runStarted" && waitingFor && event.run.workspaceId === waitingFor.workspaceId) {
            follow(event.run.id);
          }
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch {
        // Polling below still finds the run.
      }
      let context: StartupContext;
      try {
        context = await api.startupContext();
      } catch (e) {
        console.warn("Reframe: could not read the startup context", e);
        return;
      }
      if (disposed) return;
      const restore = context.pendingRestore;
      if (!restore) {
        unlisten?.();
        return;
      }
      if (restore.ask) {
        unlisten?.();
        unlisten = null;
        setPending(restore);
        setOpen(true);
        return;
      }
      if (restore.runId) return follow(restore.runId);
      waitingFor = restore;
      deadline = Date.now() + AUTO_START_TIMEOUT_MS;
      poll();
    })();

    return () => {
      disposed = true;
      clearTimeout(pollTimer);
      unlisten?.();
    };
    // `toast` is stable for the app's lifetime; this runs once per launch.
  }, [toast]);

  if (!pending) return null;

  const color = workspaces.data?.find((w) => w.id === pending.workspaceId)?.color ?? null;

  const notNow = () => {
    setOpen(false);
    void api.dismissStartupRestore().catch(() => undefined);
  };

  const restoreNow = async () => {
    setStarting(true);
    setError(null);
    try {
      const runId = await api.startRestore({
        workspaceId: pending.workspaceId,
        mode: "merge",
        entryIds: null,
        closeWindowIds: [],
      });
      setOpen(false);
      void api.dismissStartupRestore().catch(() => undefined);
      navigate({ name: "restore", runId });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setStarting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={notNow}
      size="sm"
      dismissible={!starting}
      title={`Restore ${pending.workspaceName}?`}
      footer={
        <>
          <Button variant="secondary" onClick={notNow} disabled={starting}>
            Not now
          </Button>
          <Button variant="primary" loading={starting} onClick={() => void restoreNow()} data-autofocus>
            Restore now
          </Button>
        </>
      }
    >
      <div className={styles.content}>
        <WorkspaceIcon emoji={pending.workspaceIcon} color={color} size={44} />
        <p className={styles.text}>
          Reframe opened when you logged in. Bring back the apps and windows of{" "}
          <span className={styles.name}>{pending.workspaceName}</span>? Anything already open stays open.
        </p>
      </div>
      {error && (
        <p role="alert" className={styles.error}>
          {error}
        </p>
      )}
    </Dialog>
  );
}
