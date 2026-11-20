import { useMemo, useState, type Dispatch } from "react";
import {
  closestCenter,
  DndContext,
  KeyboardSensor,
  MeasuringStrategy,
  PointerSensor,
  useSensor,
  useSensors,
  type Announcements,
  type DragEndEvent,
  type Modifier,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import { SortableContext, sortableKeyboardCoordinates, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { AppWindow, ChevronDown, FolderSearch, MonitorUp, Plus } from "lucide-react";
import { Button, ConfirmDialog, EmptyState, Menu, SectionHeader } from "@/components/ui";
import { formatList } from "@/lib/format";
import type { ApplicationEntry, EntryHealth, MonitorInfo, RestoreConfiguration } from "@/types";
import { dependencyLabels, dependents, launchOrder } from "../model/dependencies";
import { ordinal } from "../model/entries";
import { deepEqual, type DraftAction } from "../model/draft";
import { ApplicationEditor } from "./editor/ApplicationEditor";
import { ApplicationRow } from "./ApplicationRow";
import styles from "./ApplicationList.module.css";

export interface ApplicationListProps {
  entries: readonly ApplicationEntry[];
  /** Saved entries, to know when health details describe stale content. */
  savedEntries: readonly ApplicationEntry[];
  monitors: readonly MonitorInfo[];
  restore: RestoreConfiguration;
  healthById: ReadonlyMap<string, EntryHealth>;
  dispatch: Dispatch<DraftAction>;
  expandedId: string | null;
  onExpandedChange: (id: string | null) => void;
  /** Rows to highlight briefly (just added). */
  highlightIds: ReadonlySet<string>;
  onAddFromDesktop: () => void;
  onChooseApp: () => void;
  choosing: boolean;
}

/** Vertical only, and never outside the list. */
const restrictToList: Modifier = ({ transform, draggingNodeRect, containerNodeRect }) => {
  if (!draggingNodeRect || !containerNodeRect) return { ...transform, x: 0 };
  const minY = containerNodeRect.top - draggingNodeRect.top;
  const maxY = containerNodeRect.bottom - draggingNodeRect.bottom;
  return { ...transform, x: 0, y: Math.min(Math.max(transform.y, minY), maxY) };
};

const screenReaderInstructions = {
  draggable:
    "To change when this app starts, press Space or Enter to pick it up, use the up and down arrow keys to move it, then press Space or Enter to drop it. Press Escape to cancel.",
};

/** The launch sequence: apps start from the top, in this order (dependencies permitting). */
export function ApplicationList({
  entries,
  savedEntries,
  monitors,
  restore,
  healthById,
  dispatch,
  expandedId,
  onExpandedChange,
  highlightIds,
  onAddFromDesktop,
  onChooseApp,
  choosing,
}: ApplicationListProps) {
  const [removeTarget, setRemoveTarget] = useState<ApplicationEntry | null>(null);
  const [removeOpen, setRemoveOpen] = useState(false);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const ids = useMemo(() => entries.map((entry) => entry.id), [entries]);
  const savedById = useMemo(() => new Map(savedEntries.map((entry) => [entry.id, entry])), [savedEntries]);

  // When each app really starts (start-after settings can move an app later than its row), and a
  // short explanation wherever that differs from the order of the list.
  const positions = useMemo(() => {
    const launch = new Map(launchOrder(entries).map((id, index) => [id, index + 1]));
    const notes = new Map<string, string>();
    let listed = 0;
    for (const entry of entries) {
      if (!entry.enabled) continue;
      const position = launch.get(entry.id);
      if (position === undefined) {
        notes.set(entry.id, "Can't start: what it starts after leads back to itself");
        continue;
      }
      // Its place among the apps that can start, if the list order alone decided.
      listed += 1;
      if (position !== listed) {
        const waitsFor = dependencyLabels(entries, entry)
          .map((label) => label.name)
          .filter((name): name is string => name !== null);
        const waitedOnBy = dependents(entries, entry.id)
          .filter((other) => other.enabled)
          .map((other) => other.displayName);
        const when = `Starts ${ordinal(position)}`;
        notes.set(
          entry.id,
          waitsFor.length > 0
            ? `${when} because it waits for ${formatList(waitsFor)}`
            : waitedOnBy.length > 0
              ? `${when} because ${formatList(waitedOnBy)} ${waitedOnBy.length === 1 ? "waits" : "wait"} for it`
              : `${when}: an app listed above it waits for one listed further down`,
        );
      }
    }
    return { launch, notes };
  }, [entries]);

  const nameOf = (id: UniqueIdentifier) => entries.find((entry) => entry.id === id)?.displayName ?? "The app";
  const placeOf = (id: UniqueIdentifier) => ids.indexOf(String(id)) + 1;
  const announcements: Announcements = {
    onDragStart: ({ active }) => `Picked up ${nameOf(active.id)}. It is number ${placeOf(active.id)} of ${ids.length}.`,
    onDragOver: ({ active, over }) =>
      over ? `${nameOf(active.id)} moved to number ${placeOf(over.id)} of ${ids.length}.` : `${nameOf(active.id)} is outside the list.`,
    onDragEnd: ({ active, over }) =>
      over ? `${nameOf(active.id)} dropped at number ${placeOf(over.id)} of ${ids.length}.` : `${nameOf(active.id)} dropped.`,
    onDragCancel: ({ active }) => `Cancelled. ${nameOf(active.id)} stays at number ${placeOf(active.id)}.`,
  };

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return;
    dispatch({ type: "moveEntry", from: ids.indexOf(String(active.id)), to: ids.indexOf(String(over.id)) });
  };

  const removeDependents = removeTarget ? dependents(entries, removeTarget.id) : [];

  const addMenu = (
    <Menu
      aria-label="Add applications"
      minWidth={240}
      items={[
        {
          label: "From open windows…",
          description: "Pick apps that are running now",
          icon: MonitorUp,
          onSelect: onAddFromDesktop,
        },
        {
          label: "Choose an app…",
          description: "Add an app that isn't open",
          icon: FolderSearch,
          onSelect: onChooseApp,
        },
      ]}
      trigger={(props) => (
        <Button {...props} size="sm" variant="secondary" icon={Plus} iconEnd={ChevronDown} loading={choosing}>
          Add applications
        </Button>
      )}
    />
  );

  return (
    <section aria-labelledby="applications-title">
      <SectionHeader
        id="applications-title"
        title="Applications"
        count={entries.length}
        description={entries.length > 1 ? "Apps start from the top, in this order. Drag to rearrange." : undefined}
        action={entries.length > 0 ? addMenu : undefined}
      />

      {entries.length === 0 ? (
        <div className={styles.emptyPanel}>
          <EmptyState
            size="compact"
            icon={AppWindow}
            title="No apps in this workspace yet"
            text="Add the apps you use for this work — pick them from your open windows, or choose an app that isn't running."
            action={
              <>
                <Button size="sm" variant="primary" icon={MonitorUp} onClick={onAddFromDesktop}>
                  Add from open windows
                </Button>
                <Button size="sm" variant="secondary" icon={FolderSearch} loading={choosing} onClick={onChooseApp}>
                  Choose an app…
                </Button>
              </>
            }
          />
        </div>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          modifiers={[restrictToList]}
          measuring={{ droppable: { strategy: MeasuringStrategy.Always } }}
          accessibility={{ announcements, screenReaderInstructions }}
          onDragStart={() => onExpandedChange(null)}
          onDragEnd={onDragEnd}
        >
          <SortableContext items={ids} strategy={verticalListSortingStrategy}>
            <ul className={styles.list}>
              {entries.map((entry) => {
                const saved = savedById.get(entry.id);
                const expanded = expandedId === entry.id;
                return (
                  <ApplicationRow
                    key={entry.id}
                    entry={entry}
                    position={positions.launch.get(entry.id) ?? null}
                    positionNote={positions.notes.get(entry.id)}
                    health={healthById.get(entry.id)}
                    stateChanged={!saved || !deepEqual(saved.state, entry.state)}
                    dependencies={dependencyLabels(entries, entry)}
                    expanded={expanded}
                    highlighted={highlightIds.has(entry.id)}
                    onToggleExpanded={() => onExpandedChange(expanded ? null : entry.id)}
                    onToggleEnabled={(enabled) =>
                      dispatch({ type: "updateEntry", entryId: entry.id, update: (e) => ({ ...e, enabled }) })
                    }
                  >
                    {expanded && (
                      <ApplicationEditor
                        entry={entry}
                        entries={entries}
                        monitors={monitors}
                        restore={restore}
                        health={healthById.get(entry.id)}
                        onUpdate={(update) => dispatch({ type: "updateEntry", entryId: entry.id, update })}
                        onRemove={() => {
                          setRemoveTarget(entry);
                          setRemoveOpen(true);
                        }}
                      />
                    )}
                  </ApplicationRow>
                );
              })}
            </ul>
          </SortableContext>
        </DndContext>
      )}

      <ConfirmDialog
        open={removeOpen}
        onClose={() => setRemoveOpen(false)}
        onConfirm={() => {
          if (!removeTarget) return;
          if (expandedId === removeTarget.id) onExpandedChange(null);
          dispatch({ type: "removeEntry", entryId: removeTarget.id });
        }}
        tone="danger"
        title={`Remove ${removeTarget?.displayName ?? "this app"}?`}
        description={
          removeDependents.length > 0
            ? `Its saved content and windows leave this workspace, and ${formatList(removeDependents.map((d) => d.displayName))} will no longer wait for it.`
            : "Its saved content and windows leave this workspace. The app itself isn't affected."
        }
        confirmLabel="Remove app"
      />
    </section>
  );
}
