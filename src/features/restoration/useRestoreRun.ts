import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { onRestoreEvent } from "@/lib/events";
import type { RestoreEvent, RestoreRun } from "@/types";
import { applyRunEvent, replayRunEvents } from "./runState";

export interface RestoreRunState {
  run: RestoreRun | null;
  /** Loading failed and nothing is known about the run. */
  error: unknown;
  /** The first load finished (successfully or not). */
  loaded: boolean;
  /** Live progress could not be subscribed to; the run is refreshed by polling instead. */
  polling: boolean;
}

const POLL_MS = 1500;

/**
 * One restore run, kept live: subscribes to `reframe://restore` first, then loads the run and
 * replays whatever arrived meanwhile, so no update is lost between the two. If events are not
 * available the run is polled while it is running.
 */
export function useRestoreRun(runId: string): RestoreRunState {
  const [state, setState] = useState<RestoreRunState & { runId: string }>({
    runId,
    run: null,
    error: null,
    loaded: false,
    polling: false,
  });

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let buffer: RestoreEvent[] | null = [];
    let current: RestoreRun | null = null;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;
    setState({ runId, run: null, error: null, loaded: false, polling: false });

    const publish = (patch: Partial<RestoreRunState>) => {
      if (!disposed) setState((previous) => ({ ...previous, runId, ...patch }));
    };

    const onEvent = (event: RestoreEvent) => {
      if (disposed) return;
      if (buffer) {
        buffer.push(event);
        return;
      }
      const next = applyRunEvent(current, runId, event);
      if (next !== current) {
        current = next;
        publish({ run: next, error: null });
      }
    };

    const poll = () => {
      pollTimer = setTimeout(async () => {
        if (disposed) return;
        try {
          const fresh = await api.restoreRun(runId);
          if (disposed) return;
          current = fresh;
          publish({ run: fresh, error: null });
          if (fresh.status !== "running") return;
        } catch {
          // Keep showing the last known state; try again shortly.
        }
        poll();
      }, POLL_MS);
    };

    void (async () => {
      let listening = true;
      try {
        const stop = await onRestoreEvent(onEvent);
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (error) {
        listening = false;
        console.warn("Reframe: live restore progress is unavailable", error);
      }
      if (disposed) return;

      let loadError: unknown = null;
      try {
        current = await api.restoreRun(runId);
      } catch (error) {
        loadError = error;
      }
      if (disposed) return;
      const pending = buffer ?? [];
      buffer = null;
      current = replayRunEvents(current, runId, pending);
      publish({ run: current, error: current ? null : loadError, loaded: true, polling: !listening });
      if (!listening && current?.status === "running") poll();
    })();

    return () => {
      disposed = true;
      clearTimeout(pollTimer);
      unlisten?.();
    };
  }, [runId]);

  // Never show a previous run's state while the next one loads.
  if (state.runId !== runId) return { run: null, error: null, loaded: false, polling: false };
  return { run: state.run, error: state.error, loaded: state.loaded, polling: state.polling };
}
