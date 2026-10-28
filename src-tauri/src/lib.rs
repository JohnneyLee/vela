mod capture;
mod dpapi;
mod ocr;
mod offline;
mod paragraph;
mod picker;
mod screenshot;
mod screenshot_flow;
mod secrets;
mod selection;
mod settings;
mod speech;
mod suggest;
mod translate;
mod windows;
mod writeback;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::ocr::Recognizer;
use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// 当前「翻译剪贴板」快捷键（可在设置里改）。handler 用它和按键对比。
static CLIPBOARD_SHORTCUT: OnceLock<Mutex<Shortcut>> = OnceLock::new();

/// 当前「截图翻译」快捷键（可在设置里改）。
static SCREENSHOT_SHORTCUT: OnceLock<Mutex<Shortcut>> = OnceLock::new();

/// 最近一次全屏截图，只存内存，供 overlay 窗口读取，关闭即清除。
static SCREENSHOT: Mutex<Option<screenshot::ScreenCapture>> = Mutex::new(None);

fn default_clipboard_shortcut() -> Shortcut {
    Shortcut::new(
        Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
        Code::KeyT,
    )
}

fn default_screenshot_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS)
}

fn current_clipboard_shortcut() -> Shortcut {
    let lock = CLIPBOARD_SHORTCUT.get_or_init(|| Mutex::new(default_clipboard_shortcut()));
    *lock.lock().unwrap_or_else(|e| e.into_inner())
}

fn current_screenshot_shortcut() -> Shortcut {
    let lock = SCREENSHOT_SHORTCUT.get_or_init(|| Mutex::new(default_screenshot_shortcut()));
    *lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// 把设置里存的字符串解析成快捷键；解析失败回退默认值。
fn parse_clipboard_shortcut(raw: &str) -> Shortcut {
    raw.parse::<Shortcut>().unwrap_or_else(|_| default_clipboard_shortcut())
}

fn parse_screenshot_shortcut(raw: &str) -> Shortcut {
    raw.parse::<Shortcut>().unwrap_or_else(|_| default_screenshot_shortcut())
}

/// 把快捷键转成用户可读的 "Ctrl+Alt+Shift+T" 形式。
fn format_shortcut(sc: &Shortcut) -> String {
    let mut parts: Vec<String> = Vec::new();
    if sc.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".into());
    }
    if sc.mods.contains(Modifiers::ALT) {
        parts.push("Alt".into());
    }
    if sc.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".into());
    }
    if sc.mods.contains(Modifiers::SUPER) {
        parts.push("Win".into());
    }
    let key = sc.key.to_string();
    let key = key
        .strip_prefix("Key")
        .or_else(|| key.strip_prefix("Digit"))
        .unwrap_or(&key)
        .to_string();
    parts.push(key);
    parts.join("+")
}

/// 把错误提示里的 {shortcut} 占位符替换成当前「翻译剪贴板」快捷键。
fn fill_shortcut(msg: String) -> String {
    let sc = format_shortcut(&current_clipboard_shortcut());
    msg.replace("{shortcut}", &sc)
}

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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScreenshotPayload {
    width: i32,
    height: i32,
    origin_x: i32,
    origin_y: i32,
    /// RGBA pixels, base64-encoded.
    b64: String,
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

/// 按下「翻译剪贴板」快捷键：读一次剪贴板里的文字并翻译。不监听剪贴板。
fn translate_clipboard(app: &tauri::AppHandle) {
    picker::hide();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let text = tauri::async_runtime::spawn_blocking(capture::clipboard::read_text)
            .await
            .unwrap_or(None)
            .and_then(|t| capture::sanitize(&t));
        match text {
            Some(text) => {
                place_and_show(&app);
                let _ = app.emit(
                    "selection-captured",
                    SelectionPayload { text, action: "translate".into(), editable: false },
                );
            }
            None => {
                place_and_show(&app);
                let _ = app.emit("selection-error", "剪贴板里没有文字。".to_string());
            }
        }
    });
}

/// 按下「截图翻译」快捷键：先截下整个屏幕，再显示全屏框选层。
fn start_screenshot(app: &tauri::AppHandle) {
    picker::hide();
    windows::hide_bar(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let captured = tauri::async_runtime::spawn_blocking(screenshot::capture).await;
        let cap = match captured {
            Ok(Ok(cap)) => cap,
            Ok(Err(err)) => {
                place_and_show(&app);
                let _ = app.emit("selection-error", err);
                return;
            }
            Err(err) => {
                place_and_show(&app);
                let _ = app.emit("selection-error", format!("截图失败：{err}"));
                return;
            }
        };
        if screenshot::is_black(&cap.pixels) {
            place_and_show(&app);
            let _ = app.emit("selection-error", "该画面受保护，无法截图。".to_string());
            return;
        }
        let width = cap.width;
        let height = cap.height;
        let origin_x = cap.origin_x;
        let origin_y = cap.origin_y;
        if let Ok(mut guard) = SCREENSHOT.lock() {
            *guard = Some(cap);
        }
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.set_position(PhysicalPosition::new(origin_x, origin_y));
            let _ = window.set_size(PhysicalSize::new(width as u32, height as u32));
            let _ = window.show();
            let _ = window.set_focus();
        }
    });
}

