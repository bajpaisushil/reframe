import { describe, expect, it } from "vitest";
import { addExclusion, entryMatches, exclusionCandidates, removeExclusion, type ExclusionTarget } from "./exclusions";

const slack: ExclusionTarget = { appId: "exe:slack", executablePath: "/usr/lib/slack/Slack", package: null };
const code: ExclusionTarget = {
  appId: "vscode",
  executablePath: "C:\\Program Files\\Microsoft VS Code\\Code.exe",
  package: null,
};
const spotify: ExclusionTarget = {
  appId: "spotify",
  executablePath: null,
  package: { kind: "flatpak", id: "com.spotify.Client", path: null },
};

describe("exclusionCandidates", () => {
  it("lists the app id, executable names and package id in lower case", () => {
    expect(exclusionCandidates(code)).toEqual([
      "vscode",
      "c:\\program files\\microsoft vs code\\code.exe",
      "code.exe",
      "code",
    ]);
    expect(exclusionCandidates(spotify)).toEqual(["spotify", "com.spotify.client"]);
  });
});

describe("entryMatches", () => {
  it("matches case-insensitively, by stem and by exe:/pkg: ids", () => {
    expect(entryMatches("Slack", slack)).toBe(true);
    expect(entryMatches("exe:slack", slack)).toBe(true);
    expect(entryMatches("  CODE  ", code)).toBe(true);
    expect(entryMatches("pkg:com.spotify.client", spotify)).toBe(true);
    expect(entryMatches("discord", slack)).toBe(false);
    expect(entryMatches("", slack)).toBe(false);
  });
});

describe("addExclusion / removeExclusion", () => {
  it("adds the app id once", () => {
    expect(addExclusion(["discord"], slack)).toEqual(["discord", "exe:slack"]);
    expect(addExclusion(["SLACK"], slack)).toEqual(["SLACK"]);
  });

  it("removes every entry that refers to the app and keeps the others", () => {
    expect(removeExclusion(["slack", "discord", "exe:slack", "/usr/lib/slack/Slack"], slack)).toEqual(["discord"]);
    expect(removeExclusion(["discord"], slack)).toEqual(["discord"]);
  });
});
