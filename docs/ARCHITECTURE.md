# Reframe architecture

Reframe is a Tauri 2 desktop application: a React/TypeScript UI running in the system webview,
talking through typed Tauri commands to a Rust backend that owns all logic, OS integration and
persistence. Everything runs locally; there is no server, account or network dependency.

```
┌───────────────────────────────────────────────────────────────────────────┐
│ React UI (src/)                                                           │
│  features/ dashboard · desktop-explorer · workspaces · restoration ·      │
│            settings · portability · activity                              │
│  lib/api.ts (typed command client) · lib/events.ts (restore progress)     │
│  types/generated (TypeScript generated from the Rust models via ts-rs)    │
└───────────────▲───────────────────────────────────────┬───────────────────┘
                │ invoke() / events (reframe://restore)  │
┌───────────────┴───────────────────────────────────────▼───────────────────┐
│ Tauri command layer (src-tauri/src)                                       │
│  commands/* thin wrappers · events.rs · startup.rs (login restore)        │
│  plugins: dialog, autostart, single-instance                              │
└───────────────▲───────────────────────────────────────────────────────────┘
                │ ReframeService (facade)
┌───────────────┴───────────────────────────────────────────────────────────┐
│ reframe-core (src-tauri/crates/reframe-core) — no Tauri, no OS specifics  │
│  model/        workspaces, snapshots, runs, settings, export format       │
│  discovery/    platform data → applications with windows ("Explore")      │
│  capture/      selected applications → workspace entries                  │
│  adapters/     AppAdapter trait + registry: vscode, chromium, terminal,   │
│                file-manager, pdf, music, generic fallback                 │
│  restoration/  planner (DAG) · orchestrator · matcher · layout (monitors) │
│  switching/    Replace / Merge / Additive planning                        │
│  health.rs     restore-readiness report                                   │
│  database/     SQLite (rusqlite, bundled), migrations via user_version    │
│  portability/  JSON export/import, sanitization, path mapping             │
└───────────────▲───────────────────────────────────────────────────────────┘
                │ Platform trait
┌───────────────┴───────────────────────────────────────────────────────────┐
│ reframe-platform (src-tauri/crates/reframe-platform)                      │
│  win32/  Win32 + DWM + display config      macos/  AppKit + CG + AX       │
│  linux/  X11/EWMH via x11rb; Wayland detection with honest limits         │
│  common/ processes (sysinfo), detached launching, PATH lookup             │
│  mock.rs in-memory desktop used only by tests                             │
└───────────────────────────────────────────────────────────────────────────┘
```

## Why three crates

- **reframe-platform** contains every `unsafe` OS call behind one trait. It has no C dependencies,
  so it can be type-checked for Windows and macOS from any machine
  (`cargo check -p reframe-platform --target …`).
- **reframe-core** is pure Rust logic plus SQLite. Its tests run in seconds against the in-memory
  `MockPlatform` without building Tauri or a webview.
- **reframe** (the Tauri app) only maps commands to `ReframeService` calls and forwards events.

## Key data model decisions

- **Logical vs machine-specific data.** An `ApplicationEntry` stores a machine-independent
  `app_id` (`"vscode"`, `"chrome"`, `"exe:gimp"`) and adapter state, while `LaunchConfig`
  (executable, package, working directory) is machine-specific. Exports move `LaunchConfig` into a
  separate `MachineProfile`; imports resolve it again on the new machine, asking the user when
  needed.
- **Adapter state as reusable kinds.** `AppState` has variants such as `Projects`, `Browser`,
  `Terminal`, `Folders`, `Documents`, `Media` rather than one per application. A new IDE or browser
  reuses an existing kind, so adding an application never changes the engine.
- **Launch order is the list order.** Among entries that do not depend on each other, a lower index
  launches first. `depends_on` adds explicit ordering; the planner validates the graph (no cycles).
- **Documents in SQLite.** Workspaces, runs and activity are stored as versioned JSON documents with
  indexed metadata columns; `schema_version` allows explicit migrations of old documents.

## Restore pipeline

1. **Replace mode only:** close the windows the user confirmed — always a graceful request
   (`WM_CLOSE`, `_NET_CLOSE_WINDOW`, the AX close button). Windows that stay open (unsaved work)
   are reported, never killed.
2. **Plan:** topological order of enabled entries (workspace order breaks ties); cycles are errors.
3. **Validate:** each adapter builds launch steps; missing executables/directories fail that entry
   with a clear message.
4. **Reuse (Merge/Replace):** entries already satisfied by running windows are not relaunched; their
   windows are arranged.
5. **Launch:** independent entries run in parallel (bounded by `max_parallel`, staggered by
   `launch_delay_ms`); dependents wait for their dependencies; retries and timeouts per entry.
   A failed entry skips only its dependents.
6. **Match & place:** new windows are claimed per entry (so parallel launches never steal each
   other's windows), matched to saved snapshots, and placed — remapped proportionally when the
   saved monitor is missing or different.
7. **Report:** every status change is emitted as a `RestoreEvent`; the finished run is stored so
   failed entries can be retried later.

## Safety rules (enforced in code review)

- No force-killing processes, ever.
- Captured command lines are never stored or replayed; terminal startup commands run only after the
  user explicitly approves them, and imports reset that approval.
- Exports are sanitized (URL credentials and token-like parameters removed); imports are validated
  strictly (size, schema, limits, references).
- Unsupported platform operations return `PlatformError::Unsupported` with an explanation that the UI
  shows to the user.
