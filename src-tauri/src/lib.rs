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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::ocr::Recognizer;
use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// 截图翻译的框选/结果层当前是否可见（用于把 Esc 全局快捷键路由到 overlay）。
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前「翻译剪贴板」快捷键（可在设置里改）。handler 用它和按键对比。
static CLIPBOARD_SHORTCUT: OnceLock<Mutex<Shortcut>> = OnceLock::new();

/// 当前「截图翻译」快捷键（可在设置里改）。
static SCREENSHOT_SHORTCUT: OnceLock<Mutex<Shortcut>> = OnceLock::new();

/// 最近一次全屏截图，只存内存，供 overlay 窗口读取，关闭即清除。
static SCREENSHOT: Mutex<Option<screenshot::ScreenCapture>> = Mutex::new(None);

/// 截图翻译的选中区域图（O5），供 overlay 结果模式读取，关闭即清除。
static RESULT_IMAGE: Mutex<Option<screenshot::ScreenCapture>> = Mutex::new(None);

/// 截图翻译的渲染块（O5），供 overlay 结果模式读取，关闭即清除。
static OCR_BLOCKS: Mutex<Option<Vec<screenshot_flow::RenderBlock>>> = Mutex::new(None);

/// 本次截图翻译用的引擎说明，例如「AI · DeepSeek」或「机器翻译 · Google」。
static OCR_ENGINE_LABEL: Mutex<Option<String>> = Mutex::new(None);

/// 已钉住的截图翻译结果（O6）。每钉住一次生成一个独立窗口，可并存多个。
static PINNED: LazyLock<Mutex<HashMap<u64, PinnedResult>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static PINNED_SEQ: AtomicU64 = AtomicU64::new(1);

/// 一个钉住的截图翻译结果，只存内存，关闭即清除。
struct PinnedResult {
    cap: screenshot::ScreenCapture,
    blocks: Vec<screenshot_flow::RenderBlock>,
    engine_label: String,
}

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

