// Color and identity helpers: curated workspace colors, deterministic avatar colors for apps
// without an icon, and initials. Pure functions (tested in color.test.ts).

/** A curated workspace accent color. `value` is stored on the workspace as `#RRGGBB`. */
export interface WorkspaceColor {
  id: string;
  name: string;
  value: string;
}

/** The eight workspace accent colors offered by the color picker (plus "no color"). */
export const WORKSPACE_COLORS: readonly WorkspaceColor[] = [
  { id: "indigo", name: "Indigo", value: "#6D5DFC" },
  { id: "blue", name: "Blue", value: "#3E7BFA" },
  { id: "teal", name: "Teal", value: "#14A3A0" },
  { id: "green", name: "Green", value: "#2E9E5B" },
  { id: "amber", name: "Amber", value: "#E0A21B" },
  { id: "orange", name: "Orange", value: "#EB7A34" },
  { id: "rose", name: "Rose", value: "#E5484D" },
  { id: "pink", name: "Pink", value: "#D6409F" },
];

const HEX_6 = /^#?([0-9a-f]{6})$/i;
const HEX_3 = /^#?([0-9a-f]{3})$/i;

/**
 * Normalize a user/API color to uppercase `#RRGGBB` (accepts `#RGB` and a missing `#`).
 * Returns `null` for anything else, so callers can fall back to a neutral tint.
 */
export function normalizeHexColor(value: string | null | undefined): string | null {
  if (!value) return null;
  const trimmed = value.trim();
  const six = HEX_6.exec(trimmed);
  if (six) return `#${six[1].toUpperCase()}`;
  const three = HEX_3.exec(trimmed);
  if (three) {
    const [r, g, b] = three[1].toUpperCase();
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return null;
}

/** `{ r, g, b }` (0-255) for a hex color, or `null` when it is not a valid color. */
export function hexToRgb(value: string | null | undefined): { r: number; g: number; b: number } | null {
  const hex = normalizeHexColor(value);
  if (!hex) return null;
  const n = Number.parseInt(hex.slice(1), 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

/** `rgba(...)` string for a hex color at the given alpha, or `null` for invalid input. */
export function hexToRgba(value: string | null | undefined, alpha: number): string | null {
  const rgb = hexToRgb(value);
  if (!rgb) return null;
  const a = Math.min(1, Math.max(0, alpha));
  return `rgba(${rgb.r}, ${rgb.g}, ${rgb.b}, ${a})`;
}

/** Name of a curated workspace color ("Indigo"), or `null` for custom / missing colors. */
export function workspaceColorName(value: string | null | undefined): string | null {
  const hex = normalizeHexColor(value);
  if (!hex) return null;
  return WORKSPACE_COLORS.find((c) => c.value === hex)?.name ?? null;
}

/** 32-bit FNV-1a hash: fast, stable across sessions and platforms. */
export function hashString(value: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < value.length; i++) {
    hash ^= value.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** Number of generated avatar palettes defined in tokens.css (`--avatar-bg-N` / `--avatar-fg-N`). */
export const AVATAR_COLOR_COUNT = 8;

/** Deterministic avatar palette index for an app id (case-insensitive). */
export function avatarColorIndex(key: string): number {
  return hashString(key.trim().toLowerCase()) % AVATAR_COLOR_COUNT;
}

/** Minimal `Intl.Segmenter` shape (the project's TS lib target predates its typings). */
type GraphemeSegmenter = new (
  locale?: string,
  options?: { granularity: "grapheme" },
) => { segment(input: string): Iterable<{ segment: string }> };

/** First user-perceived character (grapheme cluster), so emoji and accents stay intact. */
export function firstGrapheme(value: string): string {
  if (!value) return "";
  const Segmenter = (Intl as unknown as { Segmenter?: GraphemeSegmenter }).Segmenter;
  if (Segmenter) {
    for (const { segment } of new Segmenter(undefined, { granularity: "grapheme" }).segment(value)) {
      return segment;
    }
    return "";
  }
  return Array.from(value)[0] ?? "";
}

const FILE_SUFFIX = /\.(exe|app|desktop|appimage|lnk)$/i;

/**
 * One or two initials for an application name: "Visual Studio Code" -> "VS", "GitHub" -> "GH",
 * "firefox" -> "F", "微信" -> "微". Returns "?" when nothing usable is left.
 */
export function initials(name: string): string {
  const cleaned = name.trim().replace(FILE_SUFFIX, "").trim();
  if (!cleaned) return "?";
  const words = cleaned.split(/[\s_\-./\\:()[\]]+/u).filter((w) => /[\p{L}\p{N}\p{Extended_Pictographic}]/u.test(w));
  if (words.length === 0) return "?";
  if (words.length >= 2) {
    return (firstGrapheme(words[0]) + firstGrapheme(words[1])).toLocaleUpperCase();
  }
  const word = words[0];
  // A camel-case hump ("GitHub", "iTerm") gives a second initial.
  const hump = /\p{Ll}(\p{Lu})/u.exec(word);
  const first = firstGrapheme(word);
  return (hump ? first + hump[1] : first).toLocaleUpperCase();
}
