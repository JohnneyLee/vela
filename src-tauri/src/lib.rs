mod capture;
mod dpapi;
mod offline;
mod picker;
mod secrets;
mod selection;
mod settings;
mod speech;
mod suggest;
mod translate;
mod windows;
mod writeback;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// 让「设置页开关」和「托盘菜单勾选」能互相改对方：保存托盘里的
/// CheckMenuItem，命令里直接改它的勾选状态。
struct AutostartState(Mutex<Option<CheckMenuItem<tauri::Wry>>>);

#[derive(Serialize, Clone)]
struct SelectionPayload {
    text: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    editable: bool,
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

/// 打开结果窗时的默认动作：由 suggest() 按文字内容和母语决定，
/// 不读取、不沿用上一次使用过的动作。
fn recommend_action(app: &tauri::AppHandle, text: &str) -> String {
    let editable = capture::is_editable_exe(&writeback::target::source_exe());
    let native = settings_dir(app)
        .map(|dir| settings::load(&dir).native)
        .unwrap_or_else(|_| "zh".into());
    let native = suggest::native_lang(&native);
    suggest::recommend_id(text, editable, native).to_string()
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> settings::Settings {
    settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    let theme = settings.theme.clone();
    settings::save(&settings_dir(&app)?, &settings)?;
    // 主题变化后广播给所有窗口（bar / panel / settings），让它们即时切换。
    let _ = app.emit("theme-changed", theme);
    Ok(())
}

#[tauri::command]
fn save_key(app: tauri::AppHandle, name: String, key: String) -> Result<(), String> {
    settings::save_key(&settings_dir(&app)?, &name, &key)
}

/// 开机启动当前是否开启。
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 设置页开关：改开机启动，并同步托盘勾选与设置页其它窗口。
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let autolaunch = app.autolaunch();
    let updated = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    updated.map_err(|err| err.to_string())?;
    let actual = autolaunch.is_enabled().unwrap_or(false);
    if let Some(state) = app.try_state::<AutostartState>() {
        if let Ok(guard) = state.0.lock() {
            if let Some(item) = guard.as_ref() {
                let _ = item.set_checked(actual);
            }
        }
    }
    let _ = app.emit("autostart-changed", actual);
    Ok(actual)
}

#[tauri::command]
fn delete_key(app: tauri::AppHandle, name: String) -> Result<(), String> {
    settings::delete_key(&settings_dir(&app)?, &name);
    Ok(())
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
    mode: Option<String>,
    polish_level: Option<String>,
    ticket: Option<u64>,
) -> Result<TranslateResponse, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可翻译的文字。".into());
    }
    let stored = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    let mut mt_source = mt_source.unwrap_or_else(|| stored.mt_source.clone());
    let mut ai_source = ai_source.unwrap_or_else(|| stored.ai_source.clone());
    // 完全本地模式：强制离线翻译 + Ollama，不发任何请求到外网。
    if stored.local_only {
        mt_source = "offline".into();
        ai_source = "ollama".into();
    }
    let style = style.unwrap_or(stored.style.clone());
    let direction = direction.unwrap_or_else(|| "auto".into());
    let mode = mode.unwrap_or_else(|| "translate".into());
    let polish_level = polish_level.unwrap_or_else(|| "standard".into());
    // 手动方向（zh-en / en-zh）只对这一次有效；"auto" 表示按母语规则自动判断。
    let manual = match direction.as_str() {
        "zh-en" | "en-zh" => Some(direction.as_str()),
        _ => None,
    };
    let native = stored.native.clone();
    let target = translate::resolve_target(text, &native, manual);
    let keys = translate::ProviderKeys {
        deepl: stored.deepl_key,
        deepseek: stored.deepseek_key,
        openai: stored.openai_key,
        moonshot: stored.moonshot_key,
        gemini: stored.gemini_key,
        ollama_model: stored.ollama_model,
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
                &keys.deepl,
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
                &mode,
                &polish_level,
                &native,
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
                &mode,
                &polish_level,
                &native,
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
    if let Some(window) = app.get_webview_window("panel") {
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

#[tauri::command]
fn bar_pick(app: tauri::AppHandle, action: String) {
    windows::hide_bar(&app);
    let allow_partial = settings_dir(&app)
        .map(|dir| settings::load(&dir).allow_partial_clipboard)
        .unwrap_or(false);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Clicking the bar may have given the bar's WebView focus. Switch back
        // to the source window (recorded when the bar appeared) before capturing,
        // so UI Automation / simulated copy target the app the text was in.
        let captured = tauri::async_runtime::spawn_blocking(move || {
            let _ = writeback::target::switch_back_and_verify();
            capture::capture_allow_partial(allow_partial)
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()));
        match captured {
            Ok(text) => {
                place_and_show(&app);
                let editable = capture::is_editable_exe(&writeback::target::source_exe());
                let _ = app.emit(
                    "selection-captured",
                    SelectionPayload { text, action, editable },
                );
            }
            Err(err) => {
                place_and_show(&app);
                let _ = app.emit("selection-error", err);
            }
        }
    });
}

#[tauri::command]
fn bar_hide(app: tauri::AppHandle) {
    windows::hide_bar(&app);
}

#[tauri::command]
fn open_settings(app: tauri::AppHandle) {
    windows::open_settings(&app);
}

#[tauri::command]
fn settings_close(app: tauri::AppHandle) {
    windows::close_settings(&app);
}

