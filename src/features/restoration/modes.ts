// Switch modes in plain words, shared by the switch dialog and Settings.

import type { SwitchMode } from "@/types";

export interface ModeCopy {
  value: SwitchMode;
  label: string;
  /** One line explaining what the mode does. */
  summary: string;
}

/** In the order they are offered (Replace / Merge / Additive). */
export const SWITCH_MODES: readonly ModeCopy[] = [
  {
    value: "replace",
    label: "Replace",
    summary: "Asks the apps you choose to close, keeps shared ones and opens what's missing.",
  },
  {
    value: "merge",
    label: "Merge",
    summary: "Keeps everything open, opens what's missing and arranges the windows.",
  },
  {
    value: "additive",
    label: "Additive",
    summary: "Opens every app again, even the ones that are already running.",
  },
];

export const MODE_LABEL: Record<SwitchMode, string> = {
  replace: "Replace",
  merge: "Merge",
  additive: "Additive",
};

export function modeCopy(mode: SwitchMode): ModeCopy {
  return SWITCH_MODES.find((m) => m.value === mode) ?? SWITCH_MODES[1];
}

export function isSwitchMode(value: unknown): value is SwitchMode {
  return value === "replace" || value === "merge" || value === "additive";
}
