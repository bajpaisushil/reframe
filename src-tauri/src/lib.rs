//! Reframe desktop application: Tauri shell and command layer over `reframe-core`.

mod commands;
mod error;
mod events;
mod startup;
mod state;

use reframe_core::ReframeService;
use reframe_core::database::Database;
use tauri::Manager;

use crate::state::AppState;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let launched_at_login = std::env::args().any(|a| a == startup::AUTOSTART_FLAG);

    tauri::Builder::default()
        // Must be registered first: a second launch focuses the existing window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![startup::AUTOSTART_FLAG]),
        ))
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let db_path = data_dir.join("reframe.db");
            log::info!("using database {}", db_path.display());
            let db = Database::open(&db_path)?;
            let version = app.package_info().version.to_string();
            let service = ReframeService::new(reframe_platform::native(), db, version);
            app.manage(AppState::new(service, launched_at_login));
            startup::on_launch(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::desktop::get_platform_info,
            commands::desktop::get_permissions,
            commands::desktop::request_permission,
            commands::desktop::scan_desktop,
            commands::desktop::get_app_icon,
            commands::workspaces::list_workspaces,
            commands::workspaces::get_workspace,
            commands::workspaces::create_workspace,
            commands::workspaces::capture_workspace,
            commands::workspaces::recapture_workspace,
            commands::workspaces::capture_applications,
            commands::workspaces::create_manual_entry,
            commands::workspaces::update_workspace,
            commands::workspaces::delete_workspace,
            commands::workspaces::duplicate_workspace,
            commands::workspaces::get_workspace_health,
            commands::restore::plan_switch,
            commands::restore::start_restore,
            commands::restore::cancel_restore,
            commands::restore::get_active_runs,
            commands::restore::get_restore_run,
            commands::restore::list_restore_runs,
            commands::settings::get_activity,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::get_active_workspace,
            commands::settings::get_startup_context,
            commands::settings::dismiss_startup_restore,
            commands::portability::export_workspace,
            commands::portability::preview_import,
            commands::portability::commit_import,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Reframe");
}