/// 把错误提示里的 {shortcut} 和 {screenshot} 占位符替换成当前快捷键。
fn fill_shortcut(msg: String) -> String {
    let clip = format_shortcut(&current_clipboard_shortcut());
    let shot = format_shortcut(&current_screenshot_shortcut());
    msg.replace("{shortcut}", &clip)
        .replace("{screenshot}", &shot)
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

/// 取词失败提示：文案 + 是否建议进入截图翻译（前端据此显示按钮）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CaptureErrorPayload {
    message: String,
    suggest_screenshot: bool,
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
struct OcrResultPayload {
    width: i32,
    height: i32,
    origin_x: i32,
    origin_y: i32,
    /// RGBA pixels of the selected region, base64-encoded.
    b64: String,
    blocks: Vec<screenshot_flow::RenderBlock>,
    /// 例如「AI · DeepSeek」或「机器翻译 · Google」，方便用户分辨。
    engine_label: String,
}

fn screenshot_engine_label(use_ai: bool, source: &str) -> String {
    let name = match source {
        "google" => "Google",
        "mymemory" => "MyMemory",
        "offline" => "离线",
        "deepl" => "DeepL",
        "deepseek" => "DeepSeek",
        "openai" => "OpenAI",
        "moonshot" => "Moonshot",
        "gemini" => "Gemini",
        "ollama" => "Ollama",
        "free" => "免费 AI",
        other => other,
    };
    if use_ai {
        format!("AI · {name}")
    } else {
        format!("机器翻译 · {name}")
    }
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
    debug_log("start_screenshot: entry");
    picker::hide();
    windows::hide_bar(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let captured = tauri::async_runtime::spawn_blocking(screenshot::capture).await;
        let cap = match captured {
            Ok(Ok(cap)) => cap,
            Ok(Err(err)) => {
                debug_log(&format!("start_screenshot: capture err={err}"));
                place_and_show(&app);
                let _ = app.emit("selection-error", err);
                return;
            }
            Err(err) => {
                debug_log(&format!("start_screenshot: join err={err}"));
                place_and_show(&app);
                let _ = app.emit("selection-error", format!("截图失败：{err}"));
                return;
            }
        };
        debug_log(&format!(
            "start_screenshot: captured {}x{} origin=({},{})",
            cap.width, cap.height, cap.origin_x, cap.origin_y
        ));
        if screenshot::is_black(&cap.pixels) {
            debug_log("start_screenshot: capture is black -> protected");
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
        match app.get_webview_window("overlay") {
            Some(window) => {
                // pot-desktop：先按物理尺寸铺满，再 always-on-top + show + focus。
                let _ = window.set_fullscreen(false);
                let _ = window.set_position(PhysicalPosition::new(origin_x, origin_y));
                let _ = window.set_size(PhysicalSize::new(width as u32, height as u32));
                let _ = window.set_always_on_top(true);
                let r1 = window.show();
                let r2 = window.set_focus();
                debug_log(&format!(
                    "start_screenshot: overlay show={r1:?} focus={r2:?}"
                ));
            }
            None => debug_log("start_screenshot: no overlay window!"),
        }
        OVERLAY_ACTIVE.store(true, Ordering::SeqCst);
        arm_dismiss(&app);
        // 通知 overlay 前端重画：overlay 窗口在启动时就已加载，组件不会重新挂载，
        // 必须靠事件触发 get_screenshot 重新取图，否则画布是空的。
        let _ = app.emit("screenshot-ready", ());
    });
}

/// 前端入口（取词失败提示里的按钮 / 托盘菜单）触发的截图翻译。
#[tauri::command]
fn screenshot_translate(app: tauri::AppHandle) {
    start_screenshot(&app);
}

/// 把全屏截图交给 overlay 前端显示（PNG，沿用 pot-desktop 的做法，避免
/// 4K 原始 RGBA 经 IPC 传几十 MB）。
#[tauri::command]
fn get_screenshot() -> Result<ScreenshotPayload, String> {
    let guard = SCREENSHOT.lock().map_err(|e| e.to_string())?;
    let cap = guard.as_ref().ok_or("没有可用的截图。")?;
    let png = screenshot::encode_png(&cap.pixels, cap.width, cap.height)?;
    let b64 = screenshot::encode_base64(&png);
    debug_log(&format!(
        "get_screenshot: {}x{} png={} b64={}",
        cap.width,
        cap.height,
        png.len(),
        b64.len()
    ));
    Ok(ScreenshotPayload {
        width: cap.width,
        height: cap.height,
        origin_x: cap.origin_x,
        origin_y: cap.origin_y,
        b64,
    })
}

/// 把截图翻译结果（选中区域图 + 渲染块）交给 overlay 结果模式显示。
#[tauri::command]
fn get_ocr_result() -> Result<OcrResultPayload, String> {
    let cap_guard = RESULT_IMAGE.lock().map_err(|e| e.to_string())?;
    let cap = cap_guard.as_ref().ok_or("没有可用的截图翻译结果。")?;
    let blocks_guard = OCR_BLOCKS.lock().map_err(|e| e.to_string())?;
    let blocks = blocks_guard.as_ref().cloned().unwrap_or_default();
    let engine_label = OCR_ENGINE_LABEL
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| "机器翻译".into());
    let png = screenshot::encode_png(&cap.pixels, cap.width, cap.height)?;
    Ok(OcrResultPayload {
        width: cap.width,
        height: cap.height,
        origin_x: cap.origin_x,
        origin_y: cap.origin_y,
        b64: screenshot::encode_base64(&png),
        blocks,
        engine_label,
    })
}

/// 钉住当前结果：把结果转移到独立窗口，overlay 回到待命状态（O6）。
/// 返回新窗口的 id；前端按 `#/pinned/<id>` 渲染。
#[tauri::command]
async fn pin_result(app: tauri::AppHandle) -> Result<u64, String> {
    let cap = RESULT_IMAGE
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or("没有可钉住的结果。")?;
    let blocks = OCR_BLOCKS
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .unwrap_or_default();

    let engine_label = OCR_ENGINE_LABEL
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .unwrap_or_else(|| "机器翻译".into());
    let id = PINNED_SEQ.fetch_add(1, Ordering::SeqCst);
    PINNED
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id, PinnedResult { cap, blocks, engine_label });

    let window = WebviewWindowBuilder::new(
        &app,
        format!("pinned-{id}"),
        WebviewUrl::App(format!("index.html#/pinned/{id}").into()),
    )
    .title("Vela 截图")
    .transparent(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    .focused(true)
    .build()
    .map_err(|e| e.to_string())?;

    // 定位到原结果位置；高度多留一截给顶栏（约 36 CSS px × DPI）。
    let pinned = PINNED.lock().map_err(|e| e.to_string())?;
    if let Some(result) = pinned.get(&id) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let bar = (28.0 * scale).round() as u32;
        let _ = window.set_position(PhysicalPosition::new(result.cap.origin_x, result.cap.origin_y));
        let _ = window.set_size(PhysicalSize::new(
            result.cap.width as u32,
            result.cap.height as u32 + bar,
        ));
    }
    drop(pinned);
    let _ = window.show();
    let _ = window.set_focus();

    // 关闭 overlay（回到待命），不销毁钉住的结果；Esc 继续可用。
    close_overlay(&app);
    sync_dismiss(&app);
    debug_log(&format!("pin_result: id={id}"));
    Ok(id)
}

