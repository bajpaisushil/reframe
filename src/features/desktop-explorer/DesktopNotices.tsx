import { useState } from "react";
import { Info, ShieldAlert } from "lucide-react";
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

/** One calm callout per missing permission (macOS), with a "Grant access" button. */
export function PermissionNotices({ missing, onGrant, granting }: PermissionNoticesProps) {
  if (missing.length === 0) return null;
  return (
    <div className={styles.stack}>
      {missing.map((permission) => (
        <div key={permission.kind} className={cx(styles.callout, styles.warning)} role="status">
          <Icon icon={ShieldAlert} size={18} className={styles.icon} />
          <div className={styles.copy}>
            <p className={styles.title}>Reframe needs {PERMISSION_NAMES[permission.kind]} access</p>
            <p className={styles.text}>{permission.purpose}</p>
            {permission.instructions && <p className={styles.hint}>{permission.instructions}</p>}
          </div>
          <Button
            size="sm"
            variant="secondary"
            className={styles.action}
            loading={granting === permission.kind}
            onClick={() => onGrant(permission.kind)}
          >
            Grant access
          </Button>
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
