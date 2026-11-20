import { describe, expect, it } from "vitest";
import type { ImportApplication, ImportPreview, LaunchTarget } from "@/types";
import {
  addMappingRow,
  buildResolution,
  choiceFor,
  effectiveTarget,
  importCounts,
  isPathMapped,
  mappingProblems,
  mappingsForImport,
  needsAttention,
  normalizeFolder,
  removeMappingRow,
  rowsFromMappings,
  selectedTargetValue,
  targetLabel,
  targetOptions,
  updateMappingRow,
  validateImportName,
  withChoice,
  type KeyFactory,
} from "./importModel";

function keys(): KeyFactory {
  let n = 0;
  return () => `k${++n}`;
}

const exe = (path: string): LaunchTarget => ({ kind: "executable", path });

function app(entryId: string, overrides: Partial<ImportApplication> = {}): ImportApplication {
  return {
    entryId,
    appId: entryId,
    displayName: entryId.toUpperCase(),
    adapterId: "generic",
    resolution: { kind: "found", target: exe(`/usr/bin/${entryId}`) },
    alternatives: [],
    missingPaths: [],
    ...overrides,
  };
}

function preview(applications: ImportApplication[]): ImportPreview {
  return {
    filePath: "/home/me/dev.reframe.json",
    name: "Development",
    description: "",
    icon: "💻",
    sourceOs: "windows",
    exportedAt: "2026-11-01T09:00:00Z",
    applications,
    suggestedPathMappings: [],
    warnings: [],
  };
}

describe("folder mapping rows", () => {
  it("starts from the suggestions and supports add, edit and remove", () => {
    const makeKey = keys();
    let rows = rowsFromMappings([{ fromPrefix: "C:\\Users\\a\\code", toPrefix: "/home/a/code" }], makeKey);
    expect(rows).toEqual([{ key: "k1", fromPrefix: "C:\\Users\\a\\code", toPrefix: "/home/a/code" }]);
    rows = addMappingRow(rows, makeKey);
    expect(rows[1]).toEqual({ key: "k2", fromPrefix: "", toPrefix: "" });
    rows = updateMappingRow(rows, "k2", { fromPrefix: "D:\\photos" });
    rows = updateMappingRow(rows, "k2", { toPrefix: "/media/photos" });
    expect(rows[1]).toEqual({ key: "k2", fromPrefix: "D:\\photos", toPrefix: "/media/photos" });
    rows = removeMappingRow(rows, "k1");
    expect(rows.map((r) => r.key)).toEqual(["k2"]);
  });

  it("reports incomplete and repeated rows but ignores empty ones", () => {
    const rows = [
      { key: "a", fromPrefix: "C:\\code", toPrefix: "/home/a/code" },
      { key: "b", fromPrefix: "", toPrefix: "" },
      { key: "c", fromPrefix: "c:/code/", toPrefix: "/srv/code" },
      { key: "d", fromPrefix: "D:\\x", toPrefix: "  " },
      { key: "e", fromPrefix: "", toPrefix: "/tmp" },
    ];
    const problems = mappingProblems(rows);
    expect([...problems.keys()]).toEqual(["c", "d", "e"]);
    expect(problems.get("c")).toEqual({ kind: "duplicate", message: "This folder is already mapped above." });
    expect(problems.get("d")?.kind).toBe("incomplete");
    expect(mappingsForImport(rows)).toEqual([{ fromPrefix: "C:\\code", toPrefix: "/home/a/code" }]);
  });

  it("normalizes folders without breaking roots", () => {
    expect(normalizeFolder(" /home/a/code/ ")).toBe("/home/a/code");
    expect(normalizeFolder("C:\\Users\\")).toBe("C:\\Users");
    expect(normalizeFolder("/")).toBe("/");
    expect(normalizeFolder("C:\\")).toBe("C:\\");
  });

  it("tells which missing paths a mapping covers", () => {
    const mappings = [
      { fromPrefix: "C:\\Users\\a\\code", toPrefix: "/home/a/code" },
      { fromPrefix: "/Users/a/notes/", toPrefix: "/home/a/notes" },
    ];
    expect(isPathMapped("C:\\Users\\a\\code\\reframe", mappings)).toBe(true);
    expect(isPathMapped("c:\\users\\A\\CODE", mappings)).toBe(true);
    expect(isPathMapped("C:\\Users\\a\\codebase", mappings)).toBe(false);
    expect(isPathMapped("/Users/a/notes/today.md", mappings)).toBe(true);
    expect(isPathMapped("/Users/A/notes/today.md", mappings)).toBe(false);
    expect(isPathMapped("/elsewhere", [])).toBe(false);
  });
});

