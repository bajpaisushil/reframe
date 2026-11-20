import { describe, expect, it } from "vitest";
import { filterAndSortWorkspaces, foldText, isWorkspaceSort, matchesQuery, sortWorkspaces } from "./listing";

const ws = (
  name: string,
  createdAt: string,
  updatedAt: string,
  lastRestoredAt: string | null = null,
  extra: { description?: string; applicationNames?: string[] } = {},
) => ({
  name,
  description: extra.description ?? "",
  applicationNames: extra.applicationNames ?? [],
  createdAt,
  updatedAt,
  lastRestoredAt,
});

const list = [
  ws("Writing", "2026-11-01T00:00:00Z", "2026-11-02T00:00:00Z", "2026-11-19T00:00:00Z", { applicationNames: ["Obsidian"] }),
  ws("client calls", "2026-11-10T00:00:00Z", "2026-11-10T00:00:00Z", null, { description: "Zoom and notes" }),
  ws("Résumé", "2026-11-05T00:00:00Z", "2026-11-18T00:00:00Z"),
  ws("Project 10", "2026-10-01T00:00:00Z", "2026-10-01T00:00:00Z"),
  ws("Project 9", "2026-10-02T00:00:00Z", "2026-10-02T00:00:00Z"),
];

describe("matchesQuery", () => {
  it("matches name, description and app names, ignoring case and accents", () => {
    expect(matchesQuery(list[2], "resume")).toBe(true);
    expect(matchesQuery(list[1], "ZOOM")).toBe(true);
    expect(matchesQuery(list[0], "obsid")).toBe(true);
    expect(matchesQuery(list[0], "zoom")).toBe(false);
  });

  it("requires every word and treats a blank query as a match", () => {
    expect(matchesQuery(list[1], "client notes")).toBe(true);
    expect(matchesQuery(list[1], "client obsidian")).toBe(false);
    expect(matchesQuery(list[1], "   ")).toBe(true);
  });

  it("folds text", () => {
    expect(foldText("Ça Été")).toBe("ca ete");
  });
});

describe("sortWorkspaces", () => {
  it("sorts by name naturally and case-insensitively", () => {
    expect(sortWorkspaces(list, "name").map((w) => w.name)).toEqual(["client calls", "Project 9", "Project 10", "Résumé", "Writing"]);
  });

  it("sorts by creation, newest first", () => {
    expect(sortWorkspaces(list, "created").map((w) => w.name)).toEqual(["client calls", "Résumé", "Writing", "Project 9", "Project 10"]);
  });

  it("sorts by last use (restored or edited), newest first", () => {
    expect(sortWorkspaces(list, "recent").map((w) => w.name)).toEqual(["Writing", "Résumé", "client calls", "Project 9", "Project 10"]);
  });

  it("filters before sorting", () => {
    expect(filterAndSortWorkspaces(list, "project", "name").map((w) => w.name)).toEqual(["Project 9", "Project 10"]);
  });

  it("recognises sort values", () => {
    expect(isWorkspaceSort("name")).toBe(true);
    expect(isWorkspaceSort("size")).toBe(false);
  });
});
