import { ArrowRight } from "lucide-react";
import { Button } from "@/components/ui";
import { Link, navigate } from "@/lib/router";
import styles from "./FirstRunHero.module.css";

const STEPS = [
  { title: "Explore", text: "See which apps and windows are open right now." },
  { title: "Capture", text: "Pick the ones that belong to what you're working on." },
  { title: "Save", text: "Give it a name and an emoji, like “Deep work”." },
  { title: "Restore", text: "Bring everything back, arranged, in one click." },
] as const;

/** First run (no workspaces yet): how Reframe works, and where to start. */
export function FirstRunHero() {
  return (
    <section className={styles.hero} aria-labelledby="first-run-title">
      <div className={styles.intro}>
        <p className="overline">Get started</p>
        <h2 id="first-run-title" className={styles.title}>
          Save your first workspace
        </h2>
        <p className={styles.lede}>
          Reframe remembers the apps and windows you work with, so you can return to any project exactly where you left
          it.
        </p>
      </div>

      <ol className={styles.steps}>
        {STEPS.map((step, index) => (
          <li key={step.title} className={styles.step}>
            <span className={styles.number} aria-hidden="true">
              {index + 1}
            </span>
            <span className={styles.stepTitle}>{step.title}</span>
            <span className={styles.stepText}>{step.text}</span>
          </li>
        ))}
      </ol>

      <div className={styles.actions}>
        <Button variant="primary" iconEnd={ArrowRight} onClick={() => navigate({ name: "explore" })}>
          Explore your desktop
        </Button>
        <span className={styles.alt}>
          or <Link to={{ name: "import" }}>import a workspace</Link> from another computer
        </span>
      </div>
    </section>
  );
}