/// 把全屏截图交给 overlay 前端显示。
#[tauri::command]
fn get_screenshot() -> Result<ScreenshotPayload, String> {
    let guard = SCREENSHOT.lock().map_err(|e| e.to_string())?;
    let cap = guard.as_ref().ok_or("没有可用的截图。")?;
    Ok(ScreenshotPayload {
        width: cap.width,
        height: cap.height,
        origin_x: cap.origin_x,
        origin_y: cap.origin_y,
        b64: screenshot::encode_base64(&cap.pixels),
    })
}

/// 框选结束：裁剪选中区域，识别文字，合并成段落并翻译（O4）。译文先送到
/// 结果窗显示（O5 再改成盖回原位）。
#[tauri::command]
async fn overlay_select(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        close_overlay(&app);
        return Err("框选区域太小。".into());
    }
    // 先从内存截图里裁剪，再关闭框选层并清空截图。
    let cropped = {
        let guard = SCREENSHOT.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .map(|cap| screenshot::crop_rgba(cap, x, y, width, height))
    };
    close_overlay(&app);
    let (pixels, w, h) = cropped.ok_or("没有可用的截图。")?;

    let lines = tauri::async_runtime::spawn_blocking(move || {
        let engine = ocr::WindowsOcr;
        engine.recognize(&pixels, w, h)
    })
    .await
    .map_err(|e| e.to_string())??;

    if lines.is_empty() {
        place_and_show(&app);
        let _ = app.emit("selection-error", "没有识别到文字。".to_string());
        return Ok(());
    }

    // 合并成段落，再按段落并发翻译。目标语言按 S7 的方向规则（用整段文字判断）。
    let paragraphs = paragraph::merge(lines);
    let stored = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    let native = stored.native.clone();
    let all_text = paragraphs
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let target = translate::resolve_target(&all_text, &native, None);
    let mt_source = if stored.local_only {
        "offline".to_string()
    } else {
        stored.mt_source.clone()
    };
    let model_dir = settings_dir(&app)
        .unwrap_or_else(|_| std::env::temp_dir().join("Vela"))
        .join("models");
    let translator =
        screenshot_flow::mt_translator(mt_source, model_dir, stored.deepl_key);
    let blocks = screenshot_flow::translate_paragraphs(paragraphs, target, translator).await;

    // O4 阶段把译文拼成文本送到结果窗（O5 再改为盖回原位）。保留换行，
    // 段落之间空一行；未翻译（公式/代码）和失败的段落回退到原文并标注。
    let text = blocks
        .iter()
        .map(|b| {
            if b.skip {
                format!("〔未翻译〕{}", b.original)
            } else if !b.translated.is_empty() {
                b.translated.clone()
            } else {
                format!("〔翻译失败〕{}", b.original)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    place_and_show(&app);
    let _ = app.emit("ocr-translated", text);
    Ok(())
}

/// Esc 或 ×：关闭框选层并清空内存里的截图。
#[tauri::command]
fn overlay_cancel(app: tauri::AppHandle) {
    close_overlay(&app);
}

fn close_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }
    if let Ok(mut guard) = SCREENSHOT.lock() {
        *guard = None;
    }
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> settings::Settings {
    settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    // 校验「翻译剪贴板」快捷键：空或解析不了直接报错，不保存。
    if settings.clipboard_shortcut.trim().is_empty()
        || settings.clipboard_shortcut.parse::<Shortcut>().is_err()
    {
        return Err("翻译剪贴板快捷键格式不对。例如：Ctrl+Alt+Shift+T。".into());
    }
    if settings.screenshot_shortcut.trim().is_empty()
        || settings.screenshot_shortcut.parse::<Shortcut>().is_err()
    {
        return Err("截图翻译快捷键格式不对。例如：Ctrl+Alt+S。".into());
    }
    let theme = settings.theme.clone();
    let new_shortcut = settings.clipboard_shortcut.clone();
    let new_screenshot = settings.screenshot_shortcut.clone();
    let old = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    settings::save(&settings_dir(&app)?, &settings)?;
    // 主题变化后广播给所有窗口（bar / panel / settings），让它们即时切换。
    let _ = app.emit("theme-changed", theme);
    if old.clipboard_shortcut != new_shortcut {
        rearm_clipboard_shortcut(&app, &new_shortcut)?;
    }
    if old.screenshot_shortcut != new_screenshot {
        rearm_screenshot_shortcut(&app, &new_screenshot)?;
    }
    Ok(())
}

/// 重新注册「翻译剪贴板」快捷键并更新全局状态；失败表示新快捷键被占用。
fn rearm_clipboard_shortcut(app: &tauri::AppHandle, raw: &str) -> Result<(), String> {
    let parsed = parse_clipboard_shortcut(raw);
    let _ = app.global_shortcut().unregister(current_clipboard_shortcut());
    if let Ok(mut guard) = CLIPBOARD_SHORTCUT
        .get_or_init(|| Mutex::new(default_clipboard_shortcut()))
        .lock()
    {
        *guard = parsed;
    }
    app.global_shortcut()
        .register(parsed)
        .map_err(|err| format!("翻译剪贴板快捷键 {} 已被占用。{err}", format_shortcut(&parsed)))
}

/// 重新注册「截图翻译」快捷键并更新全局状态；失败表示新快捷键被占用。
fn rearm_screenshot_shortcut(app: &tauri::AppHandle, raw: &str) -> Result<(), String> {
    let parsed = parse_screenshot_shortcut(raw);
    let _ = app.global_shortcut().unregister(current_screenshot_shortcut());
    if let Ok(mut guard) = SCREENSHOT_SHORTCUT
        .get_or_init(|| Mutex::new(default_screenshot_shortcut()))
        .lock()
    {
        *guard = parsed;
    }
    app.global_shortcut()
        .register(parsed)
        .map_err(|err| format!("截图翻译快捷键 {} 已被占用。{err}", format_shortcut(&parsed)))
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
    speech::stop();
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
                let _ = app.emit("selection-error", fill_shortcut(err));
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
fn stop_speak() {
    speech::stop();
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
                let _ = app.emit("selection-error", fill_shortcut(err));
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
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let dismiss_key = dismiss_shortcut();
                    if shortcut == &dismiss_key {
                        hide_result(app);
                        return;
                    }
                    let wanted = Shortcut::new(
                        Some(Modifiers::CONTROL | Modifiers::ALT),
                        Code::KeyT,
                    );
                    if shortcut == &wanted {
                        on_hotkey(app);
                        return;
                    }
                    if *shortcut == current_clipboard_shortcut() {
                        translate_clipboard(app);
                        return;
                    }
                    if *shortcut == current_screenshot_shortcut() {
                        start_screenshot(app);
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

            // 注册「翻译剪贴板」快捷键，并用它更新全局状态供 handler 对比。
            let stored = settings_dir(&app.handle()).map(|dir| settings::load(&dir)).unwrap_or_default();
            let clip_shortcut = parse_clipboard_shortcut(&stored.clipboard_shortcut);
            if let Ok(mut guard) = CLIPBOARD_SHORTCUT
                .get_or_init(|| Mutex::new(default_clipboard_shortcut()))
                .lock()
            {
                *guard = clip_shortcut;
            }
            if let Err(err) = app.global_shortcut().register(clip_shortcut) {
                let label = format_shortcut(&clip_shortcut);
                let message = format!("翻译剪贴板快捷键 {label} 已被占用。{err}");
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    place_and_show(&handle);
                    let _ = handle.emit("hotkey-error", message);
                });
            }

            // 注册「截图翻译」快捷键，并用它更新全局状态供 handler 对比。
            let shot_shortcut = parse_screenshot_shortcut(&stored.screenshot_shortcut);
            if let Ok(mut guard) = SCREENSHOT_SHORTCUT
                .get_or_init(|| Mutex::new(default_screenshot_shortcut()))
                .lock()
            {
                *guard = shot_shortcut;
            }
            if let Err(err) = app.global_shortcut().register(shot_shortcut) {
                let label = format_shortcut(&shot_shortcut);
                let message = format!("截图翻译快捷键 {label} 已被占用。{err}");
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
            stop_speak,
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
            panel_resize,
            get_screenshot,
            overlay_select,
            overlay_cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_clipboard_shortcut_is_ctrl_alt_shift_t() {
        let sc = default_clipboard_shortcut();
        assert_eq!(sc.key, Code::KeyT);
        assert!(sc.mods.contains(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT));
    }

    #[test]
    fn parse_clipboard_shortcut_accepts_readable_form() {
        let sc = parse_clipboard_shortcut("Ctrl+Alt+Shift+T");
        assert_eq!(sc.key, Code::KeyT);
        assert!(sc.mods.contains(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT));
    }

    #[test]
    fn parse_clipboard_shortcut_falls_back_on_bad_input() {
        let sc = parse_clipboard_shortcut("not a shortcut");
        assert_eq!(sc, default_clipboard_shortcut());
    }

    #[test]
    fn format_shortcut_is_readable() {
        let sc = Shortcut::new(
            Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
            Code::KeyT,
        );
        assert_eq!(format_shortcut(&sc), "Ctrl+Alt+Shift+T");
    }

    #[test]
    fn fill_shortcut_replaces_placeholder() {
        if let Ok(mut guard) = CLIPBOARD_SHORTCUT
            .get_or_init(|| Mutex::new(default_clipboard_shortcut()))
            .lock()
        {
            *guard = default_clipboard_shortcut();
        }
        let msg = fill_shortcut("不支持。可按 {shortcut} 翻译。".to_string());
        assert!(msg.contains("Ctrl+Alt+Shift+T"));
        assert!(!msg.contains("{shortcut}"));
    }
}
