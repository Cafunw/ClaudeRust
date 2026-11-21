//! ClaudeRust — a lightweight Claude desktop client written in Rust.

pub mod anthropic;
pub mod archmap;
pub mod artifacts;
pub mod claudecode;
pub mod attachments;
pub mod checkpoints;
pub mod codetools;
pub mod commands;
pub mod compact;
pub mod db;
pub mod devtools;
pub mod display;
pub mod engine;
pub mod error;
pub mod extensions;
pub mod filesearch;
pub mod gitops;
pub mod grepsearch;
pub mod highlight;
pub mod hooks;
pub mod import;
pub mod logdigest;
pub mod markdown;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod oai;
pub mod prompts;
pub mod pullrequest;
pub mod render;
pub mod routines;
pub mod scripts;
pub mod secrets;
pub mod settings;
pub mod shellenv;
pub mod slash;
pub mod shells;
pub mod state;
pub mod subagent;
pub mod symbols;
pub mod terminal;
pub mod todos;
pub mod turnstream;
pub mod updates;
pub mod util;
pub mod view;
pub mod watch;
pub mod worktree;

use state::AppState;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// False when the system has no tray (some Linux desktops): then closing the
/// window must quit, or the app would keep running with no way back.
static TRAY_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        display::ensure_on_screen(&w);
        let _ = w.show();
        display::ensure_on_screen_soon(&w);
        let _ = w.set_focus();
    }
}

pub fn hide_quick_entry(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("quick") {
        let _ = w.hide();
    }
}

fn toggle_quick_entry(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("quick") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
            return;
        }
        let _ = w.center();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("quick-entry-shown", ());
        return;
    }
    let builder = WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("quick.html".into()))
        .title("Quick Entry")
        .inner_size(700.0, 104.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .center()
        .focused(true);
    // Transparent windows can render black without GPU compositing (Remote
    // Desktop, software rendering): use an opaque window there.
    let opaque = display::info()["software"].as_bool().unwrap_or(false);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.transparent(!opaque);
    let builder = if opaque {
        builder.initialization_script("window.__QUICK_OPAQUE__ = true;")
    } else {
        builder
    };
    let built = builder.build();
    match built {
        Ok(w) => {
            let w2 = w.clone();
            w.on_window_event(move |e| {
                if let WindowEvent::Focused(false) = e {
                    let _ = w2.hide();
                }
            });
        }
        Err(e) => log::error!("quick entry window: {e}"),
    }
}

