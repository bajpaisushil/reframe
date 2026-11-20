import { describe, expect, it } from "vitest";
import { splitActivityMessage } from "./activity";

describe("splitActivityMessage", () => {
  it("splits around the workspace name", () => {
    expect(splitActivityMessage("Created Deep work", "Deep work")).toEqual({ before: "Created ", after: "" });
    expect(splitActivityMessage("Updated Deep work from the current desktop", "Deep work")).toEqual({
      before: "Updated ",
      after: " from the current desktop",
    });
    expect(splitActivityMessage("Could not restore Deep work", "Deep work")).toEqual({
      before: "Could not restore ",
      after: "",
    });
  });

  it("uses the first occurrence", () => {
    expect(splitActivityMessage("Restored Work after Work", "Work")).toEqual({ before: "Restored ", after: " after Work" });
  });

  it("returns null when the message does not mention the workspace", () => {
    expect(splitActivityMessage("Captured 3 applications", "Deep work")).toBeNull();
    expect(splitActivityMessage("Duplicated from Deep work", "Deep work (copy)")).toBeNull();
  });

  it("returns null for an empty workspace name", () => {
    expect(splitActivityMessage("Created something", "")).toBeNull();
    expect(splitActivityMessage("Created something", "   ")).toBeNull();
  });
});
