import { MonitorSmartphone } from "lucide-react";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "./Logo";
import styles from "./DesktopOnly.module.css";

/**
 * Shown when the UI is opened in a plain browser tab (e.g. the Vite dev server) instead of the
 * Tauri window: none of the desktop APIs exist there, so nothing else can work.
 */
export function DesktopOnly() {
  return (
    <div className={styles.screen}>
      <main className={styles.card}>
        <div className={styles.brand}>
          <Logo size={36} />
          <span className={styles.wordmark}>Reframe</span>
        </div>
        <div className={styles.icon}>
          <Icon icon={MonitorSmartphone} size={20} />
        </div>
        <h1 className={styles.title}>Open Reframe in the desktop app</h1>
        <p className={styles.text}>
          Reframe needs its desktop window to see your apps and windows and to save workspaces. This browser tab
          can&rsquo;t reach them.
        </p>
        <p className={styles.hint}>
          Developing? Start it with <code className="selectable">npm run tauri dev</code>
        </p>
      </main>
    </div>
  );
}
