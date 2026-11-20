import { describe, expect, it } from "vitest";
import {
  AVATAR_COLOR_COUNT,
  WORKSPACE_COLORS,
  avatarColorIndex,
  firstGrapheme,
  hashString,
  hexToRgb,
  hexToRgba,
  initials,
  normalizeHexColor,
  workspaceColorName,
} from "./color";

describe("normalizeHexColor", () => {
  it("accepts #RRGGBB, #RGB and a missing hash", () => {
    expect(normalizeHexColor("#6d5dfc")).toBe("#6D5DFC");
    expect(normalizeHexColor("6D5DFC")).toBe("#6D5DFC");
    expect(normalizeHexColor("#abc")).toBe("#AABBCC");
    expect(normalizeHexColor("  #ABCDEF ")).toBe("#ABCDEF");
  });

  it("rejects anything else", () => {
    expect(normalizeHexColor(null)).toBeNull();
    expect(normalizeHexColor("")).toBeNull();
    expect(normalizeHexColor("red")).toBeNull();
    expect(normalizeHexColor("#12345")).toBeNull();
    expect(normalizeHexColor("#GGGGGG")).toBeNull();
  });
});

describe("hex conversions", () => {
  it("converts to rgb / rgba", () => {
    expect(hexToRgb("#6D5DFC")).toEqual({ r: 109, g: 93, b: 252 });
    expect(hexToRgba("#000", 0.5)).toBe("rgba(0, 0, 0, 0.5)");
    expect(hexToRgba("#FFFFFF", 4)).toBe("rgba(255, 255, 255, 1)");
    expect(hexToRgba("nope", 0.5)).toBeNull();
  });

  it("names curated workspace colors", () => {
    expect(workspaceColorName("#6d5dfc")).toBe("Indigo");
    expect(workspaceColorName("#123456")).toBeNull();
    expect(new Set(WORKSPACE_COLORS.map((c) => c.value)).size).toBe(8);
    for (const color of WORKSPACE_COLORS) expect(normalizeHexColor(color.value)).toBe(color.value);
  });
});

describe("hashString / avatarColorIndex", () => {
  it("is deterministic and matches FNV-1a", () => {
    expect(hashString("")).toBe(0x811c9dc5);
    expect(hashString("a")).toBe(0xe40c292c);
    expect(hashString("vscode")).toBe(hashString("vscode"));
    expect(hashString("vscode")).not.toBe(hashString("chrome"));
  });

  it("stays within the palette and ignores case and padding", () => {
    for (const id of ["vscode", "chrome", "exe:gimp", "org.gnome.Nautilus", "", "微信"]) {
      const index = avatarColorIndex(id);
      expect(index).toBeGreaterThanOrEqual(0);
      expect(index).toBeLessThan(AVATAR_COLOR_COUNT);
    }
    expect(avatarColorIndex("Firefox")).toBe(avatarColorIndex(" firefox "));
  });

  it("spreads typical app ids over several colors", () => {
    const ids = ["vscode", "chrome", "firefox", "slack", "spotify", "terminal", "nautilus", "gimp", "zoom", "figma"];
    expect(new Set(ids.map(avatarColorIndex)).size).toBeGreaterThanOrEqual(4);
  });
});

describe("firstGrapheme", () => {
  it("keeps emoji sequences and accents intact", () => {
    expect(firstGrapheme("🧑‍💻 code")).toBe("🧑‍💻");
    expect(firstGrapheme("🇩🇪")).toBe("🇩🇪");
    expect(firstGrapheme("école")).toBe("é");
    expect(firstGrapheme("")).toBe("");
  });
});

describe("initials", () => {
  it("uses the first letters of the first two words", () => {
    expect(initials("Visual Studio Code")).toBe("VS");
    expect(initials("google-chrome")).toBe("GC");
    expect(initials("GNU Image Manipulation Program")).toBe("GI");
  });

  it("uses a camel-case hump in single words", () => {
    expect(initials("GitHub")).toBe("GH");
    expect(initials("iTerm")).toBe("IT");
    expect(initials("LibreOffice")).toBe("LO");
  });

  it("falls back to one letter", () => {
    expect(initials("firefox")).toBe("F");
    expect(initials("Slack")).toBe("S");
    expect(initials("x")).toBe("X");
  });

  it("strips file suffixes and handles non-Latin and empty names", () => {
    expect(initials("notepad.exe")).toBe("N");
    expect(initials("Calculator.app")).toBe("C");
    expect(initials("微信")).toBe("微");
    expect(initials("   ")).toBe("?");
    expect(initials("--")).toBe("?");
  });
});