#[tauri::command]
fn panel_resize(app: tauri::AppHandle, height: f64) -> Result<(), String> {
    let Some(window) = app.get_webview_window("panel") else {
        return Ok(());
    };
    let clamped = height.clamp(160.0, 4000.0);
    // `height` from the frontend is in logical (CSS) pixels; set_size with
    // LogicalSize lets Tauri do the DPI conversion. Keep the current logical
    // width (the user may have resized it).
    let scale = window.scale_factor().unwrap_or(1.0);
    let current = window
        .inner_size()
        .unwrap_or(tauri::PhysicalSize::new(440, 400));
    let width_logical = current.width as f64 / scale;
    let _ = window.set_size(tauri::LogicalSize::new(width_logical, clamped));
    Ok(())
}

#[tauri::command]
async fn speak(text: String, lang: Option<String>) -> Result<(), String> {
    let lang = lang.unwrap_or_else(|| "auto".into());
    let result = tauri::async_runtime::spawn_blocking(move || speech::speak_with_lang(&text, &lang)).await;
    match result {
        Ok(res) => res,
        Err(err) => Err(format!("朗读线程出错：{err}")),
    }
}

#[tauri::command]
fn replace_text(text: String, original: String) -> Result<(), String> {
    let exe = writeback::target::source_exe();
    writeback::write_ready(&text, &exe)?;
    writeback::undo::store(original);
    Ok(())
}

#[tauri::command]
fn undo_text() -> Result<Option<String>, String> {
    writeback::undo::undo()
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    capture::clipboard::set_text(&text)
}

#[tauri::command]
async fn explain_diff(app: tauri::AppHandle, original: String, polished: String) -> Result<String, String> {
    let stored = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    let keys = translate::ProviderKeys {
        deepl: stored.deepl_key,
        deepseek: stored.deepseek_key,
        openai: stored.openai_key,
        moonshot: stored.moonshot_key,
        gemini: stored.gemini_key,
        ollama_model: stored.ollama_model,
    };
    translate::explain_diff(&original, &polished, &stored.ai_source, &keys, &stored.native).await
}

#[tauri::command]
async fn test_ai_connection(
    app: tauri::AppHandle,
    source: String,
    key: Option<String>,
) -> Result<String, String> {
    let stored = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    let keys = translate::ProviderKeys {
        deepl: stored.deepl_key,
        deepseek: stored.deepseek_key,
        openai: stored.openai_key,
        moonshot: stored.moonshot_key,
        gemini: stored.gemini_key,
        ollama_model: stored.ollama_model,
    };
    let override_key = key.unwrap_or_default();
    translate::test_connection(&source, &override_key, &keys).await
}

#[tauri::command]
async fn detect_ollama() -> Result<Vec<String>, String> {
    translate::detect_ollama().await
}

fn place_and_show(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("panel") else {
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
    writeback::target::remember();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        place_and_show(&app);
        let editable = capture::is_editable_exe(&writeback::target::source_exe());
        let action = recommend_action(&app, &text);
        let _ = app.emit(
            "selection-captured",
            SelectionPayload { text, action, editable },
        );
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

pub(crate) fn on_hotkey(app: &tauri::AppHandle) {
    picker::hide();
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }
    writeback::target::remember();
    let allow_partial = settings_dir(app)
        .map(|dir| settings::load(&dir).allow_partial_clipboard)
        .unwrap_or(false);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let captured = tauri::async_runtime::spawn_blocking(move || {
            match capture::capture_allow_partial(allow_partial) {
                Ok(text) => Ok(text),
                Err(_) => {
                    // No selection captured: for chat apps, select the whole
                    // input field and try once more.
                    let exe = writeback::target::source_exe();
                    let pol = writeback::policy::for_exe(&exe);
                    if pol.select_all {
                        use ::windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VIRTUAL_KEY};
                        capture::input::send_chord(&[VK_CONTROL], VIRTUAL_KEY(0x41));
                        capture::capture()
                    } else {
                        Err("没有取到选中文字。请先选中要处理的文字。".into())
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()));
        match captured {
            Ok(text) => {
                place_and_show(&app);
                let editable = capture::is_editable_exe(&writeback::target::source_exe());
                let action = recommend_action(&app, &text);
                let _ = app.emit(
                    "selection-captured",
                    SelectionPayload { text, action, editable },
                );
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
            // 供 set_autostart 命令更新托盘勾选状态。
            app.manage(AutostartState(Mutex::new(Some(autostart.clone()))));
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &autostart, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("Vela")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    } else if event.id.as_ref() == "settings" {
                        windows::open_settings(app);
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
                            // 同步给设置页的开关。
                            let _ = app.emit("autostart-changed", !enabled);
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        // 左键点击托盘：在光标处呼出动作条，作为无快捷键时的入口。
                        if let Ok((x, y)) = selection::cursor_pos() {
                            windows::show_bar(tray.app_handle(), x, y, "translate".into());
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            picker::start(app.handle().clone());

            // Make the bar non-activating so it never steals focus.
            if let Some(bar) = windows::bar(app.handle()) {
                windows::apply_no_activate(&bar);
            }

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

            if let Some(window) = app.get_webview_window("panel") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_result(&app_handle);
                    }
                });
            }

            // Settings keeps the native title bar, so its close button goes
            // through CloseRequested. Route it to the same hide path as Esc and
            // the tray, so every close entry point shares one handler (the hook
            // for a future "unsaved key" prompt).
            if let Some(window) = app.get_webview_window("settings") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        windows::close_settings(&app_handle);
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            translate_text,
            get_settings,
            save_settings,
            save_key,
            delete_key,
            get_autostart,
            set_autostart,
            dismiss,
            speak,
            replace_text,
            undo_text,
            copy_text,
            explain_diff,
            test_ai_connection,
            detect_ollama,
            bar_pick,
            bar_hide,
            open_settings,
            settings_close,
            panel_resize
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
