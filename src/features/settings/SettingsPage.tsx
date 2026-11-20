import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import {
  CircleCheck,
  CircleExclamationPoint,
  CircleMinus,
  Monitor,
  Moon,
  Plus,
  RefreshCw,
  Settings as SettingsIcon,
  Sun,
  X,
} from "lucide-react";
import { SHELL_SHORTCUTS } from "@/components/layout/AppShell";
import { Logo } from "@/components/layout/Logo";
import { Page, PageHeader } from "@/components/layout/Page";
import { useTheme } from "@/components/layout/ThemeProvider";
import {
  Badge,
  Button,
  EmptyState,
  IconButton,
  Input,
  Kbd,
  SegmentedControl,
  Select,
  Skeleton,
  Switch,
  useToast,
  type SelectOption,
} from "@/components/ui";
import { useAppVersion, usePlatformInfo } from "@/hooks/usePlatformInfo";
import { useQuery } from "@/hooks/useQuery";
import { useSettings, type SettingsPatch } from "@/hooks/useSettings";
import { useWorkspaces } from "@/hooks/useWorkspaces";
import { api, errorMessage, toApiError } from "@/lib/api";
import { primaryModifierLabel } from "@/lib/platform";
import { MODE_LABEL, SWITCH_MODES, modeCopy } from "@/features/restoration/modes";
import { Notice } from "@/features/restoration/Notice";
import type { AppSettings, PermissionStatus, StartupAction, SupportLevel, ThemePreference } from "@/types";
import { RadioGroup, type RadioOption } from "./RadioGroup";
import { SettingsRow, SettingsSection } from "./SettingsSection";
import {
  PERMISSION_LABEL,
  addExcludedApp,
  capabilityRows,
  capabilitySummary,
  parseCloseWait,
  removeExcludedApp,
  startupRestoreNeedsLogin,
  startupWorkspaceMissing,
  systemDescription,
  withStartupAction,
  withStartupWorkspace,
} from "./settingsModel";
import styles from "./SettingsPage.module.css";

/** Saves a change optimistically; on failure the settings reload and the reason is shown. */
type Save = (patch: SettingsPatch) => Promise<boolean>;

function useSave(): Save {
  const { update } = useSettings();
  const toast = useToast();
  return useCallback(
    (patch: SettingsPatch) =>
      update(patch).then(
        () => true,
        (error: unknown) => {
          const e = toApiError(error);
          toast.error("Couldn't save that change", e.details.length > 0 ? e.details.join(" ") : e.message);
          return false;
        },
      ),
    [update, toast],
  );
}

/** App settings (route `settings`). Every change is saved as soon as it is made. */
export function SettingsPage() {
  const { settings, error, refresh } = useSettings();
  return (
    <Page>
      <PageHeader title="Settings" subtitle="Startup, restoring and what Reframe captures. Changes are saved as you make them." />
      {settings ? (
        <SettingsContent settings={settings} />
      ) : error ? (
        <EmptyState
          icon={SettingsIcon}
          tone="danger"
          title="Couldn't load your settings"
          text={errorMessage(error)}
          action={
            <Button variant="secondary" icon={RefreshCw} onClick={() => void refresh()}>
              Try again
            </Button>
          }
        />
      ) : (
        <SettingsSkeleton />
      )}
    </Page>
  );
}

