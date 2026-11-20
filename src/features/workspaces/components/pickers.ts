// Native file and folder pickers (tauri-plugin-dialog). Each resolves with the chosen path(s), or
// null when the user cancelled; a failure to open the picker rejects.

import { open } from "@tauri-apps/plugin-dialog";

export async function pickFolder(title: string, defaultPath?: string | null): Promise<string | null> {
  const path = await open({ title, directory: true, multiple: false, defaultPath: defaultPath ?? undefined });
  return typeof path === "string" ? path : null;
}

export async function pickFile(title: string, defaultPath?: string | null): Promise<string | null> {
  const path = await open({ title, directory: false, multiple: false, defaultPath: defaultPath ?? undefined });
  return typeof path === "string" ? path : null;
}

export async function pickFiles(title: string): Promise<string[]> {
  const paths = await open({ title, directory: false, multiple: true });
  if (Array.isArray(paths)) return paths;
  return typeof paths === "string" ? [paths] : [];
}

/** Folder containing a path (for opening a picker next to the current value). */
export function parentFolder(path: string | null | undefined): string | undefined {
  if (!path) return undefined;
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return index > 0 ? path.slice(0, index) : undefined;
}
