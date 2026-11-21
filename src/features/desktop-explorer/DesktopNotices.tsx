import { useState } from "react";
import { Info, ShieldCheck } from "lucide-react";
import { Button, Icon } from "@/components/ui";
import { cx } from "@/lib/cx";
import type { PermissionKind, PermissionStatus } from "@/types";
import { PERMISSION_NAMES } from "./notices";
import styles from "./DesktopNotices.module.css";

const VISIBLE_NOTES = 2;

export interface PermissionNoticesProps {
  /** Permissions that are not granted yet. */
  missing: readonly PermissionStatus[];
  onGrant: (kind: PermissionKind) => void;
  granting: PermissionKind | null;
}

/** Permissions the user chose "Not now" for; hidden until Reframe restarts. */
const DISMISSED_KEY = "reframe.permissionNotices.dismissed";

function readDismissed(): PermissionKind[] {
  try {
    const parsed: unknown = JSON.parse(sessionStorage.getItem(DISMISSED_KEY) ?? "[]");
    return Array.isArray(parsed) ? (parsed as PermissionKind[]) : [];
  } catch {
    return [];
  }
}

function writeDismissed(kinds: readonly PermissionKind[]) {
  try {
    sessionStorage.setItem(DISMISSED_KEY, JSON.stringify(kinds));
  } catch {
    // Storage unavailable: the notice simply stays hidden until the page reloads.
  }
}

/**
 * One calm callout per missing permission (macOS). Permissions are optional — Reframe reopens
 * apps without them — so each callout can be put off with "Not now".
 */
export function PermissionNotices({ missing, onGrant, granting }: PermissionNoticesProps) {
  const [dismissed, setDismissed] = useState<PermissionKind[]>(readDismissed);
  const visible = missing.filter((permission) => !dismissed.includes(permission.kind));
  if (visible.length === 0) return null;
  const dismiss = (kind: PermissionKind) => {
    const next = [...dismissed, kind];
    setDismissed(next);
    writeDismissed(next);
  };
  return (
    <div className={styles.stack}>
      {visible.map((permission) => (
        <div key={permission.kind} className={cx(styles.callout, styles.info)} role="status">
          <Icon icon={ShieldCheck} size={18} className={styles.icon} />
          <div className={styles.copy}>
            <p className={styles.title}>Optional: allow {PERMISSION_NAMES[permission.kind]} access</p>
            <p className={styles.text}>{permission.purpose}</p>
            {permission.instructions && <p className={styles.hint}>{permission.instructions}</p>}
          </div>
          <div className={styles.actions}>
            <Button size="sm" variant="ghost" onClick={() => dismiss(permission.kind)}>
              Not now
            </Button>
            <Button
              size="sm"
              variant="secondary"
              loading={granting === permission.kind}
              onClick={() => onGrant(permission.kind)}
            >
              Allow
            </Button>
          </div>
        </div>
      ))}
    </div>
  );
}

/** Plain-language notes about what this scan couldn't see. */
export function ScanNotes({ notes }: { notes: readonly string[] }) {
  const [expanded, setExpanded] = useState(false);
  if (notes.length === 0) return null;
  const visible = expanded ? notes : notes.slice(0, VISIBLE_NOTES);
  return (
    <div className={cx(styles.callout, styles.info)}>
      <Icon icon={Info} size={16} className={styles.icon} />
      <div className={styles.copy}>
        <ul className={styles.notes} aria-label="Notes about this scan">
          {visible.map((note, index) => (
            <li key={`${index}-${note}`}>{note}</li>
          ))}
        </ul>
        {notes.length > VISIBLE_NOTES && (
          <button type="button" className={styles.more} onClick={() => setExpanded((value) => !value)}>
            {expanded ? "Show fewer notes" : `Show ${notes.length - VISIBLE_NOTES} more`}
          </button>
        )}
      </div>
    </div>
  );
}
