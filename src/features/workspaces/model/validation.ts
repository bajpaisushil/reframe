// Parsing and validation of what users type in the workspace editor. Pure (tested in
// validation.test.ts).

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
const ALLOWED_PROTOCOLS = new Set(["http:", "https:", "file:"]);

export interface TabAddress {
  url: string;
  /** A readable stand-in title until the browser loads the page. */
  title: string;
}

/**
 * Turn what someone typed into a tab address. "example.com/docs" becomes
 * "https://example.com/docs"; only web (http/https) and file addresses are accepted.
 */
export function parseTabAddress(input: string): Parsed<TabAddress> {
  const text = input.trim();
  if (text === "") return { ok: false, error: "Enter a web address." };
  if (/\s/.test(text)) return { ok: false, error: "Web addresses can't contain spaces." };
  const hasScheme = SCHEME.test(text) && !/^[^:/]+:\d+(\/|$)/.test(text); // "localhost:3000" has no scheme
  let prefix = "";
  if (!hasScheme) {
    const host = text.split(/[/?#]/, 1)[0].replace(/:\d+$/, "");
    if (!host.includes(".") && host !== "localhost") {
      return { ok: false, error: "That doesn't look like a web address. Try something like example.com." };
    }
    prefix = host === "localhost" || /^127\.\d+\.\d+\.\d+$/.test(host) ? "http://" : "https://";
  }
  let url: URL;
  try {
    url = new URL(prefix + text);
  } catch {
    return { ok: false, error: "That doesn't look like a web address." };
  }
  if (!ALLOWED_PROTOCOLS.has(url.protocol)) {
    return { ok: false, error: "Only web (http or https) and file addresses can be opened as tabs." };
  }
  if (url.protocol !== "file:" && url.hostname === "") {
    return { ok: false, error: "That doesn't look like a web address." };
  }
  const title =
    url.protocol === "file:"
      ? decodeURIComponent(url.pathname.split("/").filter(Boolean).pop() ?? url.pathname)
      : url.hostname.replace(/^www\./, "");
  return { ok: true, value: { url: url.href, title } };
}

export interface NumberRange {
  min: number;
  max: number;
  /** Allow an empty field (meaning "use the default"). */
  optional?: boolean;
}

function parseNumberText(text: string): number | null {
  const normalized = text.trim().replace(",", ".");
  if (!/^\d*(\.\d*)?$/.test(normalized) || normalized === "" || normalized === ".") return null;
  return Number(normalized);
}

/** Seconds as typed ("1.5", "20") to whole milliseconds, within [min, max] seconds. */
export function parseSeconds(text: string, range: NumberRange): Parsed<number | null> {
  if (text.trim() === "") {
    return range.optional ? { ok: true, value: null } : { ok: false, error: "Enter a number of seconds." };
  }
  const seconds = parseNumberText(text);
  if (seconds === null) return { ok: false, error: "Enter a number of seconds, like 1.5." };
  if (seconds < range.min || seconds > range.max) {
    return { ok: false, error: `Use ${formatNumber(range.min)}–${formatNumber(range.max)} seconds.` };
  }
  return { ok: true, value: Math.round(seconds * 1000) };
}

/** A whole number within [min, max]. */
export function parseWholeNumber(text: string, range: NumberRange): Parsed<number | null> {
  if (text.trim() === "") {
    return range.optional ? { ok: true, value: null } : { ok: false, error: "Enter a number." };
  }
  const value = parseNumberText(text);
  if (value === null || !Number.isInteger(value)) return { ok: false, error: "Enter a whole number." };
  if (value < range.min || value > range.max) {
    return { ok: false, error: `Use a number from ${range.min} to ${range.max}.` };
  }
  return { ok: true, value };
}

function formatNumber(value: number): string {
  return String(Math.round(value * 100) / 100);
}

/** Milliseconds as seconds for an input: 300 → "0.3", 20000 → "20". */
export function formatSeconds(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "";
  return formatNumber(ms / 1000);
}

/** Milliseconds as a short label: 300 → "0.3 s", 20000 → "20 s". */
export function secondsLabel(ms: number): string {
  return `${formatSeconds(ms)} s`;
}

/** A positive 1-based page number, or null for an empty field. */
export function parsePage(text: string): Parsed<number | null> {
  return parseWholeNumber(text, { min: 1, max: 100_000, optional: true });
}

const INVALID_FILE_CHARS = /[\\/:*?"<>|\u0000-\u001f]/g;

/** Default export file name: "Deep work.reframe.json" (characters invalid in file names removed). */
export function exportFileName(workspaceName: string): string {
  const base = workspaceName
    .replace(INVALID_FILE_CHARS, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^\.+/, "")
    .slice(0, 80)
    .trim();
  return `${base || "Workspace"}.reframe.json`;
}
