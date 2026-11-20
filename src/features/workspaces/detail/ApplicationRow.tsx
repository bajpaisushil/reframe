import { useEffect, useId, useRef, type CSSProperties, type ReactNode } from "react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ChevronDown, GripVertical, TriangleAlert } from "lucide-react";
import { AppIcon, Badge, CapabilityBadge, Icon, IconButton, Switch, Tooltip } from "@/components/ui";
import { cx } from "@/lib/cx";
import type { ApplicationEntry, EntryHealth } from "@/types";
import type { DependencyLabel } from "../model/dependencies";
import { ordinal, stateLine, unapprovedCommands, windowCountLabel } from "../model/entries";
import { healthSummary } from "../model/readiness";
import styles from "./ApplicationRow.module.css";

export interface ApplicationRowProps {
  entry: ApplicationEntry;
  /** Place in the launch order among apps that are on (null when off). */
  position: number | null;
  /** Why the launch position differs from the row's place in the list, if it does. */
  positionNote?: string;
  health: EntryHealth | undefined;
  /** The entry's saved content differs from the draft (health details would be out of date). */
  stateChanged: boolean;
  dependencies: readonly DependencyLabel[];
  expanded: boolean;
  /** Just added: highlight briefly and bring into view. */
  highlighted?: boolean;
  onToggleExpanded: () => void;
  onToggleEnabled: (enabled: boolean) => void;
  /** The editor, rendered under the row when expanded. */
  children?: ReactNode;
}

/** One application in the launch sequence: drag handle, identity, what it reopens, on/off. */
export function ApplicationRow({
  entry,
  position,
  positionNote,
  health,
  stateChanged,
  dependencies,
  expanded,
  highlighted = false,
  onToggleExpanded,
  onToggleEnabled,
  children,
}: ApplicationRowProps) {
  const editorId = useId();
  const itemRef = useRef<HTMLLIElement | null>(null);
  useEffect(() => {
    if (highlighted) itemRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [highlighted]);
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } = useSortable({
    id: entry.id,
  });
  const style: CSSProperties = { transform: CSS.Translate.toString(transform), transition };
  const name = entry.displayName;
  const running = health?.running ?? false;
  const unavailable = health !== undefined && !health.available;
  const summary = (!stateChanged && healthSummary(health)) || stateLine(entry.state);
  const pendingCommands = unapprovedCommands(entry.state);

  const facts: ReactNode[] = [];
  if (!entry.enabled) {
    facts.push(<span key="off">Off — skipped when restoring</span>);
  } else if (unavailable) {
    facts.push(
      <span key="missing" className={styles.warn}>
        <Icon icon={TriangleAlert} size={13} />
        Can&rsquo;t be started here
      </span>,
    );
  }
  if (summary) facts.push(<span key="summary" className={styles.fact}>{summary}</span>);
  if (entry.windows.length > 0) facts.push(<span key="windows">{windowCountLabel(entry)}</span>);
  if (pendingCommands > 0) {
    facts.push(
      <span key="commands" className={styles.warn}>
        {pendingCommands === 1 ? "1 command needs approval" : `${pendingCommands} commands need approval`}
      </span>,
    );
  }
  if (facts.length === 0 && entry.enabled) facts.push(<span key="launch">Starts the app</span>);

  return (
    <li
      ref={(element) => {
        setNodeRef(element);
        itemRef.current = element;
      }}
      style={style}
      className={cx(
        styles.item,
        expanded && styles.expanded,
        isDragging && styles.dragging,
        !entry.enabled && styles.off,
        highlighted && styles.highlighted,
      )}
    >
      <div className={styles.row}>
        <button
          ref={setActivatorNodeRef}
          type="button"
          className={styles.handle}
          {...attributes}
          {...listeners}
          aria-label={`Reorder ${name}`}
        >
          <Icon icon={GripVertical} size={16} />
        </button>
        {positionNote ? (
          <Tooltip content={positionNote} delay={250}>
            <span className={cx(styles.position, styles.positionAdjusted)} tabIndex={-1}>
              <span aria-hidden="true">{position ?? "–"}</span>
              <span className="sr-only">{positionNote}.</span>
            </span>
          </Tooltip>
        ) : (
          <span className={styles.position}>
            <span aria-hidden="true">{position ?? "–"}</span>
            <span className="sr-only">{position === null ? "Off." : `Starts ${ordinal(position)}.`}</span>
          </span>
        )}
        {running ? (
          <Tooltip content="Open now — Merge and Replace reuse it instead of starting another copy" delay={300}>
            <span className={styles.iconWrap} tabIndex={-1}>
              <AppIcon
                lookup={{ appId: entry.appId, executablePath: entry.launch.executable, package: entry.launch.package }}
                name={name}
                size={28}
              />
              <span className={styles.runningDot} aria-hidden="true" />
            </span>
          </Tooltip>
        ) : (
          <span className={styles.iconWrap}>
            <AppIcon
              lookup={{ appId: entry.appId, executablePath: entry.launch.executable, package: entry.launch.package }}
              name={name}
              size={28}
            />
          </span>
        )}
        {/* A larger click target for the mouse; the chevron button is the accessible control. */}
        <div className={styles.main} onClick={onToggleExpanded}>
          <div className={styles.nameLine}>
            <span className={styles.name}>{name}</span>
            {running && <span className="sr-only">(open now)</span>}
          </div>
          <div className={styles.facts}>
            {facts.map((fact, index) => (
              <span key={index} className={styles.factItem}>
                {fact}
              </span>
            ))}
            {dependencies.map((dependency) =>
              dependency.name === null ? (
                <Badge key={dependency.id} tone="warning" className={styles.chip}>
                  After a removed app
                </Badge>
              ) : (
                <Badge key={dependency.id} tone="neutral" className={styles.chip}>
                  After {dependency.name}
                </Badge>
              ),
            )}
          </div>
        </div>
        <div className={styles.trailing}>
          {health && <CapabilityBadge level={health.capability.level} features={health.capability.features} />}
          <Switch checked={entry.enabled} onChange={onToggleEnabled} aria-label={`Restore ${name}`} />
          <IconButton
            icon={ChevronDown}
            size="sm"
            aria-label={expanded ? `Close ${name} settings` : `Edit ${name}`}
            aria-expanded={expanded}
            aria-controls={expanded ? editorId : undefined}
            className={cx(styles.chevron, expanded && styles.chevronOpen)}
            onClick={onToggleExpanded}
          />
        </div>
      </div>
      {expanded && (
        <div id={editorId} className={styles.editor}>
          {children}
        </div>
      )}
    </li>
  );
}

