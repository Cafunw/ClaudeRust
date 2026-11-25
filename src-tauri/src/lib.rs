//! ClaudeRust — a lightweight Claude desktop client written in Rust.

pub mod anthropic;
pub mod applog;
pub mod archmap;
pub mod artifact_ai;
pub mod artifact_docx;
pub mod artifacts;
pub mod claudecode;
pub mod attachments;
pub mod attachment_files;
pub mod checkpoints;
pub mod codetools;
pub mod code_exec;
pub mod commands;
pub mod compact;
pub mod connector_directory;
pub mod db;
pub mod deeplink;
pub mod devtools;
pub mod diagnostics;
pub mod display;
pub mod engine;
pub mod error;
pub mod extension_directory;
pub mod extension_sign;
pub mod extensions;
pub mod filesearch;
pub mod github_source;
pub mod gitops;
pub mod grepsearch;
pub mod highlight;
pub mod hooks;
pub mod import;
pub mod incognito;
#[cfg(windows)]
pub mod jobobject;
pub mod keepawake;
pub mod link_source;
pub mod logdigest;
pub mod markdown;
pub mod mcp;
pub mod mcp_oauth;
pub mod memory;
pub mod memory_cmds;
pub mod memory_synth;
pub mod models;
pub mod node_runtime;
pub mod oai;
pub mod office;
pub mod past_chats;
pub mod pathguard;
pub mod project_search;
pub mod prompts;
pub mod pullrequest;
pub mod remote_files;
pub mod render;
pub mod research;
pub mod routines;
pub mod scripts;
pub mod secrets;
pub mod settings;
pub mod shellenv;
pub mod slash;
pub mod shells;
pub mod sidebar_cmds;
pub mod skills;
pub mod state;
pub mod style_gen;
pub mod subagent;
pub mod symbols;
pub mod terminal;
pub mod todos;
pub mod tool_load;
pub mod turnstream;
pub mod updates;
pub mod usage;
pub mod util;
pub mod view;
pub mod watch;
pub mod webview_memory;
pub mod window_theme;
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

/// Closing the main window hides it (macOS apps stay open without windows)
/// instead of quitting.
fn hides_on_close(keep_in_tray: bool, tray_ok: bool) -> bool {
    (keep_in_tray && tray_ok) || cfg!(target_os = "macos")
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        display::ensure_on_screen(&w);
        let _ = w.show();
        display::ensure_on_screen_soon(&w);
        let _ = w.set_focus();
        webview_memory::sync(&w);
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
        .title(if turkish(app) { "Hızlı Giriş" } else { "Quick Entry" })
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

/// Whether the texts the backend shows itself are in Turkish (the UI's language setting).
pub(crate) fn turkish(app: &AppHandle) -> bool {
    app.try_state::<Arc<AppState>>().is_some_and(|s| s.turkish())
}

/// The tray menu's items (id, label); a separator goes before "quit".
fn tray_items(turkish: bool) -> [(&'static str, &'static str); 4] {
    let l = |en: &'static str, tr: &'static str| if turkish { tr } else { en };
    [
        ("open", l("Open ClaudeRust", "ClaudeRust’ı aç")),
        ("new", l("New Chat", "Yeni sohbet")),
        ("quick", l("Quick Entry", "Hızlı Giriş")),
        ("quit", l("Quit ClaudeRust", "ClaudeRust’tan çık")),
    ]
}

fn tray_menu(app: &AppHandle, turkish: bool) -> tauri::Result<Menu<tauri::Wry>> {
    let [open, new_chat, quick, quit] = tray_items(turkish).map(|(id, label)| MenuItem::with_id(app, id, label, true, None::<&str>));
    let sep = PredefinedMenuItem::separator(app)?;
    Menu::with_items(app, &[&open?, &new_chat?, &quick?, &sep, &quit?])
}

/// The tray menu again, in the UI's language (after the language setting changed).
pub fn retitle_tray(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main") else { return };
    match tray_menu(app, turkish(app)) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => log::warn!("tray menu: {e}"),
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = tray_menu(app, turkish(app))?;
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

fn artifact_protocol<R: tauri::Runtime>(app: &AppHandle<R>, req: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let token = req.uri().path().trim_start_matches('/').to_string();
    let st = app.state::<Arc<AppState>>();
    let found = st.served.lock().get(&token).cloned();
    match found {
        Some((mime, body)) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Security-Policy", artifacts::protocol_csp(&mime))
            .header("Content-Type", mime)
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
    // the settings can't be loaded the usual way: their row is read from the file alone
    let tr = settings::ui_turkish(&settings::saved_language(path));
    let (head, ask, (fresh_b, quit_b)) = if tr {
        ("ClaudeRust veritabanını açamadı:", "Yeni, boş bir veritabanıyla başlansın mı? Mevcut dosya yeniden adlandırılıp yanında saklanır; hiçbir şey silinmez.", ("Yeni veritabanıyla başla", "Çık"))
    } else {
        ("ClaudeRust couldn't open its database:", "Start with a new, empty database? The current file is kept next to it (renamed), so nothing is deleted.", ("Start with a new database", "Quit"))
    };
    let fresh = app
        .dialog()
        .message(format!("{head}\n{err}\n\n{}\n\n{ask}", path.display()))
        .title("ClaudeRust")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(fresh_b.into(), quit_b.into()))
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
                show_error(&if tr {
                    format!("{} kenara taşınamadı: {e}\n\nKlasörün yazılabilir olduğunu kontrol edin.", from.display())
                } else {
                    format!("Moving {} aside failed: {e}\n\nCheck that the folder is writable.", from.display())
                });
                app.exit(1);
                return;
            }
        }
    }
    app.restart();
}