describe("app choices", () => {
  const unresolved = app("gimp", {
    resolution: { kind: "unresolved" },
    alternatives: [exe("/opt/gimp/bin/gimp"), exe("/usr/local/bin/gimp")],
  });

  it("describes targets", () => {
    expect(targetLabel(exe("/usr/bin/code"))).toBe("/usr/bin/code");
    expect(targetLabel({ kind: "package", package: { kind: "flatpak", id: "org.gimp.GIMP", path: null } })).toBe("Flatpak: org.gimp.GIMP");
    expect(targetLabel({ kind: "open", target: "https://example.com" })).toBe("https://example.com");
  });

  it("flags apps without a target until one is chosen or the app is skipped", () => {
    expect(needsAttention(unresolved, choiceFor({}, "gimp"))).toBe(true);
    const chosen = withChoice({}, "gimp", { target: exe("/opt/gimp/bin/gimp") });
    expect(needsAttention(unresolved, choiceFor(chosen, "gimp"))).toBe(false);
    expect(effectiveTarget(unresolved, choiceFor(chosen, "gimp"))).toEqual(exe("/opt/gimp/bin/gimp"));
    const skipped = withChoice({}, "gimp", { skip: true });
    expect(needsAttention(unresolved, choiceFor(skipped, "gimp"))).toBe(false);
    expect(needsAttention(app("code"), choiceFor({}, "code"))).toBe(false);
  });

  it("offers the automatic match, alternatives and a custom pick once each", () => {
    const found = app("code", { alternatives: [exe("/usr/bin/code"), exe("/snap/bin/code")] });
    expect(targetOptions(found, choiceFor({}, "code")).map((o) => o.value)).toEqual(["auto", "alt:1"]);
    const custom = choiceFor(withChoice({}, "code", { target: exe("/home/me/bin/code") }), "code");
    const options = targetOptions(found, custom);
    expect(options.map((o) => o.label)).toEqual(["/usr/bin/code", "/snap/bin/code", "/home/me/bin/code"]);
    expect(selectedTargetValue(options, found, custom)).toBe("custom");
    expect(selectedTargetValue(targetOptions(unresolved, choiceFor({}, "gimp")), unresolved, choiceFor({}, "gimp"))).toBe("");
  });

  it("counts what will be imported", () => {
    const p = preview([app("code"), unresolved, app("vlc")]);
    expect(importCounts(p, {})).toEqual({ total: 3, included: 3, skipped: 0, attention: 1 });
    expect(importCounts(p, withChoice({}, "vlc", { skip: true }))).toEqual({ total: 3, included: 2, skipped: 1, attention: 1 });
  });
});

describe("buildResolution", () => {
  it("sends the user's decisions", () => {
    const p = preview([app("code"), app("gimp", { resolution: { kind: "unresolved" } }), app("vlc")]);
    let choices = withChoice({}, "gimp", { target: exe("/opt/gimp/bin/gimp") });
    choices = withChoice(choices, "vlc", { skip: true });
    choices = withChoice(choices, "code", { target: exe("/usr/bin/code") }); // same as automatic
    const rows = [
      { key: "a", fromPrefix: " C:\\code\\ ", toPrefix: "/home/me/code/" },
      { key: "b", fromPrefix: "", toPrefix: "" },
    ];
    expect(buildResolution(p, "  Dev on Linux ", choices, rows)).toEqual({
      filePath: "/home/me/dev.reframe.json",
      name: "Dev on Linux",
      appMappings: [
        { entryId: "code", target: null, skip: false },
        { entryId: "gimp", target: exe("/opt/gimp/bin/gimp"), skip: false },
        { entryId: "vlc", target: null, skip: true },
      ],
      pathMappings: [{ fromPrefix: "C:\\code", toPrefix: "/home/me/code" }],
    });
  });

  it("keeps the exported name when unchanged", () => {
    expect(buildResolution(preview([]), " Development ", {}, []).name).toBeNull();
  });

  it("validates the name", () => {
    expect(validateImportName("  ")).toBe("Give the workspace a name.");
    expect(validateImportName("x".repeat(81))).toBe("Use at most 80 characters.");
    expect(validateImportName("Development")).toBeNull();
  });
});
