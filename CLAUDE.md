# Reframe — engineering notes

Reframe is an offline, cross-platform desktop app (Tauri 2 + React/TypeScript + Rust + SQLite)
that captures and restores working environments (apps, windows, projects, tabs, terminals).

## Layout

- `src/` — React + TypeScript UI (Vite).
  - `src/lib/api.ts` — typed client for **every** Tauri command. Components call this, never `invoke` directly.
  - `src/lib/events.ts` — restore progress events (`reframe://restore`).
  - `src/types/generated/` — ts-rs output. **Do not edit**; run `npm run bindings`.
  - `src/components/{ui,layout}`, `src/features/{dashboard,desktop-explorer,workspaces,restoration,settings,portability}`.
- `src-tauri/` — Tauri app, a thin command layer: `src/commands/*`, `events.rs`, `startup.rs`, `state.rs`.
- `src-tauri/crates/reframe-core/` — the engine, independent of Tauri:
  `model/` (shared data contracts), `adapters/` (per-app integrations + registry), `discovery/`, `capture/`,
  `restoration/` (planner, orchestrator, matcher, layout), `switching/`, `health.rs`, `database/` (SQLite),
  `portability/` (JSON import/export, sanitization), `service.rs` (facade used by the commands).
- `src-tauri/crates/reframe-platform/` — `Platform` trait + `linux/` (X11/EWMH via x11rb), `win32/`, `macos/`,
  `common/` helpers, `mock.rs` (test double behind the `mock` feature; never used by the app).

## Commands

```bash
# Rust (run inside src-tauri/)
cargo test -p reframe-core
cargo test -p reframe-platform --features mock
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p reframe-platform --target x86_64-pc-windows-gnu    # Windows type-check (MinGW installed)
cargo check -p reframe-platform --target aarch64-apple-darwin     # macOS type-check (platform crate only)

# Frontend (repo root)
npm run typecheck && npm run build
npm test                      # vitest
npm run bindings              # regenerate TS types after changing any #[derive(TS)] Rust type
npm run tauri dev             # run the app (needs a display)
```

Headless X11 (Linux): `Xvfb :99 -screen 0 1920x1080x24 &`, `DISPLAY=:99 openbox &`, then run apps/tests with
`DISPLAY=:99`. `xterm`, `xeyes`, `xclock`, `wmctrl`, `xdotool`, `xprop`, `xrandr` are installed. Screenshot:
`DISPLAY=:99 import -window root shot.png`. For WebKitGTK under Xvfb set `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

This dev machine shares one cargo target dir across git worktrees (machine-local `~/.cargo/config.toml`);
concurrent cargo builds wait on its lock — that is expected.

## Non-negotiable rules

- Never force-kill processes. Closing windows is always a graceful request; apps may refuse (unsaved work).
- Never store or replay captured command lines blindly; terminal startup commands run only if user-approved.
- Never export credentials/tokens; imports are validated strictly.
- Unsupported OS features return `PlatformError::Unsupported` with a human-readable reason — never panic,
  never fake success. Document platform limitations honestly.
- No fake/sample data in functional code paths. `MockPlatform` is for tests only.
- Fully offline: no network calls, telemetry, accounts or paid services.

## Conventions

- Types crossing into TypeScript derive `TS` with `#[ts(export)]` and serde `rename_all = "camelCase"`
  (tagged enums also need `rename_all_fields = "camelCase"`). Avoid `u64`/`i64` in them (they become `bigint`)
  unless annotated `#[ts(type = "number")]`.
- Unfinished code is marked `PLACEHOLDER` in its doc comment; remove the marker when implementing it.
- Shared contracts — `model/*`, the `Platform` and `AppAdapter` traits, `service.rs`, `src/lib/api.ts` — change
  only additively and deliberately.
- Rust: edition 2024, `cargo fmt`, clippy clean. Tests live next to the code (`#[cfg(test)]`).
- UI: CSS Modules + design tokens in `src/styles/tokens.css`; icons from `lucide-react`; no UI frameworks.
