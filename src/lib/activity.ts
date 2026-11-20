// Pure helpers for activity entries (tested in activity.test.ts).

export interface MessageParts {
  /** Text before the workspace name. */
  before: string;
  /** Text after the workspace name. */
  after: string;
}

/**
 * Split an activity message around the workspace name ("Created Deep work" -> "Created " + name),
 * so the name can be shown once, inline and emphasised. Returns `null` when the message does not
 * mention the workspace ("Captured 3 applications"); the name is then shown on its own line.
 */
export function splitActivityMessage(message: string, workspaceName: string): MessageParts | null {
  const name = workspaceName.trim();
  if (!name) return null;
  const index = message.indexOf(name);
  if (index === -1) return null;
  return { before: message.slice(0, index), after: message.slice(index + name.length) };
}
