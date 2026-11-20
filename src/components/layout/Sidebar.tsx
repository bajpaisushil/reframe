import { Activity, Compass, House, LayoutGrid, Monitor, Moon, Settings, Sun } from "lucide-react";
import { Icon, type LucideIcon } from "@/components/ui/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Menu } from "@/components/ui/Menu";
import { Skeleton } from "@/components/ui/Skeleton";
import { Spinner } from "@/components/ui/Spinner";
import { useToast } from "@/components/ui/Toast";
import { WorkspaceIcon } from "@/components/ui/WorkspaceIcon";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useAppVersion } from "@/hooks/usePlatformInfo";
import { useActiveWorkspace } from "@/hooks/useWorkspaces";
import { errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { Link, navigate, navSection, useRoute, type NavSection, type Route } from "@/lib/router";
import type { ThemePreference } from "@/types";
import { Logo } from "./Logo";
import { useTheme } from "./ThemeProvider";
import styles from "./Sidebar.module.css";

interface NavItem {
  section: NavSection;
  label: string;
  icon: LucideIcon;
  route: Route;
}

/** Sidebar destinations, in order (also used for the ⌘/Ctrl+1…5 shortcuts). */
export const NAV_ITEMS: readonly NavItem[] = [
  { section: "home", label: "Home", icon: House, route: { name: "home" } },
  { section: "explore", label: "Explore", icon: Compass, route: { name: "explore" } },
  { section: "workspaces", label: "Workspaces", icon: LayoutGrid, route: { name: "workspaces" } },
  { section: "activity", label: "Activity", icon: Activity, route: { name: "activity" } },
  { section: "settings", label: "Settings", icon: Settings, route: { name: "settings" } },
];

const THEME_ICONS: Record<ThemePreference, LucideIcon> = { system: Monitor, light: Sun, dark: Moon };
const THEME_LABELS: Record<ThemePreference, string> = { system: "Match system", light: "Light", dark: "Dark" };

export function Sidebar() {
  const { route } = useRoute();
  const current = navSection(route);

  return (
    <aside className={styles.sidebar}>
      <div className={styles.brand}>
        <Logo size={22} />
        <span className={styles.wordmark}>Reframe</span>
      </div>

      <nav aria-label="Main" className={styles.nav}>
        <ul className={styles.navList}>
          {NAV_ITEMS.map((item) => {
            const active = item.section === current;
            return (
              <li key={item.section}>
                <Link
                  to={item.route}
                  className={cx(styles.navItem, active && styles.active)}
                  aria-current={active ? "page" : undefined}
                >
                  <Icon icon={item.icon} size={18} className={styles.navIcon} />
                  {item.label}
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>

      <div className={styles.bottom}>
        <WorkspaceStatus />
        <SidebarFooter />
      </div>
    </aside>
  );
}

/** The active workspace — or the restore in progress, which matters more. */
function WorkspaceStatus() {
  const { running } = useActiveRuns();
  const { workspace, isLoading } = useActiveWorkspace();
  const run = running[0];

  if (run) {
    return (
      <button type="button" className={styles.pill} onClick={() => navigate({ name: "restore", runId: run.id })}>
        <span className={styles.pillIcon}>
          <Spinner size={16} />
        </span>
        <span className={styles.pillText}>
          <span className={styles.pillName}>{run.workspaceName}</span>
          <span className={styles.pillCaption}>Restoring…</span>
        </span>
      </button>
    );
  }

  if (isLoading) {
    return (
      <div className={cx(styles.pill, styles.pillStatic)} aria-hidden="true">
        <Skeleton width={28} height={28} radius={7} />
        <span className={styles.pillText}>
          <Skeleton width={96} height={10} text />
          <Skeleton width={64} height={8} text style={{ marginTop: 6 }} />
        </span>
      </div>
    );
  }

  if (!workspace) {
    return (
      <div className={cx(styles.pill, styles.pillStatic, styles.pillEmpty)}>
        <span className={styles.pillDot} aria-hidden="true" />
        <span className={styles.pillText}>
          <span className={styles.pillName}>No active workspace</span>
        </span>
      </div>
    );
  }

  return (
    <button
      type="button"
      className={styles.pill}
      onClick={() => navigate({ name: "workspace", id: workspace.id })}
      aria-label={`Active workspace: ${workspace.name}`}
    >
      <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={28} />
      <span className={styles.pillText}>
        <span className={styles.pillName}>{workspace.name}</span>
        <span className={styles.pillCaption}>Active workspace</span>
      </span>
    </button>
  );
}

function SidebarFooter() {
  const version = useAppVersion();
  const { preference, setTheme } = useTheme();
  const toast = useToast();

  const choose = (next: ThemePreference) => {
    setTheme(next).catch((error: unknown) => toast.error("Couldn't save the theme", errorMessage(error)));
  };

  return (
    <div className={styles.footer}>
      <span className={styles.version}>{version.data ? `Version ${version.data}` : " "}</span>
      <Menu
        aria-label="Appearance"
        placement="top-end"
        items={[
          { type: "label", label: "Appearance" },
          ...(["system", "light", "dark"] as const).map((option) => ({
            label: THEME_LABELS[option],
            checked: preference === option,
            onSelect: () => choose(option),
          })),
        ]}
        trigger={(props) => (
          <IconButton
            {...props}
            icon={THEME_ICONS[preference]}
            aria-label={`Appearance: ${THEME_LABELS[preference]}`}
            size="sm"
            tooltip="Appearance"
            tooltipPlacement="top"
          />
        )}
      />
    </div>
  );
}
