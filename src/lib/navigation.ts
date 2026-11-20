// Validation of navigation requests sent by the native side (`reframe://navigate`, e.g. from the
// system tray). Pure (tested in navigation.test.ts).

import type { NavigateRequest } from "./events";
import type { Route } from "./router";

export interface ParsedNavigateRequest {
  /** Where to go; `null` when the request names no usable destination. */
  route: Route | null;
  /** Error to show, if any (trimmed, never empty). */
  message: string | null;
}

const ROUTES: ReadonlySet<NavigateRequest["route"]> = new Set(["home", "explore", "restore"]);

function nonEmptyString(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}

/**
 * Turn an event payload into a destination and an optional message. Unknown routes and a
 * `restore` request without a run id are ignored (the message, if any, is still shown).
 */
export function parseNavigateRequest(payload: unknown): ParsedNavigateRequest {
  if (!payload || typeof payload !== "object") return { route: null, message: null };
  const { route, runId, message } = payload as Record<string, unknown>;
  const text = nonEmptyString(message);
  if (typeof route !== "string" || !ROUTES.has(route as NavigateRequest["route"])) {
    return { route: null, message: text };
  }
  switch (route as NavigateRequest["route"]) {
    case "home":
      return { route: { name: "home" }, message: text };
    case "explore":
      return { route: { name: "explore" }, message: text };
    case "restore": {
      const id = nonEmptyString(runId);
      return { route: id ? { name: "restore", runId: id } : null, message: text };
    }
  }
}