pub fn run() {
    display::prepare();
    // a test copy never comes forward by itself (display::automation): not when another start
    // of it is refused, nor by restoring a window saved as visible (the plugin shows and focuses it)
    let test_copy = display::automation();
    let window_state = tauri_plugin_window_state::Builder::default().with_denylist(&["quick"]);
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(move |app, _args, _cwd| {
            if !test_copy {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(if test_copy { window_state.skip_initial_state("main") } else { window_state }.build())
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
            settings::set_app_identifier(&app.config().identifier);
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            applog::init(&data_dir.join("logs"));
            #[cfg(windows)]
            if let Some(job) = jobobject::start() {
                app.resources_table().add(job);
            }
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // also for a copy that no installer registered (portable, AppImage); a dev build must not take the links over
                #[cfg(any(windows, target_os = "linux"))]
                if !cfg!(debug_assertions) && deeplink::should_register(&app.config().identifier, deeplink::registered_command().as_deref()) {
                    if let Err(e) = app.deep_link().register_all() {
                        log::warn!("registering clauderust:// failed: {e}");
                    }
                }
                let h = handle.clone();
                app.deep_link().on_open_url(move |e| {
                    deeplink::open(&h, &e.urls());
                    show_main(&h);
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    deeplink::open(&handle, &urls);
                }
            }
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
            claudecode::sweep_prompt_files(&data_dir);
            // incognito chats a crash left (none of their turns can be running yet)
            incognito::sweep(&db, &data_dir);
            node_runtime::init(&data_dir);
            let st = Arc::new(AppState::new(db, data_dir, mcp_path));
            st.shells.set_app(app.handle().clone());
            devtools::start(app.handle());
            commands::start_routine_scheduler(app.handle().clone(), st.clone());
            updates::start_background_checks(app.handle().clone(), st.clone());
            extension_directory::start_update_checks(app.handle().clone(), st.clone());
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
            // MCP servers that change by themselves (restarted, given up, need a sign-in, new tools)
            {
                let st = st.clone();
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        st.mcp.changed().await;
                        let _ = handle.emit("mcp-changed", ());
                    }
                });
            }
            mcp::reload_on_outside_edits(handle.clone(), st.clone());

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
                let system_dark = w.theme().is_ok_and(|t| t == tauri::Theme::Dark);
                let _ = w.set_background_color(Some(window_theme::page_background(&s.theme, system_dark)));
                if display::automation() {
                    // a test copy must not appear on the person's screen or take focus; a window outside every monitor still renders
                    let _ = w.set_position(tauri::PhysicalPosition::new(-12000, -12000));
                    // show() activates the window on Windows and takes the person's focus: show it without activating
                    #[cfg(windows)]
                    if let Ok(h) = w.hwnd() {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE};
                        // SAFETY: the handle belongs to this live window
                        unsafe {
                            let ex = GetWindowLongPtrW(h.0 as _, GWL_EXSTYLE);
                            SetWindowLongPtrW(h.0 as _, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE as isize);
                            ShowWindow(h.0 as _, SW_SHOWNOACTIVATE);
                        }
                    }
                    #[cfg(not(windows))]
                    let _ = w.show();
                } else {
                    let _ = w.show();
                    let _ = w.set_focus();
                    display::ensure_on_screen_soon(&w);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            // minimised, hidden or shown again: WebView2's memory target follows (webview_memory.rs)
            if matches!(event, WindowEvent::Resized(_) | WindowEvent::Focused(_)) {
                if let Some(w) = window.app_handle().get_webview_window("main") {
                    webview_memory::sync(&w);
                }
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let keep = window
                    .app_handle()
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.read().keep_in_tray)
                    .unwrap_or(false);
                if hides_on_close(keep, TRAY_OK.load(std::sync::atomic::Ordering::SeqCst)) {
                    api.prevent_close();
                    let _ = window.hide();
                    if let Some(w) = window.app_handle().get_webview_window("main") {
                        webview_memory::sync(&w);
                    }
                } else {
                    // A hidden Quick Entry window would otherwise keep the
                    // process alive with nothing on screen.
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            webview_memory::trim_memory,
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
            commands::conversation_meta,
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
            attachment_files::attachment_data,
            attachment_files::attachment_save,
            attachment_files::attachment_open,
            attachment_files::artifact_read_file,
            commands::pick_files,
            commands::pick_directory,
            commands::save_text_file,
            code_exec::save_output_file,
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
            artifacts::artifact_edit,
            artifacts::export_artifact,
            artifact_ai::artifact_complete,
            artifact_ai::artifact_complete_check,
            claudecode::prewarm_chat,
            commands::artifact_storage,
            commands::artifact_storage_import,
            commands::render_markdown,
            commands::highlight_code,
            commands::mcp_status,
            commands::mcp_reload,
            commands::mcp_save_config,
            commands::mcp_list_prompts,
            commands::mcp_get_prompt,
            commands::mcp_read_resource,
            mcp_oauth::mcp_oauth_sign_in,
            mcp_oauth::mcp_oauth_sign_out,
            mcp::elicit::mcp_elicit_pending,
            mcp::elicit::mcp_elicit_respond,
            connector_directory::connector_directory,
            mcp::apps::mcp_app_info,
            mcp::apps::mcp_app_open,
            mcp::apps::mcp_app_call,
            mcp::artifact_tools::artifact_tool_check,
            mcp::artifact_tools::artifact_tool_call,
            skills::skills_list,
            skills::skill_set_enabled,
            skills::skill_pick_file,
            skills::skill_install_preview,
            skills::skill_install,
            skills::skill_uninstall,
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
            commands::usage_limits,
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
            commands::workflows_list,
            commands::workflow_stop,
            commands::workflows_clear,
            commands::claude_code_session_info,
            commands::claude_code_login,
            commands::claude_auth_status,
            commands::claude_auth_login,
            commands::claude_auth_login_cancel,
            commands::claude_auth_logout,
            commands::extension_inspect,
            commands::extension_install,
            extensions::extension_settings,
            extensions::extension_reconfigure,
            extensions::extension_uninstall,
            extension_directory::extension_directory,
            extension_directory::extension_directory_download,
            extension_directory::extension_directory_cancel,
            extension_directory::extension_directory_install,
            node_runtime::node_runtime_status,
            node_runtime::node_runtime_install,
            node_runtime::node_runtime_cancel,
            node_runtime::node_runtime_remove,
            import::import_scan,
            import::import_scan_sessions,
            import::import_mcp_servers,
            import::import_desktop_extensions,
            import::import_claude_code_sessions,
            import::import_claude_export,
            import::pick_import_file,
            incognito::close_incognito,
            sidebar_cmds::conversation_set_pinned,
            sidebar_cmds::conversation_set_archived,
            sidebar_cmds::conversation_set_unread,
            sidebar_cmds::conversation_set_section,
            sidebar_cmds::conversation_opened,
            sidebar_cmds::list_archived_conversations,
            sidebar_cmds::search_conversations,
            sidebar_cmds::sections_list,
            sidebar_cmds::section_create,
            sidebar_cmds::section_rename,
            sidebar_cmds::section_delete,
            memory_cmds::chat_memory_list,
            memory_cmds::chat_memory_get,
            memory_cmds::chat_memory_set,
            memory_cmds::chat_memory_delete,
            memory_cmds::chat_memory_delete_all,
            memory_cmds::chat_memory_import,
            memory_cmds::chat_memory_update_now,
            style_gen::style_from_samples,
            commands::open_external,
            commands::open_path,
            commands::reveal_path,
            commands::copy_text,
            diagnostics::log_tail,
            commands::set_zoom,
            commands::quick_entry_submit,
            commands::quick_entry_hide,
            commands::code_info,
            turnstream::turn_stream_protocol,
            turnstream::turn_stream_resync,
            deeplink::deep_link_take,
            commands::usage_stats,
            commands::reset_onboarding,
            github_source::github_list,
            github_source::github_add,
            github_source::github_sync,
            github_source::github_token_status,
            github_source::github_token_set,
            link_source::link_add,
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
                    // the chats' kept Claude Code processes (not in `runs`: no turn holds them)
                    claudecode::end_all_warm(&st).await;
                    // the incognito chats still open, once their turns stopped
                    incognito::sweep(&st.db, &st.data_dir);
                    st.mcp.stop_all().await
                });
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_menu_is_in_the_ui_language() {
        let ids = |tr| tray_items(tr).map(|(id, _)| id);
        // the ids the tray's menu handler acts on
        assert_eq!(ids(false), ["open", "new", "quick", "quit"]);
        assert_eq!(ids(true), ids(false));
        assert_eq!(tray_items(false).map(|(_, l)| l), ["Open ClaudeRust", "New Chat", "Quick Entry", "Quit ClaudeRust"]);
        assert_eq!(tray_items(true).map(|(_, l)| l), ["ClaudeRust’ı aç", "Yeni sohbet", "Hızlı Giriş", "ClaudeRust’tan çık"]);
    }

    #[test]
    fn closing_the_window_quits_unless_a_tray_keeps_the_app() {
        assert!(hides_on_close(true, true));
        // no tray (some Linux desktops): hiding would leave no way back
        assert_eq!(hides_on_close(true, false), cfg!(target_os = "macos"));
        assert_eq!(hides_on_close(false, true), cfg!(target_os = "macos"));
    }

    #[test]
    fn the_artifact_protocol_serves_kept_documents_with_their_security_headers() {
        let dir = std::env::temp_dir().join(format!("cr-lib-{}", util::new_id()));
        let st = Arc::new(AppState::new(db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        st.served.lock().insert("tok".into(), ("text/html".into(), b"<p>hi</p>".to_vec()));
        st.served.lock().insert("app".into(), (artifacts::MCP_APP_SERVED.into(), b"<p>app</p>".to_vec()));
        let app = tauri::test::mock_app();
        app.manage(st);
        let get = |path: &str| artifact_protocol(app.handle(), &tauri::http::Request::builder().uri(format!("artifact://localhost/{path}")).body(vec![]).unwrap());
        let r = get("tok");
        assert_eq!(r.status(), 200);
        let h = |r: &tauri::http::Response<Vec<u8>>, name: &str| r.headers()[name].to_str().unwrap().to_string();
        assert_eq!(h(&r, "Content-Type"), "text/html");
        assert_eq!(h(&r, "Content-Security-Policy"), artifacts::protocol_csp("text/html"));
        assert_eq!(h(&r, "Cache-Control"), "no-store");
        assert_eq!(h(&r, "X-Content-Type-Options"), "nosniff");
        assert_eq!(r.body(), b"<p>hi</p>");
        // an MCP app gets its own policy
        let r = get("app");
        assert_eq!(h(&r, "Content-Security-Policy"), artifacts::protocol_csp(artifacts::MCP_APP_SERVED));
        assert_ne!(artifacts::protocol_csp(artifacts::MCP_APP_SERVED), artifacts::protocol_csp("text/html"));
        for missing in ["other", "", "tok/x"] {
            let r = get(missing);
            assert_eq!(r.status(), 404, "{missing}");
            assert_eq!(h(&r, "Content-Type"), "text/plain");
            assert!(r.headers().get("Content-Security-Policy").is_none());
        }
    }
}
