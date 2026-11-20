import { useId, useMemo, useState, type MouseEvent, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Eye, EyeOff, Monitor, MoreHorizontal } from "lucide-react";
import {
  AppIcon,
  Badge,
  Button,
  CapabilityBadge,
  Checkbox,
  Icon,
  IconButton,
  Menu,
  SectionHeader,
  Skeleton,
  Switch,
  Tooltip,
  type MenuEntry,
} from "@/components/ui";
import { cx } from "@/lib/cx";
import { formatCount } from "@/lib/format";
import type { DiscoveredApplication, MonitorInfo, WindowInfo } from "@/types";
import {
  WINDOW_STATE_LABEL,
  detailText,
  groupByCategory,
  iconLookup,
  monitorNames,
  windowCountLabel,
  windowSize,
  windowTitle,
} from "./describe";
import type { HoverTarget } from "./MonitorMap";
import { checkState, resolveSelection } from "./selection";
import type { AppSelection } from "./useAppSelection";
import type { ExclusionActions } from "./useExclusionActions";
import styles from "./AppList.module.css";

export interface AppListProps {
  apps: readonly DiscoveredApplication[];
  monitors: readonly MonitorInfo[];
  selection: AppSelection;
  /** Shared hover state with the mini-map (Explore). */
  hovered?: HoverTarget | null;
  onHover?: (target: HoverTarget | null) => void;
  /** "Always exclude" / "Stop excluding" in each row's menu (Explore only). */
  exclusions?: ExclusionActions;
  /** `dialog` is denser and uses smaller headings (app picker). */
  variant?: "page" | "dialog";
  /** Extra controls in the Applications header, before "Select all" (e.g. a rescan button). */
  toolbar?: ReactNode;
}

/** Shown in the details line at most; the rest are listed in a tooltip. */
const VISIBLE_DETAILS = 3;

/** Clicks on these inside a row are theirs, not a row toggle. */
const CONTROLS = "button, a, input, label, select, textarea, [role='menuitem'], [tabindex]";

function fromControl(event: MouseEvent<HTMLElement>): boolean {
  const target = event.target;
  if (!(target instanceof Element)) return false;
  const control = target.closest(CONTROLS);
  return control !== null && control !== event.currentTarget && event.currentTarget.contains(control);
}

/**
 * The applications of a desktop snapshot, grouped as Applications, Utilities (collapsed) and
 * System (behind a toggle), with checkboxes for apps and — when expanded — their windows.
 */
export function AppList({
  apps,
  monitors,
  selection,
  hovered = null,
  onHover,
  exclusions,
  variant = "page",
  toolbar,
}: AppListProps) {
  const [utilitiesOpen, setUtilitiesOpen] = useState(false);
  const [showSystem, setShowSystem] = useState(false);
  const groups = useMemo(() => groupByCategory(apps), [apps]);
  const names = useMemo(() => monitorNames(monitors), [monitors]);

  const rowProps = { selection, hovered, onHover, exclusions, monitorNames: names, variant };
  const systemCount = groups.system.length;

  return (
    <div className={cx(styles.list, styles[variant])}>
      <AppSection
        title="Applications"
        description={
          variant === "page" ? "Checked apps are saved with their windows. Expand an app to choose specific windows." : undefined
        }
        apps={groups.application}
        bulkAction
        toolbar={toolbar}
        emptyText={
          apps.length > 0
            ? "Everything open right now is a utility or part of the system."
            : "Nothing with a window is open right now."
        }
        {...rowProps}
      />

      {groups.utility.length > 0 && (
        <AppSection
          title="Utilities"
          description="Small helpers, prompts and tool windows. Not suggested, but you can include them."
          apps={groups.utility}
          collapsible
          open={utilitiesOpen}
          onOpenChange={setUtilitiesOpen}
          {...rowProps}
        />
      )}

      {showSystem && systemCount > 0 && (
        <AppSection
          title="System"
          description="Parts of your desktop itself, like panels and launchers. Rarely worth saving."
          apps={groups.system}
          {...rowProps}
        />
      )}

      {systemCount > 0 && (
        <Switch
          className={styles.systemToggle}
          checked={showSystem}
          onChange={setShowSystem}
          label={`Show system items (${formatCount(systemCount)})`}
        />
      )}
    </div>
  );
}

