// Plain-language notes about what a desktop scan could and couldn't see
// (tested in notices.test.ts).

import type { DesktopSnapshot, PermissionKind, PermissionStatus } from "@/types";

export interface DesktopNotes {
  /** Set when this system doesn't let Reframe list windows at all: the reason, in plain words. */
  blocked: string | null;
  /** Limitations of this scan worth knowing about (deduplicated). */
  notes: string[];
}

/** Platform warnings from `discovery::gather` start with the part that failed. */
const FAILED_PART: ReadonlyArray<[prefix: string, lead: string]> = [
  ["Monitors:", "Couldn't read your display layout"],
  ["Windows:", "Couldn't list the open windows"],
  ["Processes:", "Couldn't read the running programs"],
];

const BLOCKED_FALLBACK = "This computer doesn't let Reframe see which windows are open.";

/** Backend summaries that are followed by a reason ("…may be missing: <capability note>"). */
const SUMMARY_LEADS = ["Some open windows may be missing", "Reframe cannot list open windows on this system"];

/** Reword a raw scan warning ("Monitors: …" → "Couldn't read your display layout — …"). */
export function plainWarning(raw: string): string {
  const text = raw.trim();
  for (const [prefix, lead] of FAILED_PART) {
    if (text.startsWith(prefix)) {
      const reason = text.slice(prefix.length).trim();
      return reason ? `${lead} — ${sentence(reason)}` : `${lead}.`;
    }
  }
  // "Some open windows may be missing: Wayland session: only…" reads better as two sentences.
  for (const lead of SUMMARY_LEADS) {
    if (text.startsWith(`${lead}:`)) {
      const reason = text.slice(lead.length + 1).trim();
      return reason ? `${lead}. ${sentence(reason)}` : `${lead}.`;
    }
  }
  return sentence(text);
}

/** Capitalize the first letter and end with a period. */
function sentence(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  const capitalized = trimmed[0].toUpperCase() + trimmed.slice(1);
  return /[.!?…]$/.test(capitalized) ? capitalized : `${capitalized}.`;
}

function normalize(text: string): string {
  return text.trim().replace(/[.\s]+$/, "").toLowerCase();
}

export function desktopNotes(snapshot: Pick<DesktopSnapshot, "warnings" | "platform">): DesktopNotes {
  const { listWindows, listMonitors } = snapshot.platform.capabilities;
  const listingNote = listWindows.note?.trim() || null;
  const blocked = listWindows.level === "unsupported" ? (listingNote ? sentence(listingNote) : BLOCKED_FALLBACK) : null;

  const notes: string[] = [];
  const seen = new Set<string>();
  const add = (text: string) => {
    const key = normalize(text);
    if (!key || seen.has(key)) return;
    // Already explained by another note (the backend often quotes capability notes).
    if ([...seen].some((existing) => existing.includes(key) || key.includes(existing))) return;
    seen.add(key);
    notes.push(text);
  };

  for (const warning of snapshot.warnings) {
    // When listing is impossible the page explains why once; don't repeat it in every note.
    if (blocked && listingNote && normalize(warning).includes(normalize(listingNote))) continue;
    if (blocked && warning.startsWith("Reframe cannot list open windows")) continue;
    add(plainWarning(warning));
  }

  const monitorNote = listMonitors.note?.trim();
  if (listMonitors.level !== "full" && monitorNote && !(blocked && normalize(monitorNote) === normalize(listingNote ?? ""))) {
    add(sentence(monitorNote));
  }
  return { blocked, notes };
}

export const PERMISSION_NAMES: Record<PermissionKind, string> = {
  accessibility: "Accessibility",
  screenRecording: "Screen Recording",
  automation: "Automation",
};

/** Permissions Reframe still needs, in the order the platform reports them. */
export function missingPermissions(list: readonly PermissionStatus[] | undefined): PermissionStatus[] {
  return (list ?? []).filter((permission) => !permission.granted);
}
