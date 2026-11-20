import { Component, type ErrorInfo, type ReactNode } from "react";
import { House, RotateCcw, TriangleExclamationPoint } from "lucide-react";
import { Button } from "@/components/ui/Button";
import { Icon } from "@/components/ui/Icon";
import styles from "./ErrorBoundary.module.css";

export interface ErrorBoundaryProps {
  children: ReactNode;
  /** When any of these change (e.g. the route), a shown error is cleared. */
  resetKeys?: readonly unknown[];
  /** "Go to Home" action; omitted on the outermost boundary (no router context to rely on). */
  onGoHome?: () => void;
  /** `page` keeps the sidebar visible; `app` fills the window. */
  variant?: "page" | "app";
}

interface ErrorBoundaryState {
  error: Error | null;
}

function sameKeys(a: readonly unknown[] = [], b: readonly unknown[] = []): boolean {
  return a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
}

/** Catches rendering errors and shows a calm recovery screen instead of a blank window. */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Reframe UI error:", error, info.componentStack);
  }

  componentDidUpdate(previous: ErrorBoundaryProps): void {
    if (this.state.error && !sameKeys(previous.resetKeys, this.props.resetKeys)) {
      this.setState({ error: null });
    }
  }

  private reset = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const { onGoHome, variant = "page" } = this.props;
    return (
      <div className={variant === "app" ? styles.app : styles.page} role="alert">
        <div className={styles.card}>
          <div className={styles.icon}>
            <Icon icon={TriangleExclamationPoint} size={20} />
          </div>
          <h1 className={styles.title}>Something went wrong</h1>
          <p className={styles.text}>
            This view ran into an unexpected problem and couldn&rsquo;t be shown. Your saved workspaces are not affected.
          </p>
          <div className={styles.actions}>
            <Button variant="primary" icon={RotateCcw} onClick={this.reset}>
              Try again
            </Button>
            {onGoHome && (
              <Button
                variant="secondary"
                icon={House}
                onClick={() => {
                  this.reset();
                  onGoHome();
                }}
              >
                Go to Home
              </Button>
            )}
          </div>
          <details className={styles.details}>
            <summary>Details</summary>
            <pre className="selectable">{error.stack || error.message}</pre>
          </details>
        </div>
      </div>
    );
  }
}
