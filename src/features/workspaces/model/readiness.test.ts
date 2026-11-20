import { describe, expect, it } from "vitest";
import type { MonitorMapping } from "@/types";
import { describeMapping, describeMonitors, healthSummary, readinessHeadline } from "./readiness";

const estimate = (full: number, partial: number, launchOnly: number, unavailable: number) => ({ full, partial, launchOnly, unavailable });

describe("readinessHeadline", () => {
  it("celebrates a fully restorable workspace", () => {
    expect(readinessHeadline(estimate(5, 0, 0, 0))).toEqual({ title: "All 5 apps can be fully restored", detail: null, tone: "success" });
    expect(readinessHeadline(estimate(1, 0, 0, 0)).title).toBe("The app can be fully restored");
  });

  it("calls mixed capability ready, not fully restorable", () => {
    expect(readinessHeadline(estimate(2, 1, 2, 0)).title).toBe("All 5 apps are ready to restore");
    expect(readinessHeadline(estimate(0, 0, 1, 0)).title).toBe("The app is ready to restore");
  });

  it("counts what can be restored when apps are missing", () => {
    expect(readinessHeadline(estimate(4, 0, 0, 1))).toEqual({
      title: "4 of 5 apps can be fully restored",
      detail: "One app can't be started on this computer and will be skipped.",
      tone: "warning",
    });
    expect(readinessHeadline(estimate(1, 1, 0, 2)).title).toBe("2 of 4 apps are ready to restore");
    expect(readinessHeadline(estimate(0, 0, 1, 1)).title).toBe("1 of 2 apps is ready to restore");
    expect(readinessHeadline(estimate(0, 0, 0, 2)).detail).toBe("2 apps can't be started on this computer and will be skipped.");
  });

  it("handles an empty estimate", () => {
    expect(readinessHeadline(estimate(0, 0, 0, 0)).title).toBe("Nothing to restore yet");
  });
});

const mapping = (savedId: string, mappedId: string | null, exact: boolean): MonitorMapping => ({
  savedId,
  savedName: `Saved ${savedId}`,
  mappedId,
  mappedName: mappedId ? `Now ${mappedId}` : null,
  exact,
});

describe("describeMonitors", () => {
  it("says nothing without a saved layout", () => {
    expect(describeMonitors([])).toBeNull();
  });

  it("describes exact matches", () => {
    expect(describeMonitors([mapping("a", "a", true)])?.text).toBe(
      "Saved on 1 display; it's connected — windows go back exactly where they were.",
    );
    expect(describeMonitors([mapping("a", "a", true), mapping("b", "b", true)])?.text).toBe(
      "Saved on 2 displays; both are connected — windows go back exactly where they were.",
    );
  });

  it("explains rescaling when geometry changed", () => {
    const summary = describeMonitors([mapping("a", "x", false)]);
    expect(summary).toEqual({
      text: "Saved on 1 display; it's connected, but its size or position has changed — windows are moved and scaled to fit.",
      tone: "neutral",
    });
  });

  it("warns about missing displays", () => {
    expect(describeMonitors([mapping("a", "a", true), mapping("b", null, false)])).toEqual({
      text: "Saved on 2 displays; 1 is connected — windows from the missing one go to your main display.",
      tone: "warning",
    });
    expect(describeMonitors([mapping("a", null, false)])?.text).toBe(
      "Saved on a display that isn't connected — its windows go to your main display.",
    );
    expect(describeMonitors([mapping("a", null, false), mapping("b", null, false), mapping("c", "c", true)])?.text).toBe(
      "Saved on 3 displays; 1 is connected — windows from the missing ones go to your main display.",
    );
  });

  it("describes single mappings", () => {
    expect(describeMapping(mapping("a", null, false))).toEqual({ from: "Saved a", to: "Main display", note: "Not connected" });
    expect(describeMapping(mapping("a", "b", true))).toEqual({ from: "Saved a", to: "Now b", note: "Same size and position" });
  });
});

describe("healthSummary", () => {
  it("joins the first few details", () => {
    const details = [
      { label: "Project", value: "reframe" },
      { label: "Tabs", value: "12" },
      { label: "Profile", value: "" },
      { label: "Extra", value: "x" },
    ];
    expect(healthSummary({ details })).toBe("Project: reframe · Tabs: 12 · Profile");
    expect(healthSummary(undefined)).toBe("");
  });
});
