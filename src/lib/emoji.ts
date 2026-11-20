// Curated workspace emoji and validation of custom emoji input (tested in emoji.test.ts).

import { firstGrapheme } from "./color";

export interface CuratedEmoji {
  emoji: string;
  label: string;
}

/** 48 emoji covering common kinds of work and life contexts, in picker order. */
export const CURATED_EMOJI: readonly CuratedEmoji[] = [
  { emoji: "💼", label: "Briefcase" },
  { emoji: "💻", label: "Laptop" },
  { emoji: "🖥️", label: "Desktop computer" },
  { emoji: "⌨️", label: "Keyboard" },
  { emoji: "🧑‍💻", label: "Technologist" },
  { emoji: "🧠", label: "Brain" },
  { emoji: "🎯", label: "Target" },
  { emoji: "🚀", label: "Rocket" },
  { emoji: "✍️", label: "Writing hand" },
  { emoji: "📝", label: "Memo" },
  { emoji: "📚", label: "Books" },
  { emoji: "🎓", label: "Graduation cap" },
  { emoji: "🔬", label: "Microscope" },
  { emoji: "🧪", label: "Test tube" },
  { emoji: "📊", label: "Bar chart" },
  { emoji: "📈", label: "Chart increasing" },
  { emoji: "🗂️", label: "Card index dividers" },
  { emoji: "📁", label: "Folder" },
  { emoji: "🗓️", label: "Calendar" },
  { emoji: "⏰", label: "Alarm clock" },
  { emoji: "📮", label: "Postbox" },
  { emoji: "💬", label: "Speech balloon" },
  { emoji: "📞", label: "Telephone" },
  { emoji: "🤝", label: "Handshake" },
  { emoji: "🎨", label: "Palette" },
  { emoji: "✏️", label: "Pencil" },
  { emoji: "🎬", label: "Clapper board" },
  { emoji: "📷", label: "Camera" },
  { emoji: "🎧", label: "Headphones" },
  { emoji: "🎵", label: "Musical note" },
  { emoji: "🎙️", label: "Microphone" },
  { emoji: "🎮", label: "Game controller" },
  { emoji: "🛠️", label: "Hammer and wrench" },
  { emoji: "⚙️", label: "Gear" },
  { emoji: "🐛", label: "Bug" },
  { emoji: "🔒", label: "Lock" },
  { emoji: "💡", label: "Light bulb" },
  { emoji: "⚡", label: "High voltage" },
  { emoji: "🔥", label: "Fire" },
  { emoji: "⭐", label: "Star" },
  { emoji: "☕", label: "Hot beverage" },
  { emoji: "🌱", label: "Seedling" },
  { emoji: "🌙", label: "Crescent moon" },
  { emoji: "☀️", label: "Sun" },
  { emoji: "🏠", label: "House" },
  { emoji: "✈️", label: "Airplane" },
  { emoji: "🧘", label: "Person in lotus position" },
  { emoji: "💰", label: "Money bag" },
];

/** Default icon for new workspaces. */
export const DEFAULT_WORKSPACE_EMOJI = "💼";

/** The backend accepts icons of at most 16 Unicode scalar values (`chars().count()`). */
export const MAX_ICON_CODE_POINTS = 16;

/**
 * Turn free text into a workspace icon: the first grapheme cluster, trimmed. Returns `null` for
 * empty/whitespace input or a cluster longer than the backend accepts.
 */
export function normalizeEmojiInput(value: string): string | null {
  const grapheme = firstGrapheme(value.trim());
  if (!grapheme || /^\s+$/u.test(grapheme)) return null;
  if (Array.from(grapheme).length > MAX_ICON_CODE_POINTS) return null;
  return grapheme;
}
