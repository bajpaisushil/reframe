import { describe, expect, it } from "vitest";
import { exportFileName, formatSeconds, parsePage, parseSeconds, parseTabAddress, parseWholeNumber } from "./validation";

describe("parseTabAddress", () => {
  it("adds https to bare domains and derives a title", () => {
    expect(parseTabAddress("  example.com/docs ")).toEqual({ ok: true, value: { url: "https://example.com/docs", title: "example.com" } });
    expect(parseTabAddress("www.reframe.app")).toEqual({ ok: true, value: { url: "https://www.reframe.app/", title: "reframe.app" } });
  });

  it("keeps explicit http(s) and uses http for localhost", () => {
    expect(parseTabAddress("http://intranet/wiki")).toMatchObject({ ok: true, value: { url: "http://intranet/wiki" } });
    expect(parseTabAddress("localhost:3000/app")).toMatchObject({ ok: true, value: { url: "http://localhost:3000/app" } });
  });

  it("accepts file addresses", () => {
    expect(parseTabAddress("file:///home/me/notes%20today.html")).toEqual({
      ok: true,
      value: { url: "file:///home/me/notes%20today.html", title: "notes today.html" },
    });
  });

  it("rejects empty, spaced, schemeless words and other protocols", () => {
    expect(parseTabAddress("").ok).toBe(false);
    expect(parseTabAddress("not a url").ok).toBe(false);
    expect(parseTabAddress("intranet").ok).toBe(false);
    expect(parseTabAddress("javascript:alert(1)")).toEqual({
      ok: false,
      error: "Only web (http or https) and file addresses can be opened as tabs.",
    });
    expect(parseTabAddress("mailto:me@example.com").ok).toBe(false);
  });
});

describe("numbers", () => {
  it("parses seconds into milliseconds within a range", () => {
    expect(parseSeconds("1.5", { min: 0, max: 60 })).toEqual({ ok: true, value: 1500 });
    expect(parseSeconds("0,3", { min: 0, max: 60 })).toEqual({ ok: true, value: 300 });
    expect(parseSeconds("", { min: 0, max: 60, optional: true })).toEqual({ ok: true, value: null });
    expect(parseSeconds("", { min: 0, max: 60 }).ok).toBe(false);
    expect(parseSeconds("-1", { min: 0, max: 60 }).ok).toBe(false);
    expect(parseSeconds("abc", { min: 0, max: 60 }).ok).toBe(false);
    expect(parseSeconds("400", { min: 1, max: 300 })).toEqual({ ok: false, error: "Use 1–300 seconds." });
  });

  it("parses whole numbers and pages", () => {
    expect(parseWholeNumber("3", { min: 0, max: 5 })).toEqual({ ok: true, value: 3 });
    expect(parseWholeNumber("2.5", { min: 0, max: 5 }).ok).toBe(false);
    expect(parseWholeNumber("9", { min: 1, max: 8 })).toEqual({ ok: false, error: "Use a number from 1 to 8." });
    expect(parsePage("")).toEqual({ ok: true, value: null });
    expect(parsePage("0").ok).toBe(false);
  });

  it("formats milliseconds as seconds", () => {
    expect(formatSeconds(300)).toBe("0.3");
    expect(formatSeconds(20000)).toBe("20");
    expect(formatSeconds(1250)).toBe("1.25");
    expect(formatSeconds(null)).toBe("");
  });
});

describe("exportFileName", () => {
  it("builds a safe default file name", () => {
    expect(exportFileName("Deep work")).toBe("Deep work.reframe.json");
    expect(exportFileName('Client: "Acme" / Q4?')).toBe("Client Acme Q4.reframe.json");
    expect(exportFileName("...")).toBe("Workspace.reframe.json");
    expect(exportFileName("  ")).toBe("Workspace.reframe.json");
  });
});