/// 读取一个钉住的结果供前端渲染。
#[tauri::command]
fn get_pinned_result(id: u64) -> Result<OcrResultPayload, String> {
    let pinned = PINNED.lock().map_err(|e| e.to_string())?;
    let result = pinned.get(&id).ok_or("结果已关闭。")?;
    let png = screenshot::encode_png(&result.cap.pixels, result.cap.width, result.cap.height)?;
    Ok(OcrResultPayload {
        width: result.cap.width,
        height: result.cap.height,
        origin_x: result.cap.origin_x,
        origin_y: result.cap.origin_y,
        b64: screenshot::encode_base64(&png),
        blocks: result.blocks.clone(),
        engine_label: result.engine_label.clone(),
    })
}

/// 关闭一个钉住的窗口并立刻清空内存里的截图和识别结果。
#[tauri::command]
fn close_pinned(app: tauri::AppHandle, id: u64) {
    debug_log(&format!("close_pinned: id={id}"));
    if let Ok(mut pinned) = PINNED.lock() {
        pinned.remove(&id);
    }
    if let Some(window) = app.get_webview_window(&format!("pinned-{id}")) {
        let _ = window.hide();
        let _ = window.destroy();
    }
    sync_dismiss(&app);
}

/// 关掉所有钉住的窗口并清空内存。
fn close_all_pinned(app: &tauri::AppHandle) {
    let ids: Vec<u64> = PINNED
        .lock()
        .map(|g| g.keys().copied().collect())
        .unwrap_or_default();
    for id in ids {
        close_pinned(app.clone(), id);
    }
}

/// 框选结束：裁剪选中区域，识别文字，合并成段落，翻译并把译文盖回原位
/// （O5）。结果存内存，由 overlay 结果模式显示，关闭即清除。
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
    // 裁剪选中区域并记录其在虚拟桌面上的绝对位置；全屏截图随后即可释放。
    let (pixels, w, h, abs_x, abs_y) = {
        let guard = SCREENSHOT.lock().map_err(|e| e.to_string())?;
        let cap = guard.as_ref().ok_or("没有可用的截图。")?;
        let (pixels, w, h) = screenshot::crop_rgba(cap, x, y, width, height);
        (pixels, w, h, cap.origin_x + x, cap.origin_y + y)
    };
    if let Ok(mut guard) = SCREENSHOT.lock() {
        *guard = None;
    }

    let (recognized, pixels) = tauri::async_runtime::spawn_blocking(move || {
        let engine = ocr::WindowsOcr;
        let result = engine.recognize(&pixels, w, h);
        (result, pixels)
    })
    .await
    .map_err(|e| e.to_string())?;
    let lines = match recognized {
        Ok(lines) => lines,
        Err(err) => {
            place_and_show(&app);
            let _ = app.emit("selection-error", err);
            close_overlay(&app);
            return Ok(());
        }
    };

    if lines.is_empty() {
        place_and_show(&app);
        let _ = app.emit("selection-error", "没有识别到文字。".to_string());
        close_overlay(&app);
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
    let ai_source = if stored.local_only {
        "ollama".to_string()
    } else {
        stored.ai_source.clone()
    };
    let keys = translate::ProviderKeys {
        deepl: stored.deepl_key.clone(),
        deepseek: stored.deepseek_key.clone(),
        openai: stored.openai_key.clone(),
        moonshot: stored.moonshot_key.clone(),
        gemini: stored.gemini_key.clone(),
        ollama_model: stored.ollama_model.clone(),
    };
    let model_dir = settings_dir(&app)
        .unwrap_or_else(|_| std::env::temp_dir().join("Vela"))
        .join("models");
    // 盖回原位只显示一路：按「结果显示方式」在 AI / 机器翻译之间选。
    let use_ai = screenshot_flow::prefer_ai_for_overlay(&stored.display_mode, &ai_source, &keys);
    let engine_label = if use_ai {
        screenshot_engine_label(true, &ai_source)
    } else {
        screenshot_engine_label(false, &mt_source)
    };
    let (translator, concurrency) = if use_ai {
        let style = if stored.style == "none" {
            None
        } else {
            Some(stored.style.clone())
        };
        debug_log(&format!(
            "overlay_select: engine=ai source={ai_source} mode={}",
            stored.display_mode
        ));
        (
            screenshot_flow::ai_translator(ai_source, style, native, keys),
            1usize,
        )
    } else {
        debug_log(&format!(
            "overlay_select: engine=mt source={mt_source} mode={}",
            stored.display_mode
        ));
        (
            screenshot_flow::mt_translator(mt_source, model_dir, stored.deepl_key),
            6usize,
        )
    };
    if let Ok(mut guard) = OCR_ENGINE_LABEL.lock() {
        *guard = Some(engine_label);
    }
    let blocks = screenshot_flow::translate_paragraphs_with_concurrency(
        paragraphs, target, translator, concurrency,
    )
    .await;

    // 采样每个段落边框的背景色，准备渲染块（擦除原文 + 写译文）。
    let render = screenshot_flow::prepare_render(blocks, &pixels, w, h);

    // 保存结果供 overlay 结果模式读取。
    if let Ok(mut guard) = RESULT_IMAGE.lock() {
        *guard = Some(screenshot::ScreenCapture {
            width: w,
            height: h,
            origin_x: abs_x,
            origin_y: abs_y,
            pixels,
            monitors: Vec::new(),
        });
    }
    if let Ok(mut guard) = OCR_BLOCKS.lock() {
        *guard = Some(render);
    }

    // 把 overlay 缩到选中区域并切换到结果模式。
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.set_position(PhysicalPosition::new(abs_x, abs_y));
        let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("ocr-result-ready", ());
    Ok(())
}