function SettingsContent({ settings }: { settings: AppSettings }) {
  const save = useSave();
  return (
    <div className={styles.content}>
      <AppearanceSection />
      <StartupSection settings={settings} save={save} />
      <RestoringSection settings={settings} save={save} />
      <BrowserSection settings={settings} save={save} />
      <ExcludedAppsSection settings={settings} save={save} />
      <PermissionsSection />
      <ThisComputerSection />
      <ShortcutsSection />
      <AboutSection />
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------------------------

const THEME_OPTIONS = [
  { value: "system" as const, label: "System", icon: Monitor },
  { value: "light" as const, label: "Light", icon: Sun },
  { value: "dark" as const, label: "Dark", icon: Moon },
];

const THEME_DESCRIPTION: Record<ThemePreference, string> = {
  system: "Follows your computer's light or dark appearance.",
  light: "Always light.",
  dark: "Always dark.",
};

function AppearanceSection() {
  const { preference, setTheme } = useTheme();
  const toast = useToast();
  return (
    <SettingsSection id="settings-appearance" title="Appearance" description="How Reframe looks.">
      <SettingsRow
        label="Theme"
        description={THEME_DESCRIPTION[preference]}
        control={
          <SegmentedControl
            aria-label="Theme"
            value={preference}
            options={THEME_OPTIONS}
            onChange={(next) => {
              setTheme(next).catch((error: unknown) => toast.error("Couldn't save the theme", errorMessage(error)));
            }}
          />
        }
      />
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------------------------

interface SectionProps {
  settings: AppSettings;
  save: Save;
}

function StartupSection({ settings, save }: SectionProps) {
  const workspaces = useWorkspaces();
  const list = workspaces.data ?? [];
  const loaded = workspaces.data !== undefined;
  const actionLabelId = useId();
  const startup = settings.startup;

  const workspaceOptions: SelectOption[] = list.map((w) => ({ value: w.id, label: `${w.icon} ${w.name}`.trim() }));
  const selectedKnown = list.some((w) => w.id === startup.workspaceId);
  if (!selectedKnown) {
    workspaceOptions.unshift({ value: "", label: list.length === 0 ? "No workspaces yet" : "Choose a workspace", disabled: true });
  }

  const options: RadioOption<StartupAction>[] = [
    { value: "doNothing", label: "Don't restore anything" },
    {
      value: "restoreWorkspace",
      label: "Restore a workspace",
      description: loaded && list.length === 0 ? "Save a workspace first." : undefined,
      disabled: !loaded || (list.length === 0 && startup.action !== "restoreWorkspace"),
      extra: startup.action === "restoreWorkspace" && (
        <Select
          aria-label="Workspace to restore at startup"
          size="sm"
          value={selectedKnown ? (startup.workspaceId ?? "") : ""}
          options={workspaceOptions}
          disabled={list.length === 0}
          onChange={(id) => {
            if (id) void save((s) => withStartupWorkspace(s, id));
          }}
        />
      ),
    },
    {
      value: "restoreLastActive",
      label: "Restore the last active workspace",
      description: "Whichever workspace you restored most recently.",
    },
  ];

  return (
    <SettingsSection id="settings-startup" title="Startup" description="What happens when you log in to your computer.">
      <SettingsRow>
        <Switch
          label="Open Reframe when you log in"
          description="Reframe starts quietly in the background when you sign in."
          checked={startup.launchAtLogin}
          onChange={(on) => void save((s) => ({ ...s, startup: { ...s.startup, launchAtLogin: on } }))}
        />
      </SettingsRow>
      <SettingsRow label="When Reframe opens at login" labelId={actionLabelId}>
        <RadioGroup
          aria-labelledby={actionLabelId}
          value={startup.action}
          options={options}
          onChange={(action) => void save((s) => withStartupAction(s, action, list))}
        />
        {loaded && startupWorkspaceMissing(settings, list) ? (
          <Notice tone="warning" size="sm" className={styles.rowNote}>
            The workspace chosen for startup no longer exists. Choose another one.
          </Notice>
        ) : (
          startupRestoreNeedsLogin(settings) && (
            <Notice tone="info" size="sm" className={styles.rowNote}>
              This only happens when Reframe opens at login. Turn on “Open Reframe when you log in” above.
            </Notice>
          )
        )}
      </SettingsRow>
      <SettingsRow>
        <Switch
          label="Ask before restoring"
          description="Show “Restore now” and “Not now” first instead of restoring right away. Startup restores never close anything."
          checked={startup.askBeforeRestoring}
          disabled={startup.action === "doNothing"}
          onChange={(on) => void save((s) => ({ ...s, startup: { ...s.startup, askBeforeRestoring: on } }))}
        />
      </SettingsRow>
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------------------------
// Restoring
// ---------------------------------------------------------------------------------------------

const MODE_OPTIONS = SWITCH_MODES.map((m) => ({ value: m.value, label: m.label }));

function RestoringSection({ settings, save }: SectionProps) {
  return (
    <SettingsSection
      id="settings-restoring"
      title="Restoring"
      description="How one-click restores behave. You can always choose differently in the restore dialog."
    >
      <SettingsRow
        label="Default way to restore"
        description={
          <>
            <span className={styles.strong}>{MODE_LABEL[settings.defaultSwitchMode]}:</span> {modeCopy(settings.defaultSwitchMode).summary}
          </>
        }
        control={
          <SegmentedControl
            aria-label="Default way to restore"
            value={settings.defaultSwitchMode}
            options={MODE_OPTIONS}
            onChange={(mode) => void save({ defaultSwitchMode: mode })}
          />
        }
      />
      <CloseWaitRow value={settings.closeWaitSeconds} save={save} />
    </SettingsSection>
  );
}

function CloseWaitRow({ value, save }: { value: number; save: Save }) {
  const [text, setText] = useState(String(value));
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const errorId = useId();

  // Follow the saved value unless the user is typing.
  useEffect(() => {
    if (!editing) {
      setText(String(value));
      setError(null);
    }
  }, [value, editing]);

  // Save shortly after typing stops (and right away on blur / Enter).
  useEffect(() => {
    if (!editing) return;
    const parsed = parseCloseWait(text);
    if (!parsed.ok || parsed.value === value) return;
    const timer = setTimeout(() => void save({ closeWaitSeconds: parsed.value }), 700);
    return () => clearTimeout(timer);
  }, [text, editing, value, save]);

  const commit = () => {
    const parsed = parseCloseWait(text);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    setError(null);
    if (parsed.value !== value) void save({ closeWaitSeconds: parsed.value });
  };

  return (
    <SettingsRow
      label="Wait for apps to close"
      description="In Replace, how long Reframe waits for apps to close before moving on. Apps that ask you to save are never forced."
      control={
        <span className={styles.seconds}>
          <Input
            type="number"
            inputMode="numeric"
            min={1}
            max={120}
            step={1}
            size="sm"
            className={styles.secondsInput}
            value={text}
            aria-label="Seconds to wait for apps to close"
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            onFocus={() => setEditing(true)}
            onChange={(event) => {
              setText(event.target.value);
              setError(null);
            }}
            onBlur={() => {
              setEditing(false);
              commit();
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") commit();
              if (event.key === "Escape") {
                setText(String(value));
                setError(null);
              }
            }}
          />
          <span className={styles.secondsUnit}>seconds</span>
        </span>
      }
    >
      {error ? (
        <p id={errorId} className={styles.fieldError} role="alert">
          {error}
        </p>
      ) : undefined}
    </SettingsRow>
  );
}

// ---------------------------------------------------------------------------------------------
// Browser tabs & exclusions
// ---------------------------------------------------------------------------------------------

function BrowserSection({ settings, save }: SectionProps) {
  return (
    <SettingsSection id="settings-browser" title="Browser tabs" description="An optional integration for Chrome, Edge, Brave and other Chromium browsers.">
      <SettingsRow>
        <Switch
          label="Capture open browser tabs"
          description="Reads your browser's local session file to list open tabs. Nothing leaves your computer."
          checked={settings.browserTabCapture}
          onChange={(on) => void save({ browserTabCapture: on })}
        />
      </SettingsRow>
    </SettingsSection>
  );
}

function ExcludedAppsSection({ settings, save }: SectionProps) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const messageId = useId();
  const list = settings.excludedApps;

  const add = (event: FormEvent) => {
    event.preventDefault();
    const result = addExcludedApp(list, text);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    const value = text;
    setText("");
    setError(null);
    void save((s) => {
      const next = addExcludedApp(s.excludedApps, value);
      return next.ok ? { ...s, excludedApps: next.value } : s;
    });
  };

  return (
    <SettingsSection
      id="settings-excluded"
      title="Excluded apps"
      description="Apps Reframe never suggests when you capture your desktop."
    >
      <SettingsRow>
        <form className={styles.addForm} onSubmit={add} noValidate>
          <Input
            value={text}
            placeholder="App name, program or app id"
            aria-label="App to exclude"
            aria-invalid={error ? true : undefined}
            aria-describedby={messageId}
            onChange={(event) => {
              setText(event.target.value);
              setError(null);
            }}
          />
          <Button type="submit" icon={Plus} disabled={text.trim() === ""}>
            Add
          </Button>
        </form>
        <p id={messageId} className={error ? styles.fieldError : styles.fieldHint} role={error ? "alert" : undefined}>
          {error ?? "For example “slack”, “exe:slack” or a full path. Capital letters don't matter."}
        </p>
      </SettingsRow>
      <SettingsRow>
        {list.length === 0 ? (
          <p className={styles.emptyText}>Nothing excluded — every app with a window can be suggested.</p>
        ) : (
          <ul className={styles.excludedList} aria-label="Excluded apps">
            {list.map((item) => (
              <li key={item} className={styles.excludedItem}>
                <span className={styles.excludedName}>{item}</span>
                <IconButton
                  icon={X}
                  size="sm"
                  aria-label={`Remove ${item}`}
                  tooltip
                  onClick={() => void save((s) => ({ ...s, excludedApps: removeExcludedApp(s.excludedApps, item) }))}
                />
              </li>
            ))}
          </ul>
        )}
      </SettingsRow>
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------------------------
// Permissions & this computer
// ---------------------------------------------------------------------------------------------

function PermissionsSection() {
  const permissions = useQuery("permissions", api.permissions, { refetchOnFocus: true });
  const list = permissions.data ?? [];
  // Only some systems need extra permissions (macOS); elsewhere the section is not shown.
  if (list.length === 0) return null;
  return (
    <SettingsSection
      id="settings-permissions"
      title="Permissions"
      description="What your system lets Reframe do. Granting happens in your system settings."
    >
      {list.map((permission) => (
        <PermissionRow key={permission.kind} permission={permission} onChanged={() => void permissions.refresh()} />
      ))}
    </SettingsSection>
  );
}

function PermissionRow({ permission, onChanged }: { permission: PermissionStatus; onChanged: () => void }) {
  const toast = useToast();
  const [requesting, setRequesting] = useState(false);
  const grant = async () => {
    setRequesting(true);
    try {
      await api.requestPermission(permission.kind);
    } catch (error) {
      toast.error(`Couldn't request ${PERMISSION_LABEL[permission.kind]}`, errorMessage(error));
    } finally {
      setRequesting(false);
      onChanged();
    }
  };
  return (
    <SettingsRow
      label={PERMISSION_LABEL[permission.kind]}
      description={permission.purpose}
      control={
        permission.granted ? (
          <Badge tone="success" icon={CircleCheck}>
            Granted
          </Badge>
        ) : (
          <>
            <Badge tone="warning">Not granted</Badge>
            <Button size="sm" variant="primary" loading={requesting} onClick={() => void grant()}>
              Grant
            </Button>
          </>
        )
      }
    >
      {!permission.granted && permission.instructions ? <p className={styles.fieldHint}>{permission.instructions}</p> : undefined}
    </SettingsRow>
  );
}

const SUPPORT_BADGE: Record<SupportLevel, { tone: "success" | "warning" | "danger"; icon: typeof CircleCheck; label: string }> = {
  full: { tone: "success", icon: CircleCheck, label: "Full" },
  partial: { tone: "warning", icon: CircleExclamationPoint, label: "Partial" },
  unsupported: { tone: "danger", icon: CircleMinus, label: "Not available" },
};

function ThisComputerSection() {
  const info = usePlatformInfo();
  const rows = info.data ? capabilityRows(info.data.capabilities) : [];
  return (
    <SettingsSection
      id="settings-computer"
      title="This computer"
      description={
        info.data
          ? `What Reframe can do on this system. ${capabilitySummary(rows)} Limits are shown as they are.`
          : "What Reframe can do on this system."
      }
    >
      {info.data ? (
        <>
          <SettingsRow
            label="System"
            control={<span className={styles.systemValue}>{systemDescription(info.data)}</span>}
          />
          <SettingsRow>
            <ul className={styles.capabilities} aria-label="Capabilities">
              {rows.map((row) => {
                const badge = SUPPORT_BADGE[row.level];
                return (
                  <li key={row.key} className={styles.capability}>
                    <span className={styles.capabilityText}>
                      <span>{row.label}</span>
                      {row.note && <span className={styles.capabilityNote}>{row.note}</span>}
                    </span>
                    <Badge tone={badge.tone} icon={badge.icon}>
                      {badge.label}
                    </Badge>
                  </li>
                );
              })}
            </ul>
          </SettingsRow>
        </>
      ) : info.error ? (
        <SettingsRow>
          <p className={styles.fieldError}>Couldn't read this computer's details: {errorMessage(info.error)}</p>
        </SettingsRow>
      ) : (
        <SettingsRow>
          <div className={styles.skeletonRows} aria-hidden="true">
            {[60, 45, 52].map((w, i) => (
              <Skeleton key={i} width={`${w}%`} height={12} text />
            ))}
          </div>
        </SettingsRow>
      )}
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------------------------
// Shortcuts & about
// ---------------------------------------------------------------------------------------------

function ShortcutsSection() {
  const modifier = primaryModifierLabel();
  return (
    <SettingsSection id="settings-shortcuts" title="Keyboard shortcuts" description="Work anywhere in Reframe.">
      {SHELL_SHORTCUTS.map((shortcut) => (
        <SettingsRow
          key={shortcut.label}
          label={<span className={styles.shortcutLabel}>{shortcut.label}</span>}
          control={
            <span className={styles.keys}>
              {shortcut.primary && <Kbd>{modifier}</Kbd>}
              <Kbd>{shortcut.keys}</Kbd>
            </span>
          }
        />
      ))}
    </SettingsSection>
  );
}

function AboutSection() {
  const version = useAppVersion();
  return (
    <SettingsSection id="settings-about" title="About">
      <SettingsRow>
        <div className={styles.about}>
          <Logo size={44} />
          <div className={styles.aboutText}>
            <p className={styles.aboutName}>
              Reframe
              {version.data && <span className={styles.aboutVersion}>Version {version.data}</span>}
            </p>
            <p className={styles.aboutTagline}>Your workspace, exactly where you left it.</p>
            <p className={styles.aboutFacts}>Free · Offline · No account · No telemetry</p>
          </div>
        </div>
      </SettingsRow>
    </SettingsSection>
  );
}

function SettingsSkeleton() {
  return (
    <div className={styles.content} aria-hidden="true">
      {[2, 3, 2].map((rows, index) => (
        <div key={index} className={styles.skeletonSection}>
          <div className={styles.skeletonIntro}>
            <Skeleton width={110} height={16} />
            <Skeleton width={170} height={12} text />
          </div>
          <div className={styles.skeletonCard}>
            {Array.from({ length: rows }, (_, i) => (
              <div key={i} className={styles.skeletonRow}>
                <Skeleton width={`${40 + ((i * 17) % 30)}%`} height={12} text />
                <Skeleton width={32} height={18} radius={9} />
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