interface RowSharedProps {
  selection: AppSelection;
  hovered: HoverTarget | null;
  onHover?: (target: HoverTarget | null) => void;
  exclusions?: ExclusionActions;
  monitorNames: Map<string, string>;
  variant: "page" | "dialog";
}

interface AppSectionProps extends RowSharedProps {
  title: string;
  description?: string;
  apps: readonly DiscoveredApplication[];
  /** "Select all" / "Deselect all" in the header. */
  bulkAction?: boolean;
  toolbar?: ReactNode;
  emptyText?: string;
  collapsible?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

function AppSection({
  title,
  description,
  apps,
  bulkAction = false,
  toolbar,
  emptyText,
  collapsible = false,
  open = true,
  onOpenChange,
  ...rowProps
}: AppSectionProps) {
  const headingId = useId();
  const listId = useId();
  const { selection, variant } = rowProps;
  const Heading = variant === "page" ? "h2" : "h3";

  const selectedCount = apps.filter((app) => checkState(app, selection.choices) !== "unchecked").length;
  // Excluded apps only join a bulk selection once included one by one.
  const selectable = apps.filter((app) => !app.excluded);
  const allSelected = selectable.length > 0 && selectable.every((app) => checkState(app, selection.choices) !== "unchecked");

  let bulk: ReactNode;
  if (bulkAction && selectable.length > 1) {
    bulk = (
      <Button
        variant="ghost"
        size="sm"
        className={styles.bulkAction}
        onClick={() => selection.setApps(allSelected ? apps : selectable, !allSelected)}
      >
        {allSelected ? "Deselect all" : "Select all"}
      </Button>
    );
  }
  const action =
    toolbar || bulk ? (
      <>
        {toolbar}
        {bulk}
      </>
    ) : undefined;

  const rows =
    apps.length > 0 ? (
      <ul id={listId} className={styles.rows} aria-labelledby={headingId}>
        {apps.map((app) => (
          <AppRow key={app.key} app={app} {...rowProps} />
        ))}
      </ul>
    ) : (
      emptyText && <p className={styles.sectionEmpty}>{emptyText}</p>
    );

  if (collapsible) {
    return (
      <section className={styles.section}>
        <Heading className={styles.collapsibleHeading}>
          <button
            type="button"
            id={headingId}
            className={styles.collapseButton}
            aria-expanded={open}
            aria-controls={open ? listId : undefined}
            onClick={() => onOpenChange?.(!open)}
          >
            <Icon icon={ChevronRight} size={16} className={styles.collapseChevron} />
            <span>{title}</span>
            <span className={styles.headingCount}>{formatCount(apps.length)}</span>
            {selectedCount > 0 && <span className={styles.headingNote}>{formatCount(selectedCount)} selected</span>}
          </button>
        </Heading>
        {open && (
          <div className={styles.collapsibleBody}>
            {description && <p className={styles.sectionDescription}>{description}</p>}
            {rows}
          </div>
        )}
      </section>
    );
  }

  return (
    <section className={styles.section}>
      <SectionHeader
        id={headingId}
        as={Heading}
        title={title}
        count={apps.length}
        description={description}
        action={action}
        className={styles.sectionHeader}
      />
      {rows}
    </section>
  );
}

interface AppRowProps extends RowSharedProps {
  app: DiscoveredApplication;
}

function AppRow({ app, selection, hovered, onHover, exclusions, monitorNames: names }: AppRowProps) {
  const [expanded, setExpanded] = useState(false);
  const windowsId = useId();
  const state = checkState(app, selection.choices);
  const resolved = resolveSelection(app, selection.choices);
  const selectedIds = new Set(resolved?.windowIds ?? []);
  // An excluded app stays out of the capture until it is included explicitly.
  const locked = app.excluded && state === "unchecked";
  const multiWindow = app.windows.length > 1;
  const pending = exclusions?.pendingKey === app.key;
  const name = app.displayName;

  const toggleApp = () => selection.setApp(app, state !== "checked");
  const onRowClick = (event: MouseEvent<HTMLDivElement>) => {
    if (locked || fromControl(event)) return;
    toggleApp();
  };

  const details = app.details.map(detailText);
  // Without adapter details, the frontmost window's title says what's open — unless it only
  // repeats the app's name ("xclock").
  const rawTitle = app.windows[0]?.title.trim() ?? "";
  const frontTitle = rawTitle && rawTitle.toLowerCase() !== name.trim().toLowerCase() ? windowTitle(rawTitle) : "";

  const menu: MenuEntry[] = app.excluded
    ? [
        {
          label: "Stop excluding",
          icon: Eye,
          description: "Suggest it for capture again",
          onSelect: () => void exclusions?.stopExcluding(app),
        },
      ]
    : [
        {
          label: "Always exclude",
          icon: EyeOff,
          description: "Never suggest it for capture",
          onSelect: () => void exclusions?.exclude(app),
        },
      ];

  return (
    <li
      className={styles.row}
      data-highlighted={hovered?.appKey === app.key || undefined}
      data-locked={locked || undefined}
      data-expanded={expanded || undefined}
      onMouseEnter={onHover ? () => onHover({ appKey: app.key }) : undefined}
      onMouseLeave={onHover ? () => onHover(null) : undefined}
    >
      <div className={styles.main} onClick={onRowClick}>
        <Checkbox
          checked={state === "checked"}
          indeterminate={state === "indeterminate"}
          onChange={toggleApp}
          disabled={locked}
          aria-label={locked ? `${name} (excluded)` : `Include ${name}`}
        />
        <AppIcon lookup={iconLookup(app)} name={name} size={28} className={styles.appIcon} />
        <div className={styles.text}>
          <div className={styles.titleLine}>
            <span className={styles.name}>{name}</span>
            <CapabilityBadge level={app.capability.level} features={app.capability.features} />
            {app.excluded && (
              <Tooltip
                content="You chose never to suggest this app. Include it to save it this time."
                describe
              >
                <Badge icon={EyeOff} tabIndex={0}>
                  Excluded
                </Badge>
              </Tooltip>
            )}
          </div>
          {details.length > 0 ? (
            <div className={styles.meta}>
              {details.slice(0, VISIBLE_DETAILS).map((detail, index) => (
                <span key={`${index}-${detail}`} className={styles.detail}>
                  {detail}
                </span>
              ))}
              {details.length > VISIBLE_DETAILS && (
                <Tooltip
                  content={
                    <span className={styles.tooltipLines}>
                      {details.slice(VISIBLE_DETAILS).map((detail, index) => (
                        <span key={`${index}-${detail}`}>{detail}</span>
                      ))}
                    </span>
                  }
                  describe
                >
                  <span className={styles.moreDetails} tabIndex={0}>
                    +{details.length - VISIBLE_DETAILS} more
                  </span>
                </Tooltip>
              )}
            </div>
          ) : (
            frontTitle && <div className={cx(styles.meta, styles.metaQuiet)}>{frontTitle}</div>
          )}
        </div>
        <div className={styles.end}>
          {locked && (
            <Button size="sm" variant="secondary" onClick={() => selection.setApp(app, true)}>
              Include
            </Button>
          )}
          <button
            type="button"
            className={styles.windowsToggle}
            aria-expanded={expanded}
            aria-controls={expanded ? windowsId : undefined}
            aria-label={`${windowCountLabel(app.windows.length, resolved && !resolved.all ? resolved.windowIds.length : null)} — ${expanded ? "hide" : "show"} windows of ${name}`}
            onClick={() => setExpanded((value) => !value)}
          >
            <span aria-hidden="true">
              {windowCountLabel(app.windows.length, resolved && !resolved.all ? resolved.windowIds.length : null)}
            </span>
            <Icon icon={ChevronDown} size={14} className={styles.chevron} />
          </button>
          {exclusions && (
            <Menu
              aria-label={`Actions for ${name}`}
              minWidth={220}
              items={menu}
              trigger={(props) => (
                <IconButton
                  {...props}
                  icon={MoreHorizontal}
                  size="sm"
                  aria-label={`More actions for ${name}`}
                  loading={pending}
                  className={styles.menuButton}
                />
              )}
            />
          )}
        </div>
      </div>

      {expanded && (
        <ul id={windowsId} className={styles.windows} aria-label={`Windows of ${name}`}>
          {app.windows.map((window) => (
            <WindowRow
              key={window.id}
              window={window}
              monitorName={window.monitorId ? names.get(window.monitorId) : undefined}
              selectable={multiWindow}
              selected={selectedIds.has(window.id)}
              disabled={locked}
              highlighted={hovered?.windowId === window.id}
              onChange={(checked) => selection.setWindow(app, window.id, checked)}
              onHover={onHover ? (over) => onHover(over ? { appKey: app.key, windowId: window.id } : { appKey: app.key }) : undefined}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

interface WindowRowProps {
  window: WindowInfo;
  monitorName: string | undefined;
  /** Per-window checkboxes only make sense for apps with several windows. */
  selectable: boolean;
  selected: boolean;
  disabled: boolean;
  highlighted: boolean;
  onChange: (checked: boolean) => void;
  onHover?: (over: boolean) => void;
}

function WindowRow({ window, monitorName, selectable, selected, disabled, highlighted, onChange, onHover }: WindowRowProps) {
  const title = windowTitle(window.title);
  const stateLabel = WINDOW_STATE_LABEL[window.state];
  const interactive = selectable && !disabled;
  return (
    <li
      className={styles.windowRow}
      data-highlighted={highlighted || undefined}
      data-interactive={interactive || undefined}
      onMouseEnter={onHover ? () => onHover(true) : undefined}
      onMouseLeave={onHover ? () => onHover(false) : undefined}
      onClick={(event) => {
        if (!interactive || fromControl(event)) return;
        onChange(!selected);
      }}
    >
      {selectable ? (
        <Checkbox checked={selected} onChange={onChange} disabled={disabled} aria-label={`Include window: ${title}`} />
      ) : (
        <span className={styles.windowBullet} aria-hidden="true" />
      )}
      <span className={styles.windowTitle} title={title}>
        {title}
      </span>
      <span className={styles.windowMeta}>
        {stateLabel && <Badge>{stateLabel}</Badge>}
        {monitorName && (
          <span className={styles.monitorName}>
            <Icon icon={Monitor} size={12} />
            {monitorName}
          </span>
        )}
        <span className={styles.size}>{windowSize(window.bounds)}</span>
      </span>
    </li>
  );
}

/** Placeholder rows while the first scan runs. */
export function AppListSkeleton({ rows = 5, variant = "page" }: { rows?: number; variant?: "page" | "dialog" }) {
  return (
    <div className={cx(styles.list, styles[variant])} aria-hidden="true">
      <section className={styles.section}>
        <div className={styles.skeletonHeader}>
          <Skeleton width={120} height={16} text />
          {variant === "page" && <Skeleton width={340} height={12} text />}
        </div>
        <ul className={styles.rows}>
          {Array.from({ length: rows }, (_, index) => (
            <li key={index} className={styles.row}>
              <div className={styles.main}>
                <Skeleton width={16} height={16} radius={4} />
                <Skeleton width={28} height={28} radius={7} />
                <div className={styles.text}>
                  <Skeleton width={[140, 110, 170, 96, 128][index % 5]} height={13} text />
                  <Skeleton width={[220, 160, 260, 140, 200][index % 5]} height={11} text />
                </div>
                <Skeleton width={64} height={12} text />
              </div>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
