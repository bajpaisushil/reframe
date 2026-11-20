import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "@/components/ui";
import type { ImportPreview } from "@/types";
import { ImportReview } from "./ImportPage";

// Test fixture only: the shape the backend's preview_import returns.
const preview: ImportPreview = {
  filePath: "/home/me/Downloads/development.reframe.json",
  name: "Development",
  description: "Daily coding setup",
  icon: "💻",
  sourceOs: "windows",
  exportedAt: "2026-11-02T09:15:00Z",
  applications: [
    {
      entryId: "e1",
      appId: "vscode",
      displayName: "Visual Studio Code",
      adapterId: "vscode",
      resolution: { kind: "found", target: { kind: "executable", path: "/usr/bin/code" } },
      alternatives: [{ kind: "executable", path: "/snap/bin/code" }],
      missingPaths: ["C:\\Users\\me\\code\\reframe"],
    },
    {
      entryId: "e2",
      appId: "exe:gimp",
      displayName: "GIMP",
      adapterId: "generic",
      resolution: { kind: "unresolved" },
      alternatives: [],
      missingPaths: [],
    },
  ],
  suggestedPathMappings: [{ fromPrefix: "C:\\Users\\me\\code", toPrefix: "/home/me/code" }],
  warnings: ["Window positions were saved on two monitors."],
};

function render(): string {
  return renderToStaticMarkup(
    <ToastProvider>
      <ImportReview preview={preview} />
    </ToastProvider>,
  );
}

describe("ImportReview", () => {
  const html = render();

  it("shows the workspace and where it came from", () => {
    expect(html).toContain('value="Development"');
    expect(html).toContain("Exported from Windows");
    expect(html).toContain("Daily coding setup");
    expect(html).toContain("Window positions were saved on two monitors.");
  });

  it("lists each app with how it was resolved", () => {
    expect(html).toContain("Visual Studio Code");
    expect(html).toContain("Found");
    expect(html).toContain("/snap/bin/code");
    expect(html).toContain("GIMP");
    expect(html).toContain("Unresolved");
    expect(html).toContain("No program chosen yet");
  });

  it("pre-fills the suggested folder mapping and marks covered paths", () => {
    expect(html).toContain('value="C:\\Users\\me\\code"');
    expect(html).toContain('value="/home/me/code"');
    expect(html).toContain("Mapped");
  });

  it("summarizes what will be imported", () => {
    expect(html).toContain("2 apps will be imported");
    expect(html).toContain("1 without a program");
  });
});
