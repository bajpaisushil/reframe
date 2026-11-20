import { useMemo } from "react";
import {
  CircleCheck,
  FileQuestion,
  FolderSearch,
  Info,
  Link2Off,
  Monitor,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";
import { AppIcon, Badge, Button, CapabilityBadge, Icon, IconButton, Skeleton, type LucideIcon } from "@/components/ui";
import { errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import type { ApplicationEntry, CapabilityLevel, EntryHealth, LaunchTarget, WorkspaceHealth } from "@/types";
import { launchFromTarget, launchTargetLabel } from "../model/entries";
import { describeMapping, describeMonitors, estimateTotal, readinessHeadline } from "../model/readiness";
import styles from "./ReadinessPanel.module.css";

export interface ReadinessPanelProps {
  health: WorkspaceHealth | undefined;
  error: unknown;
  isFetching: boolean;
  onRefresh: () => void;
  /** Saved applications (health describes the saved version). */
  applications: readonly ApplicationEntry[];
  /** The apps have unsaved changes the report doesn't reflect yet. */
  stale: boolean;
  onLocate: (entry: ApplicationEntry) => void;
  onUseFound: (entry: ApplicationEntry, target: LaunchTarget) => void;
  /** Entry id being fixed (spinner). */
  fixingId: string | null;
}

const BREAKDOWN: Array<{ level: CapabilityLevel; key: "full" | "partial" | "launchOnly" }> = [
  { level: "full", key: "full" },
  { level: "partial", key: "partial" },
  { level: "launchOnly", key: "launchOnly" },
];

/** Typographic dash for the backend's " - " separators. */
const prose = (text: string) => text.replace(/ - /g, " — ");

/** "Will this restore well here?" — a plain summary first, then anything that needs attention. */
export function ReadinessPanel({
  health,
  error,
  isFetching,
  onRefresh,
  applications,
  stale,
  onLocate,
  onUseFound,
  fixingId,
}: ReadinessPanelProps) {
  const byId = useMemo(() => new Map(applications.map((entry) => [entry.id, entry])), [applications]);

  if (!health) {
    if (error) {
      return (
        <section className={styles.panel} aria-label="Restore readiness">
          <div className={styles.header}>
            <StatusMark tone="warning" icon={TriangleAlert} />
            <div className={styles.headline}>
              <h2 className={styles.title}>Couldn&rsquo;t check this workspace</h2>
              <p className={styles.detail}>{errorMessage(error)}</p>
            </div>
            <Button size="sm" variant="secondary" icon={RefreshCw} onClick={onRefresh}>
              Try again
            </Button>
          </div>
        </section>
      );
    }
    return (
      <section className={styles.panel} aria-busy="true" aria-label="Checking restore readiness">
        <div className={styles.header}>
          <Skeleton width={32} height={32} radius={16} />
          <div className={styles.headline}>
            <Skeleton width={260} height={14} text />
            <Skeleton width={180} height={10} text style={{ marginTop: 8 }} />
          </div>
        </div>
      </section>
    );
  }

  const estimate = health.estimate;
  const total = estimateTotal(estimate);
  const headline = readinessHeadline(estimate);
  const monitors = describeMonitors(health.monitors);
  const entryHealth = health.entries
    .map((h) => ({ health: h, entry: byId.get(h.entryId) }))
    .filter((item): item is { health: EntryHealth; entry: ApplicationEntry } => item.entry !== undefined && item.entry.enabled);
  const unavailable = entryHealth.filter((item) => !item.health.available);
  const otherIssues = entryHealth.filter((item) => item.health.available && item.health.issues.length > 0);
  const hasRows =
    unavailable.length > 0 ||
    otherIssues.length > 0 ||
    monitors !== null ||
    health.dependencyIssues.length > 0 ||
    health.platformNotes.length > 0;
  const headlineIcon = headline.tone === "success" ? CircleCheck : headline.tone === "warning" ? TriangleAlert : Info;

  return (
    <section className={styles.panel} aria-labelledby="readiness-title">
      <div className={styles.header}>
        <StatusMark tone={headline.tone} icon={headlineIcon} />
        <div className={styles.headline}>
          <h2 id="readiness-title" className={styles.title}>
            {total === 0 && applications.length > 0 ? "All apps are turned off" : headline.title}
          </h2>
          {total > 0 ? (
            <div className={styles.breakdown}>
              {BREAKDOWN.filter(({ key }) => estimate[key] > 0).map(({ level, key }) => (
                <span key={key} className={styles.stat}>
                  <span className={styles.statCount}>{estimate[key]}</span>
                  <CapabilityBadge level={level} focusable />
                </span>
              ))}
              {estimate.unavailable > 0 && (
                <span className={styles.stat}>
                  <span className={styles.statCount}>{estimate.unavailable}</span>
                  <Badge tone="warning" icon={TriangleAlert}>
                    Can&rsquo;t start here
                  </Badge>
                </span>
              )}
            </div>
          ) : (
            <p className={styles.detail}>
              {applications.length === 0
                ? "Add apps below and Reframe will check that they can be restored on this computer."
                : "Turn on at least one app below to restore this workspace."}
            </p>
          )}
          {headline.detail && <p className={styles.detail}>{headline.detail}</p>}
        </div>
        <div className={styles.headerSide}>
          {/* Inline, so it never pushes the page around while editing. */}
          {stale && (
            <span className={styles.stale}>
              <Icon icon={Info} size={14} />
              Save to check your changes
            </span>
          )}
          <IconButton
            icon={RefreshCw}
            size="sm"
            aria-label="Check again"
            tooltip
            loading={isFetching}
            onClick={onRefresh}
            className={styles.refresh}
          />
        </div>
      </div>

      {hasRows && (
        <ul className={styles.rows}>
          {unavailable.map(({ entry, health: h }) => {
            const found = h.launchTarget ? launchFromTarget(h.launchTarget) : null;
            const issue = h.issues[0];
            return (
              <li key={entry.id} className={styles.row}>
                <AppIcon lookup={{ appId: entry.appId, executablePath: entry.launch.executable, package: entry.launch.package }} name={entry.displayName} size={20} />
                <div className={styles.rowText}>
                  <p className={styles.rowTitle}>{entry.displayName} can&rsquo;t be started on this computer</p>
                  {issue && <p className={styles.rowDetail}>{prose(issue)}</p>}
                  {h.issues.length > 1 && (
                    <ul className={styles.moreIssues}>
                      {h.issues.slice(1).map((more) => (
                        <li key={more}>{prose(more)}</li>
                      ))}
                    </ul>
                  )}
                </div>
                <div className={styles.rowActions}>
                  {found && h.launchTarget && (
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => onUseFound(entry, h.launchTarget as LaunchTarget)}
                      disabled={fixingId !== null}
                      title={launchTargetLabel(h.launchTarget)}
                    >
                      Use found app
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant={found ? "ghost" : "secondary"}
                    icon={FolderSearch}
                    loading={fixingId === entry.id}
                    disabled={fixingId !== null && fixingId !== entry.id}
                    onClick={() => onLocate(entry)}
                  >
                    Locate…
                  </Button>
                </div>
              </li>
            );
          })}

          {otherIssues.map(({ entry, health: h }) => (
            <li key={entry.id} className={styles.row}>
              <AppIcon lookup={{ appId: entry.appId, executablePath: entry.launch.executable, package: entry.launch.package }} name={entry.displayName} size={20} />
              <div className={styles.rowText}>
                <p className={styles.rowTitle}>{entry.displayName}</p>
                {h.issues.map((issue) => (
                  <p key={issue} className={styles.rowDetail}>
                    {prose(issue)}
                  </p>
                ))}
              </div>
              {h.missingPaths.length > 0 && (
                <Badge tone="warning" icon={FileQuestion} className={styles.rowBadge}>
                  {h.missingPaths.length === 1 ? "1 item missing" : `${h.missingPaths.length} items missing`}
                </Badge>
              )}
            </li>
          ))}

          {monitors && (
            <li className={styles.row}>
              <RowIcon icon={Monitor} tone={monitors.tone} />
              <div className={styles.rowText}>
                <p className={styles.rowDetailStrong}>{monitors.text}</p>
                {health.monitors.some((m) => !m.exact) && (
                  <ul className={styles.mappings}>
                    {health.monitors.map((m) => {
                      const d = describeMapping(m);
                      return (
                        <li key={m.savedId}>
                          <span className="mono">{d.from}</span>
                          <span aria-hidden="true" className={styles.arrow}>
                            →
                          </span>
                          <span className="sr-only"> goes to </span>
                          <span className="mono">{d.to}</span>
                          <span className={styles.mappingNote}>{d.note}</span>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>
            </li>
          )}

          {health.dependencyIssues.map((issue) => (
            <li key={issue} className={styles.row}>
              <RowIcon icon={Link2Off} tone="warning" />
              <div className={styles.rowText}>
                <p className={styles.rowDetailStrong}>{prose(issue)}</p>
                <p className={styles.rowDetail}>Change what these apps start after, below.</p>
              </div>
            </li>
          ))}

          {health.platformNotes.map((note) => (
            <li key={note} className={styles.row}>
              <RowIcon icon={Info} tone="neutral" />
              <div className={styles.rowText}>
                <p className={styles.rowDetail}>{prose(note)}</p>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function StatusMark({ tone, icon }: { tone: "success" | "warning" | "neutral"; icon: LucideIcon }) {
  return (
    <span className={cx(styles.mark, styles[tone])} aria-hidden="true">
      <Icon icon={icon} size={18} />
    </span>
  );
}

function RowIcon({ icon, tone }: { icon: LucideIcon; tone: "warning" | "neutral" }) {
  return (
    <span className={cx(styles.rowIcon, tone === "warning" && styles.rowIconWarning)} aria-hidden="true">
      <Icon icon={icon} size={16} />
    </span>
  );
}
