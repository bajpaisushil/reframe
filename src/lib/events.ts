import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RestoreEvent } from "@/types";

/** Tauri event name for restore progress (see src-tauri/src/events.rs). */
export const RESTORE_EVENT = "reframe://restore";

/** Subscribe to restore progress. Returns an unsubscribe function. */
export function onRestoreEvent(handler: (event: RestoreEvent) => void): Promise<UnlistenFn> {
  return listen<RestoreEvent>(RESTORE_EVENT, (e) => handler(e.payload));
}
