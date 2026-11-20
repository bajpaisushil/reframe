//! System tray menu: open Reframe, restore any workspace in one click, capture the desktop.
//!
//! The tray is a convenience; if the desktop has no tray host (e.g. GNOME without the
//! AppIndicator extension) Reframe works exactly the same without it.

use reframe_core::model::{RestoreRequest, SwitchMode};
use serde::Serialize;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};
use uuid::Uuid;

use crate::events::restore_sink;
use crate::state::AppState;

const TRAY_ID: &str = "reframe-tray";
/// Max workspaces listed in the tray menu.
const MAX_TRAY_WORKSPACES: usize = 15;

/// Asks the UI to show a page (`{ route, runId? }`).
pub const NAVIGATE_EVENT: &str = "reframe://navigate";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateRequest {
    pub route: &'static str,
    pub run_id: Option<Uuid>,
    pub message: Option<String>,
}

pub fn init(app: &AppHandle) {
    let result = (|| -> tauri::Result<()> {
        let menu = build_menu(app)?;
        let mut builder = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip("Reframe")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        builder.build(app)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("system tray unavailable: {e}");
    }
}

/// Rebuild the menu after workspaces changed.
pub fn refresh(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        match build_menu(app) {
            Ok(menu) => {
                if let Err(e) = tray.set_menu(Some(menu)) {
                    log::warn!("could not update tray menu: {e}");
                }
            }
            Err(e) => log::warn!("could not build tray menu: {e}"),
        }
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let workspaces = app
        .state::<AppState>()
        .service
        .list_workspaces()
        .unwrap_or_default();
    let open = MenuItem::with_id(app, "open", "Open Reframe", true, None::<&str>)?;
    let capture = MenuItem::with_id(app, "capture", "Capture desktop…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Reframe", true, None::<&str>)?;
    let separator_a = PredefinedMenuItem::separator(app)?;
    let separator_b = PredefinedMenuItem::separator(app)?;

    let workspace_items: Vec<MenuItem<Wry>> = workspaces
        .iter()
        .take(MAX_TRAY_WORKSPACES)
        .map(|ws| {
            let label = if ws.is_active {
                format!("{}  {}  ✓", ws.icon, ws.name)
            } else {
                format!("{}  {}", ws.icon, ws.name)
            };
            MenuItem::with_id(app, format!("restore:{}", ws.id), label, true, None::<&str>)
        })
        .collect::<tauri::Result<_>>()?;
    let restore = if workspace_items.is_empty() {
        let empty = MenuItem::with_id(app, "none", "No saved workspaces yet", false, None::<&str>)?;
        Submenu::with_items(
            app,
            "Restore workspace",
            true,
            &[&empty as &dyn IsMenuItem<Wry>],
        )?
    } else {
        let refs: Vec<&dyn IsMenuItem<Wry>> = workspace_items
            .iter()
            .map(|i| i as &dyn IsMenuItem<Wry>)
            .collect();
        Submenu::with_items(app, "Restore workspace", true, &refs)?
    };

    Menu::with_items(
        app,
        &[&open, &separator_a, &restore, &capture, &separator_b, &quit],
    )
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main_window(app),
        "capture" => {
            show_main_window(app);
            navigate(
                app,
                NavigateRequest {
                    route: "explore",
                    run_id: None,
                    message: None,
                },
            );
        }
        "quit" => app.exit(0),
        other => {
            if let Some(id) = other
                .strip_prefix("restore:")
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                restore_from_tray(app, id);
            }
        }
    }
}

/// Tray restores use Merge: it never closes anything, so it is safe without a confirmation step.
fn restore_from_tray(app: &AppHandle, workspace_id: Uuid) {
    let service = app.state::<AppState>().service.clone();
    let request = RestoreRequest {
        workspace_id,
        mode: SwitchMode::Merge,
        entry_ids: None,
        close_window_ids: Vec::new(),
    };
    show_main_window(app);
    match service.start_restore(request, restore_sink(app)) {
        Ok(run_id) => navigate(
            app,
            NavigateRequest {
                route: "restore",
                run_id: Some(run_id),
                message: None,
            },
        ),
        Err(e) => navigate(
            app,
            NavigateRequest {
                route: "home",
                run_id: None,
                message: Some(e.to_string()),
            },
        ),
    }
}

fn navigate(app: &AppHandle, request: NavigateRequest) {
    if let Err(e) = app.emit(NAVIGATE_EVENT, &request) {
        log::warn!("could not emit navigation event: {e}");
    }
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
