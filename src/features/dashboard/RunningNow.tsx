import { useState } from "react";
import { Info, MonitorOff, RefreshCw } from "lucide-react";
import {
  AppIconStack,
  Button,
  EmptyState,
  Icon,
  IconButton,
  RelativeTime,
  SectionHeader,
  SectionLink,
  Skeleton,
} from "@/components/ui";
import { useDesktopSnapshot } from "@/hooks/useDesktopSnapshot";
import { errorMessage } from "@/lib/api";
import { formatCount } from "@/lib/format";
import type { DesktopSnapshot } from "@/types";
import styles from "./RunningNow.module.css";

const MAX_ICONS = 8;
const VISIBLE_NOTES = 2;

/** What's open right now (a desktop scan), with a link to Explore. */
export function RunningNow() {
  const snapshot = useDesktopSnapshot();
  const scanning = snapshot.isFetching;

  return (
    <section aria-labelledby="dashboard-running" className={styles.section}>
      <SectionHeader
        id="dashboard-running"
        title="Running now"
        action={
          <>
            <IconButton
              icon={RefreshCw}
              aria-label="Scan again"
              tooltip
              size="sm"
              loading={scanning}
              onClick={() => void snapshot.refresh()}
            />
            <SectionLink to={{ name: "explore" }}>Explore</SectionLink>
          </>
        }
      />
      <div className={styles.panel} aria-busy={scanning || undefined}>
        {snapshot.data ? (
          <SnapshotSummary snapshot={snapshot.data} />
        ) : snapshot.error ? (
          <EmptyState
            className={styles.centered}
            size="compact"
            tone="danger"
            icon={MonitorOff}
            title="Couldn't look at your desktop"
            text={errorMessage(snapshot.error)}
            action={
              <Button size="sm" variant="secondary" icon={RefreshCw} onClick={() => void snapshot.refresh()}>
                Try again
              </Button>
            }
          />
        ) : (
          <SummarySkeleton />
        )}
      </div>
    </section>
  );
}

function SnapshotSummary({ snapshot }: { snapshot: DesktopSnapshot }) {
  const [showAllNotes, setShowAllNotes] = useState(false);
  const apps = snapshot.applications;
  const listing = snapshot.platform.capabilities.listWindows;
  const cannotList = listing.level === "unsupported";
  // When the limitation is explained above, don't repeat it in every note.
  const reason = cannotList ? listing.note : null;
  const notes = reason ? snapshot.warnings.filter((note) => !note.includes(reason)) : snapshot.warnings;
  const visibleNotes = showAllNotes ? notes : notes.slice(0, VISIBLE_NOTES);
  const icons = apps.map((app) => ({
    lookup: { appId: app.appId, executablePath: app.executablePath, package: app.package },
    name: app.displayName,
  }));

  return (
    <div className={styles.summary}>
      {cannotList ? (
        // "0 apps" would be misleading: Reframe cannot see windows on this system at all.
        <div className={styles.unsupported}>
          <span className={styles.unsupportedIcon} aria-hidden="true">
            <Icon icon={MonitorOff} size={18} />
          </span>
          <span className={styles.unsupportedText}>
            <span className={styles.unsupportedTitle}>Reframe can&rsquo;t see app windows here</span>
            <span className={styles.unsupportedNote}>
              {listing.note ?? "This computer doesn't let Reframe list the windows that are open."}
            </span>
          </span>
        </div>
      ) : (
        <>
          <div className={styles.headline}>
            <span className={styles.count}>{formatCount(apps.length)}</span>
            <span className={styles.label}>
              {apps.length === 1 ? "app with open windows" : "apps with open windows"}
              <RelativeTime className={styles.scanned} value={snapshot.capturedAt} prefix="Checked " />
            </span>
          </div>

          {apps.length > 0 ? (
            <AppIconStack apps={icons} max={MAX_ICONS} size={28} showNames />
          ) : (
            <p className={styles.none}>
              Reframe didn&rsquo;t find any app windows. Open the apps you work with, then scan again.
            </p>
          )}
        </>
      )}

      {notes.length > 0 && (
        <ul className={styles.notes} aria-label="Notes about this scan">
          {visibleNotes.map((note, index) => (
            <li key={`${index}-${note}`} className={styles.note}>
              <Icon icon={Info} size={14} className={styles.noteIcon} />
              <span>{note}</span>
            </li>
          ))}
          {notes.length > VISIBLE_NOTES && (
            <li>
              <button type="button" className={styles.more} onClick={() => setShowAllNotes((v) => !v)}>
                {showAllNotes ? "Show fewer notes" : `Show ${notes.length - VISIBLE_NOTES} more`}
              </button>
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

function SummarySkeleton() {
  return (
    <div className={styles.summary} aria-label="Looking at your desktop…" role="status">
      <div className={styles.headline}>
        <Skeleton width={34} height={30} radius={6} />
        <Skeleton width={150} height={12} text />
      </div>
      <div className={styles.skeletonIcons}>
        {Array.from({ length: 5 }, (_, i) => (
          <Skeleton key={i} width={28} height={28} radius={7} />
        ))}
      </div>
    </div>
  );
}
