import { describe, expect, it } from "vitest";
import { parseNavigateRequest } from "./navigation";

describe("parseNavigateRequest", () => {
  it("maps each supported route", () => {
    expect(parseNavigateRequest({ route: "home" })).toEqual({ route: { name: "home" }, message: null });
    expect(parseNavigateRequest({ route: "explore", runId: null, message: null })).toEqual({
      route: { name: "explore" },
      message: null,
    });
    expect(parseNavigateRequest({ route: "restore", runId: "7f0c-run" })).toEqual({
      route: { name: "restore", runId: "7f0c-run" },
      message: null,
    });
  });

  it("passes a trimmed message along with the route", () => {
    expect(parseNavigateRequest({ route: "home", message: "  Deep work is already being restored. " })).toEqual({
      route: { name: "home" },
      message: "Deep work is already being restored.",
    });
  });

  it("ignores empty or non-string messages", () => {
    expect(parseNavigateRequest({ route: "home", message: "   " }).message).toBeNull();
    expect(parseNavigateRequest({ route: "home", message: 42 }).message).toBeNull();
  });

  it("needs a run id to open a restore", () => {
    expect(parseNavigateRequest({ route: "restore" }).route).toBeNull();
    expect(parseNavigateRequest({ route: "restore", runId: "" }).route).toBeNull();
    expect(parseNavigateRequest({ route: "restore", runId: 12 }).route).toBeNull();
  });

  it("rejects unknown routes and malformed payloads but keeps the message", () => {
    expect(parseNavigateRequest({ route: "settings", message: "Oops" })).toEqual({ route: null, message: "Oops" });
    expect(parseNavigateRequest({ route: 3 })).toEqual({ route: null, message: null });
    expect(parseNavigateRequest(null)).toEqual({ route: null, message: null });
    expect(parseNavigateRequest("home")).toEqual({ route: null, message: null });
  });
});