/// Esc 或 ×：关闭框选层并清空内存里的截图。
#[tauri::command]
fn overlay_cancel(app: tauri::AppHandle) {
    close_overlay(&app);
}

fn close_overlay(app: &tauri::AppHandle) {
    OVERLAY_ACTIVE.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }
    if let Ok(mut guard) = SCREENSHOT.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = RESULT_IMAGE.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = OCR_BLOCKS.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = OCR_ENGINE_LABEL.lock() {
        *guard = None;
    }
    sync_dismiss(app);
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
    // Persist and broadcast the same effective engines used by local-only translation.
    let settings = settings.normalize();
    let theme = settings.theme.clone();
    let new_shortcut = settings.clipboard_shortcut.clone();
    let new_screenshot = settings.screenshot_shortcut.clone();
    let old = settings_dir(&app)
        .map(|dir| settings::load(&dir))
        .unwrap_or_default();
    settings::save(&settings_dir(&app)?, &settings)?;
    let _ = app.emit(
        "translation-engines-changed",
        serde_json::json!({
            "mtSource": settings.mt_source,
            "aiSource": settings.ai_source,
            "localOnly": settings.local_only,
        }),
    );
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
    polish_instruction: Option<String>,
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
    let polish_instruction = polish_instruction.unwrap_or_default();
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
                &polish_instruction,
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
                &polish_instruction,
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

/// 调试日志：写到临时目录，方便在不弹窗的情况下定位截图翻译问题。
fn debug_log(msg: &str) {
    let path = std::env::temp_dir().join("vela-debug.log");
    let line = format!(
        "{} {msg}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

fn hide_result(app: &tauri::AppHandle) {
    translate::cancel_ai();
    speech::stop();
    if let Some(window) = app.get_webview_window("panel") {
        let _ = window.hide();
    }
    sync_dismiss(app);
    let _ = app.emit("result-dismissed", ());
}

fn arm_dismiss(app: &tauri::AppHandle) {
    let _ = app.global_shortcut().unregister(dismiss_shortcut());
    let _ = app.global_shortcut().register(dismiss_shortcut());
}

/// 让 Esc 全局快捷键只在「结果窗 / 截图框选层 / 钉住窗」需要时注册。
fn sync_dismiss(app: &tauri::AppHandle) {
    let panel_visible = app
        .get_webview_window("panel")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let has_pinned = PINNED
        .lock()
        .map(|g| !g.is_empty())
        .unwrap_or(false);
    if OVERLAY_ACTIVE.load(Ordering::SeqCst) || panel_visible || has_pinned {
        arm_dismiss(app);
    } else {
        let _ = app.global_shortcut().unregister(dismiss_shortcut());
    }
}

#[tauri::command]
fn dismiss(app: tauri::AppHandle) {
    hide_result(&app);
}

#[tauri::command]
fn bar_pick(app: tauri::AppHandle, action: String) {
    let allow_partial = settings_dir(&app)
        .map(|dir| settings::load(&dir).allow_partial_clipboard)
        .unwrap_or(false);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Electron：探测阶段已缓存选区文字，点按钮后选区多半没了——直接用缓存。
        // 其它应用：先切回源窗口再取词，再关动作条（先 hide 容易丢焦点）。
        let captured = tauri::async_runtime::spawn_blocking(move || {
            let exe = writeback::target::source_exe();
            capture::log_focus_stage("bar_pick", &exe);
            let budget = capture::prefetch_config().budget_ms;
            // 出条后立刻点「译」时，后台 arm 可能还没写完缓存。
            if let Some(text) =
                capture::take_electron_cache_wait(std::time::Duration::from_millis(budget))
            {
                capture::log_focus_stage("bar_pick_cache_hit", &exe);
                return Ok(text);
            }
            capture::log_focus_stage("bar_pick_fallback", &exe);
            let _ = writeback::target::switch_back_and_verify();
            capture::capture_allow_partial(allow_partial)
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()));
        windows::hide_bar(&app);
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
                let _ = app.emit(
                    "capture-error",
                    CaptureErrorPayload {
                        message: fill_shortcut(err),
                        suggest_screenshot: true,
                    },
                );
            }
        }
    });
}

