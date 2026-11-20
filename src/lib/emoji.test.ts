import { describe, expect, it } from "vitest";
import { CURATED_EMOJI, DEFAULT_WORKSPACE_EMOJI, MAX_ICON_CODE_POINTS, normalizeEmojiInput } from "./emoji";

describe("CURATED_EMOJI", () => {
  it("has 48 unique, labelled emoji that the backend accepts", () => {
    expect(CURATED_EMOJI).toHaveLength(48);
    expect(new Set(CURATED_EMOJI.map((e) => e.emoji)).size).toBe(48);
    for (const item of CURATED_EMOJI) {
      expect(item.label.length).toBeGreaterThan(0);
      expect(Array.from(item.emoji).length).toBeLessThanOrEqual(MAX_ICON_CODE_POINTS);
      expect(normalizeEmojiInput(item.emoji)).toBe(item.emoji);
    }
    expect(CURATED_EMOJI.some((e) => e.emoji === DEFAULT_WORKSPACE_EMOJI)).toBe(true);
  });
});

describe("normalizeEmojiInput", () => {
  it("takes the first grapheme of trimmed input", () => {
    expect(normalizeEmojiInput("  🚀 ")).toBe("🚀");
    expect(normalizeEmojiInput("🧑‍💻🚀")).toBe("🧑‍💻");
    expect(normalizeEmojiInput("👩‍👩‍👧‍👦")).toBe("👩‍👩‍👧‍👦");
    expect(normalizeEmojiInput("🇯🇵")).toBe("🇯🇵");
  });

  it("allows a letter as an icon", () => {
    expect(normalizeEmojiInput("Alpha")).toBe("A");
  });

  it("rejects empty or whitespace input", () => {
    expect(normalizeEmojiInput("")).toBeNull();
    expect(normalizeEmojiInput("   ")).toBeNull();
  });
});
