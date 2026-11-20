import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RestoreEvent } from "@/types";

/** Tauri event name for restore progress (see src-tauri/src/events.rs). */
export const RESTORE_EVENT = "reframe://restore";

/** Subscribe to restore progress. Returns an unsubscribe function. */
export function onRestoreEvent(handler: (event: RestoreEvent) => void): Promise<UnlistenFn> {
  return listen<RestoreEvent>(RESTORE_EVENT, (e) => handler(e.payload));
}

/**
 * Tauri event name for navigation requests from outside the webview, e.g. the system tray
 * ("Capture desktop", "Restore <workspace>"); see src-tauri/src/tray.rs.
 */
export const NAVIGATE_EVENT = "reframe://navigate";

/** Payload of `reframe://navigate`. */
export interface NavigateRequest {
  route: "home" | "explore" | "restore";
  /** The restore run to show (route `restore`). */
  runId?: string | null;
  /** Something went wrong (e.g. a restore could not start): show it as an error. */
  message?: string | null;
}

/**
 * Subscribe to navigation requests. The payload crosses a process boundary, so it is passed on
 * as `unknown`: validate it with `parseNavigateRequest` (src/lib/navigation.ts).
 */
export function onNavigateRequest(handler: (payload: unknown) => void): Promise<UnlistenFn> {
  return listen<unknown>(NAVIGATE_EVENT, (e) => handler(e.payload));
}