#[tauri::command]
fn bar_hide(app: tauri::AppHandle) {
    windows::hide_bar(&app);
}

#[tauri::command]
fn bar_layout(app: tauri::AppHandle, mode: String) {
    windows::set_bar_layout(&app, &mode);
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
    let ai_source = ai_source_for_settings(&stored).to_string();
    let keys = translate::ProviderKeys {
        deepl: stored.deepl_key,
        deepseek: stored.deepseek_key,
        openai: stored.openai_key,
        moonshot: stored.moonshot_key,
        gemini: stored.gemini_key,
        ollama_model: stored.ollama_model,
    };
    translate::explain_diff(&original, &polished, &ai_source, &keys, &stored.native).await
}

fn ai_source_for_settings(settings: &settings::Settings) -> &str {
    if settings.local_only {
        "ollama"
    } else {
        &settings.ai_source
    }
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
    if stored.local_only && source != "ollama" {
        return Err("完全本地模式已开启，已阻止云端连接测试。".into());
    }
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
                let _ = app.emit(
                    "capture-error",
                    CaptureErrorPayload {
                        message: fill_shortcut(err),
                        suggest_screenshot: true,
                    },
                );
            }
        }
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
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
                    debug_log(&format!(
                        "shortcut: {} {}",
                        format_shortcut(shortcut),
                        if shortcut.mods.is_empty() { "(bare)" } else { "" }
                    ));
                    let dismiss_key = dismiss_shortcut();
                    if shortcut == &dismiss_key {
                        if OVERLAY_ACTIVE.load(Ordering::SeqCst) {
                            close_overlay(app);
                        } else if PINNED
                            .lock()
                            .map(|g| !g.is_empty())
                            .unwrap_or(false)
                        {
                            close_all_pinned(app);
                        } else {
                            hide_result(app);
                        }
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
            let screenshot_item = MenuItem::with_id(app, "screenshot", "截图翻译", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&screenshot_item, &settings_item, &autostart, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("Vela")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    } else if event.id.as_ref() == "settings" {
                        windows::open_settings(app);
                    } else if event.id.as_ref() == "screenshot" {
                        start_screenshot(app);
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
            bar_layout,
            open_settings,
            settings_close,
            panel_resize,
            get_screenshot,
            get_ocr_result,
            pin_result,
            get_pinned_result,
            close_pinned,
            overlay_select,
            overlay_cancel,
            screenshot_translate
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_mode_forces_local_ai_for_polish_explanations() {
        let mut settings = settings::Settings::default();
        settings.local_only = true;
        settings.ai_source = "deepseek".into();
        assert_eq!(ai_source_for_settings(&settings), "ollama");
    }

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

    #[test]
    fn fill_shortcut_replaces_screenshot_placeholder() {
        if let Ok(mut guard) = SCREENSHOT_SHORTCUT
            .get_or_init(|| Mutex::new(default_screenshot_shortcut()))
            .lock()
        {
            *guard = default_screenshot_shortcut();
        }
        let msg = fill_shortcut("可按 {screenshot} 截图翻译，或按 {shortcut} 翻译剪贴板。".to_string());
        assert!(msg.contains("Ctrl+Alt+S"));
        assert!(msg.contains("Ctrl+Alt+Shift+T"));
        assert!(!msg.contains("{screenshot}"));
        assert!(!msg.contains("{shortcut}"));
    }
}