pub fn register_shortcut(app: &AppHandle, s: &settings::Settings) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if !s.quick_entry_enabled || s.quick_entry_shortcut.trim().is_empty() {
        return;
    }
    match s.quick_entry_shortcut.parse::<Shortcut>() {
        Ok(sc) => {
            if let Err(e) = gs.register(sc) {
                log::warn!("could not register shortcut {}: {e}", s.quick_entry_shortcut);
            }
        }
        Err(e) => log::warn!("invalid shortcut {}: {e}", s.quick_entry_shortcut),
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open ClaudeRust", true, None::<&str>)?;
    let new_chat = MenuItem::with_id(app, "new", "New Chat", true, None::<&str>)?;
    let quick = MenuItem::with_id(app, "quick", "Quick Entry", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit ClaudeRust", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &new_chat, &quick, &sep, &quit])?;
    let mut b = TrayIconBuilder::with_id("main")
        .tooltip("ClaudeRust")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "new" => {
                show_main(app);
                let _ = app.emit_to("main", "menu-new-chat", ());
            }
            "quick" => toggle_quick_entry(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    Ok(())
}

fn artifact_protocol(app: &AppHandle, req: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let token = req.uri().path().trim_start_matches('/').to_string();
    let st = app.state::<Arc<AppState>>();
    let found = st.served.lock().get(&token).cloned();
    match found {
        Some((mime, body)) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", mime)
            .header("Content-Security-Policy", artifacts::ARTIFACT_CSP)
            .header("Cache-Control", "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .body(body)
            .unwrap(),
        None => tauri::http::Response::builder()
            .status(404)
            .header("Content-Type", "text/plain")
            .body(b"artifact not found".to_vec())
            .unwrap(),
    }
}

/// The database couldn't be opened (damaged by a crash or a sync conflict,
/// written by an incompatible version, disk full): say so and offer to keep
/// the file aside and start with a new one, instead of failing silently on
/// every start. Runs on its own thread: the dialog needs the event loop.
fn offer_fresh_db(app: &AppHandle, path: &std::path::Path, err: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let show_error = |msg: &str| {
        app.dialog().message(msg).title("ClaudeRust").kind(MessageDialogKind::Error).blocking_show();
    };
    let fresh = app
        .dialog()
        .message(format!(
            "ClaudeRust couldn't open its database:\n{err}\n\n{}\n\nStart with a new, empty database? The current file is kept next to it (renamed), so nothing is deleted.",
            path.display()
        ))
        .title("ClaudeRust")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom("Start with a new database".into(), "Quit".into()))
        .blocking_show();
    if !fresh {
        app.exit(1);
        return;
    }
    let stamp = util::now_ms();
    for suffix in ["", "-wal", "-shm"] {
        let from = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if from.exists() {
            let to = std::path::PathBuf::from(format!("{}.unreadable-{stamp}{suffix}", path.display()));
            if let Err(e) = std::fs::rename(&from, &to) {
                show_error(&format!("Moving {} aside failed: {e}\n\nCheck that the folder is writable.", from.display()));
                app.exit(1);
                return;
            }
        }
    }
    app.restart();
}

pub fn run() {
    display::prepare();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["quick"])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _sc, ev| {
                    if ev.state() == ShortcutState::Pressed {
                        toggle_quick_entry(app);
                    }
                })
                .build(),
        )
        .register_uri_scheme_protocol("artifact", |ctx, req| artifact_protocol(ctx.app_handle(), &req))
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let mcp_path = config_dir.join("claude_desktop_config.json");
            let _ = mcp::ensure_config(&mcp_path);
            let db_path = data_dir.join("clauderust.db");
            let db = match db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    // nothing else starts (the window stays hidden); the user
                    // decides in a dialog, and the app restarts or quits
                    log::error!("opening {} failed: {e}", db_path.display());
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                    let app = app.handle().clone();
                    let err = e.to_string();
                    std::thread::spawn(move || offer_fresh_db(&app, &db_path, &err));
                    return Ok(());
                }
            };
            let _ = db.mark_interrupted();
            let _ = db.delete_unfinished_imports();
            let st = Arc::new(AppState::new(db, data_dir, mcp_path));
            st.shells.set_app(app.handle().clone());
            devtools::start(app.handle());
            commands::start_routine_scheduler(app.handle().clone(), st.clone());
            updates::start_background_checks(app.handle().clone(), st.clone());
            app.manage(st.clone());

            // Resolve the login-shell PATH, then start MCP servers.
            {
                let st = st.clone();
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = tokio::task::spawn_blocking(shellenv::init).await;
                    let disabled = st.settings.read().disabled_mcp_servers.clone();
                    st.mcp.start_all(&disabled).await;
                    let _ = handle.emit("mcp-changed", ());
                });
            }

            match build_tray(&handle) {
                Ok(()) => TRAY_OK.store(true, std::sync::atomic::Ordering::SeqCst),
                Err(e) => log::warn!("tray unavailable: {e}"),
            }
            let s = st.settings.read().clone();
            register_shortcut(&handle, &s);
            if let Some(w) = app.get_webview_window("main") {
                if (s.zoom - 1.0).abs() > 0.01 {
                    let _ = w.set_zoom(s.zoom);
                }
                display::ensure_on_screen(&w);
                let _ = w.show();
                let _ = w.set_focus();
                display::ensure_on_screen_soon(&w);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let keep = window
                    .app_handle()
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.read().keep_in_tray)
                    .unwrap_or(false);
                let tray = TRAY_OK.load(std::sync::atomic::Ordering::SeqCst);
                if (keep && tray) || cfg!(target_os = "macos") {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // A hidden Quick Entry window would otherwise keep the
                    // process alive with nothing on screen.
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::restart_app,
            commands::third_party_licenses,
            commands::update_check,
            commands::update_install,
            commands::get_settings,
            commands::save_settings,
            commands::set_api_key,
            commands::test_connection,
            commands::get_models,
            commands::list_conversations,
            commands::create_conversation,
            commands::get_conversation,
            commands::send_message,
            commands::regenerate,
            commands::switch_branch,
            commands::stop_generation,
            commands::update_conversation,
            commands::delete_conversation,
            commands::message_feedback,
            commands::export_conversation,
            commands::search,
            commands::stage_paths,
            commands::stage_bytes,
            commands::stage_text,
            commands::unstage,
            commands::pick_files,
            commands::pick_directory,
            commands::save_text_file,
            commands::respond_permission,
            commands::list_projects,
            commands::create_project,
            commands::get_project,
            commands::update_project,
            commands::delete_project,
            commands::add_project_paths,
            commands::add_project_text,
            commands::add_project_bytes,
            commands::remove_project_file,
            commands::list_artifacts,
            commands::delete_artifact,
            commands::serve_artifact,
            commands::render_markdown,
            commands::highlight_code,
            commands::mcp_status,
            commands::mcp_reload,
            commands::mcp_save_config,
            commands::mcp_list_prompts,
            commands::mcp_get_prompt,
            commands::mcp_read_resource,
            commands::pick_extension,
            commands::claude_code_info,
            commands::shells_list,
            commands::fork_conversation,
            commands::open_in_editor,
            commands::context_breakdown,
            commands::git_log,
            commands::git_show,
            commands::git_revert,
            commands::usage_overview,
            commands::oai_refresh,
            commands::arch_map,
            commands::commit_message_suggest,
            commands::git_commit,
            commands::watch_start,
            commands::watch_stop,
            commands::watch_status,
            commands::watch_run,
            commands::watch_auto_fix,
            commands::routines_list,
            commands::routine_save,
            commands::routine_delete,
            commands::routine_run,
            commands::preview_urls,
            commands::doctor,
            commands::subagents_list,
            commands::subagent_output,
            commands::subagent_stop,
            commands::subagents_clear,
            commands::hooks_info,
            commands::hooks_trust,
            commands::compact_conversation,
            commands::memory_files,
            commands::memory_save,
            commands::memory_append,
            commands::plan_state,
            commands::rewind,
            commands::rewind_info,
            commands::rewind_undo,
            commands::worktree_apply,
            commands::worktree_commit,
            commands::file_search,
            commands::symbol_search,
            commands::project_dir,
            commands::project_file,
            commands::scan_todos,
            commands::open_system_terminal,
            commands::file_sessions,
            commands::secret_scan,
            commands::change_revert_hunk,
            commands::git_branches,
            commands::git_switch,
            commands::apply_patch,
            commands::changes_patch,
            commands::git_stash,
            commands::git_stash_count,
            commands::project_scripts,
            commands::cc_open_in_terminal,
            commands::file_blame,
            commands::search_in_files,
            commands::prompt_history,
            commands::pr_info,
            commands::pr_suggest,
            commands::pr_create,
            commands::pr_fetch,
            commands::settings_export,
            commands::settings_import,
            commands::improve_prompt,
            commands::slash_list,
            commands::slash_expand,
            commands::slash_save,
            commands::changes_list,
            commands::change_diff,
            commands::change_revert,
            commands::term_open,
            commands::term_write,
            commands::term_resize,
            commands::term_close,
            commands::term_list,
            devtools::dev_report,
            commands::shell_output,
            commands::shell_kill,
            commands::shells_clear,
            commands::claude_code_login,
            commands::extension_inspect,
            commands::extension_install,
            import::import_scan,
            import::import_mcp_servers,
            import::import_desktop_extensions,
            import::import_claude_code_sessions,
            import::import_claude_export,
            import::pick_import_file,
            commands::open_external,
            commands::open_path,
            commands::reveal_path,
            commands::copy_text,
            commands::set_zoom,
            commands::quick_entry_submit,
            commands::quick_entry_hide,
            commands::code_info,
            commands::usage_stats,
            commands::reset_onboarding,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building ClaudeRust");
    app.run(|handle, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(handle),
        RunEvent::Exit => {
            if let Some(st) = handle.try_state::<Arc<AppState>>() {
                let st = st.inner().clone();
                // stop running turns too: their commands (a server the agent
                // started, the Claude Code CLI) run in their own process
                // groups and would outlive the app
                for c in st.runs.lock().values() {
                    c.cancel();
                }
                st.shells.kill_all();
                st.terminals.close_all();
                st.watches.stop_all();
                tauri::async_runtime::block_on(async move {
                    // the cancelled tasks kill their process groups on the
                    // runtime's threads; give them a moment before exiting
                    let start = std::time::Instant::now();
                    while (!st.runs.lock().is_empty() || st.shells.any_running()) && start.elapsed() < std::time::Duration::from_millis(2500) {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    st.mcp.stop_all().await
                });
            }
        }
        _ => {}
    });
}
