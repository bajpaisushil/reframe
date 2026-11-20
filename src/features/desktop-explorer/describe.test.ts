import { describe, expect, it } from "vitest";
import { backgroundNote, detailText, groupByCategory, monitorNames, windowCountLabel, windowSize, windowTitle } from "./describe";

describe("window labels", () => {
  it("names untitled windows and formats sizes", () => {
    expect(windowTitle("  ")).toBe("Untitled window");
    expect(windowTitle(" ~/src — bash ")).toBe("~/src — bash");
    expect(windowSize({ width: 1280.4, height: 719.6 })).toBe("1,280 × 720");
  });

  it("formats adapter details", () => {
    expect(detailText({ label: "Project", value: "reframe" })).toBe("Project: reframe");
    expect(detailText({ label: "Private window", value: "" })).toBe("Private window");
  });

  it("counts windows, mentioning partial selections", () => {
    expect(windowCountLabel(1, null)).toBe("1 window");
    expect(windowCountLabel(3, 3)).toBe("3 windows");
    expect(windowCountLabel(3, 0)).toBe("3 windows");
    expect(windowCountLabel(3, 1)).toBe("1 of 3 windows");
  });
});

describe("backgroundNote", () => {
  it("uses singular and plural forms and stays empty for zero", () => {
    expect(backgroundNote(48)).toBe("48 background processes aren't shown — they have no windows.");
    expect(backgroundNote(1)).toBe("1 background process isn't shown — it has no windows.");
    expect(backgroundNote(0)).toBe("");
  });
});

describe("groupByCategory", () => {
  it("keeps the order inside each category", () => {
    const groups = groupByCategory([
      { key: "a", category: "application" as const },
      { key: "s", category: "system" as const },
      { key: "u", category: "utility" as const },
      { key: "b", category: "application" as const },
    ]);
    expect(groups.application.map((a) => a.key)).toEqual(["a", "b"]);
    expect(groups.utility.map((a) => a.key)).toEqual(["u"]);
    expect(groups.system.map((a) => a.key)).toEqual(["s"]);
  });
});

describe("monitorNames", () => {
  it("only names monitors when there is more than one", () => {
    expect(monitorNames([{ id: "a", name: "HDMI-1" }]).size).toBe(0);
    expect(monitorNames([{ id: "a", name: "HDMI-1" }, { id: "b", name: "DP-1" }]).get("b")).toBe("DP-1");
  });
});
