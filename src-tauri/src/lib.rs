mod offline;
mod vault;
mod picker;
mod secrets;
mod selection;
mod settings;
mod translate;

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static CAPTURING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Clone)]
struct SelectionPayload {
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateResponse {
    mt_text: String,
    mt_service: String,
    mt_error: String,
    ai_text: String,
    ai_service: String,
    ai_error: String,
    target: String,
    direction_label: String,
    cancelled: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    ticket: u64,
    text: String,
}

fn settings_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_config_dir().map_err(|err| err.to_string())
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> settings::Settings {
    settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    settings::save(&settings_dir(&app)?, &settings)
}

#[tauri::command]
fn save_secret(
    app: tauri::AppHandle,
    name: String,
    key: String,
    password: String,
) -> Result<(), String> {
    let dir = settings_dir(&app)?;
    vault::save_key(&dir, &name, &key, &password)?;
    secrets::discard(&name);
    settings::set_key_present(&dir, &name, !key.trim().is_empty())
}

#[tauri::command]
async fn translate_text(
    app: tauri::AppHandle,
    text: String,
    mt_source: Option<String>,
    ai_source: Option<String>,
    side: Option<String>,
    style: Option<String>,
    direction: Option<String>,
    ticket: Option<u64>,
    master_password: Option<String>,
) -> Result<TranslateResponse, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可翻译的文字。".into());
    }
    let stored = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    let mt_source = mt_source.unwrap_or_else(|| stored.mt_source.clone());
    let ai_source = ai_source.unwrap_or_else(|| stored.ai_source.clone());
    let style = style.unwrap_or(stored.style.clone());
    let direction = direction.unwrap_or(stored.direction);
    let target = translate::resolve_target(text, &direction);
    let needs_saved_key = matches!(ai_source.as_str(), "deepseek" | "deepl")
        && side.as_deref() != Some("mt");
    let keys = if needs_saved_key {
        let dir = settings_dir(&app)?;
        let (deepl, deepseek) = vault::load_keys(&dir, master_password.as_deref().unwrap_or(""))?;
        translate::ProviderKeys { deepl, deepseek }
    } else {
        translate::ProviderKeys {
            deepl: String::new(),
            deepseek: String::new(),
        }
    };
    let model_dir = settings_dir(&app)
        .unwrap_or_else(|_| std::env::temp_dir().join("Vela"))
        .join("models");
    let direction_label = translate::direction_label(target).to_string();
    let ticket = ticket.unwrap_or(0);
    let style_ref = if style == "none" { None } else { Some(style.as_str()) };
    let (mt_text, mt_service, mt_error, ai_text, ai_service, ai_error) = match side.as_deref() {
        Some("mt") => {
            let app_status = app.clone();
            let (text, service, error) = translate::translate_mt(
                text,
                target,
                &mt_source,
                &model_dir,
                &move |status| {
                    let _ = app_status.emit(
                        "mt-status",
                        ProgressPayload {
                            ticket,
                            text: status,
                        },
                    );
                },
            )
            .await;
            (text, service, error, String::new(), String::new(), String::new())
        }
        Some("ai") => {
            let app_chunk = app.clone();
            let (text, service, error) = translate::translate_ai(
                text,
                target,
                &ai_source,
                style_ref,
                &keys,
                &move |chunk| {
                    let _ = app_chunk.emit(
                        "ai-chunk",
                        ProgressPayload {
                            ticket,
                            text: chunk,
                        },
                    );
                },
            )
            .await;
            (String::new(), String::new(), String::new(), text, service, error)
        }
        _ => {
            let compared = translate::translate_compare(
                text,
                target,
                &mt_source,
                &ai_source,
                style_ref,
                &keys,
                &model_dir,
            )
            .await;
            (
                compared.mt_text,
                compared.mt_service,
                compared.mt_error,
                compared.ai_text,
                compared.ai_service,
                compared.ai_error,
            )
        }
    };
    let cancelled = mt_service == "cancelled" || ai_service == "cancelled";
    Ok(TranslateResponse {
        mt_text,
        mt_service,
        mt_error,
        ai_text,
        ai_service,
        ai_error,
        target: target.to_string(),
        direction_label,
        cancelled,
    })
}

fn dismiss_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

fn hide_result(app: &tauri::AppHandle) {
    translate::cancel_ai();
    if let Some(window) = app.get_webview_window("result") {
        let _ = window.hide();
    }
    let _ = app.global_shortcut().unregister(dismiss_shortcut());
    let _ = app.emit("result-dismissed", ());
}

fn arm_dismiss(app: &tauri::AppHandle) {
    let _ = app.global_shortcut().unregister(dismiss_shortcut());
    let _ = app.global_shortcut().register(dismiss_shortcut());
}

#[tauri::command]
fn dismiss(app: tauri::AppHandle) {
    hide_result(&app);
}

fn place_and_show(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("result") else {
        return;
    };
    if let Ok((x, y)) = selection::cursor_pos() {
        let size = window
            .outer_size()
            .unwrap_or(tauri::PhysicalSize::new(440, 280));
        let (px, py) =
            selection::clamp_near_cursor(x, y, size.width as i32, size.height as i32);
        let _ = window.set_position(PhysicalPosition::new(px, py));
    }
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    arm_dismiss(app);
}

pub(crate) fn show_captured(app: &tauri::AppHandle, text: String) {
    picker::hide();
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        place_and_show(&app);
        let _ = app.emit("selection-captured", SelectionPayload { text });
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

pub(crate) fn on_hotkey(app: &tauri::AppHandle) {
    picker::hide();
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let captured = tauri::async_runtime::spawn_blocking(selection::capture_selection)
            .await
            .unwrap_or_else(|err| Err(err.to_string()));
        match captured {
            Ok(text) => {
                place_and_show(&app);
                let _ = app.emit("selection-captured", SelectionPayload { text });
            }
            Err(err) => {
                place_and_show(&app);
                let _ = app.emit("selection-error", err);
            }
        }
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            place_and_show(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let wanted = Shortcut::new(
                        Some(Modifiers::CONTROL | Modifiers::ALT),
                        Code::KeyT,
                    );
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let dismiss_key = dismiss_shortcut();
                    if shortcut == &dismiss_key {
                        hide_result(app);
                        return;
                    }
                    if shortcut == &wanted {
                        on_hotkey(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let autostart = CheckMenuItem::with_id(
                app,
                "autostart",
                "开机启动",
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
            let autostart_item = autostart.clone();
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&autostart, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("Vela")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    } else if event.id.as_ref() == "autostart" {
                        let autolaunch = app.autolaunch();
                        let enabled = autolaunch.is_enabled().unwrap_or(false);
                        let updated = if enabled {
                            autolaunch.disable()
                        } else {
                            autolaunch.enable()
                        };
                        if updated.is_ok() {
                            let _ = autostart_item.set_checked(!enabled);
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        place_and_show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            picker::start(app.handle().clone());

            let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyT);
            if let Err(err) = app.global_shortcut().register(shortcut) {
                let message = format!("快捷键 Ctrl+Alt+T 已被占用。{err}");
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    place_and_show(&handle);
                    let _ = handle.emit("hotkey-error", message);
                });
            }

            if let Some(window) = app.get_webview_window("result") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_result(&app_handle);
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            translate_text,
            get_settings,
            save_settings,
            save_secret,
            dismiss
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
