//! Machine-translation and AI adapters. The window stacks the two results.
//! Translation text is not stored.

use std::cmp::Reverse;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::watch;

static AI_GEN: AtomicU64 = AtomicU64::new(1);
static CANCEL_TX: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(1).0);

const CANCELLED: &str = "cancelled";

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 Vela")
        .build()
        .expect("reqwest client")
});

pub struct TranslateRequest<'a> {
    pub text: &'a str,
    pub target: &'a str,
    pub style: Option<&'a str>,
}

#[async_trait]
pub trait Translator: Send + Sync {
    fn id(&self) -> &'static str;
    async fn translate(&self, req: TranslateRequest<'_>) -> Result<String, String>;
}

pub struct GoogleGtx;

#[async_trait]
impl Translator for GoogleGtx {
    fn id(&self) -> &'static str {
        "google"
    }

    async fn translate(&self, req: TranslateRequest<'_>) -> Result<String, String> {
        let _ = req.style;
        let url = format!(
            "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
            req.target,
            urlencoding::encode(req.text)
        );
        let body: serde_json::Value = http_get_json(&url, Duration::from_millis(2500)).await?;
        let mut out = String::new();
        let Some(parts) = body.get(0).and_then(|v| v.as_array()) else {
            return Err("Google 返回了无法解析的结果。".into());
        };
        for part in parts {
            if let Some(piece) = part.get(0).and_then(|v| v.as_str()) {
                out.push_str(piece);
            }
        }
        if out.trim().is_empty() {
            return Err("Google 没有返回译文。".into());
        }
        Ok(out)
    }
}

pub struct MyMemory;

#[async_trait]
impl Translator for MyMemory {
    fn id(&self) -> &'static str {
        "mymemory"
    }

    async fn translate(&self, req: TranslateRequest<'_>) -> Result<String, String> {
        let _ = req.style;
        let source = if req.target == "en" { "zh-CN" } else { "en" };
        let url = format!(
            "https://api.mymemory.translated.net/get?q={}&langpair={source}|{}",
            urlencoding::encode(req.text),
            req.target
        );
        let body: serde_json::Value = http_get_json(&url, Duration::from_millis(6000)).await?;
        let text = body
            .get("responseData")
            .and_then(|v| v.get("translatedText"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return Err("MyMemory 没有返回译文。".into());
        }
        Ok(text)
    }
}

pub struct ProviderKeys {
    pub deepl: String,
    pub deepseek: String,
    pub openai: String,
    pub moonshot: String,
    pub gemini: String,
    /// 本机 Ollama 选用的模型名（非 Key）。为空时取第一个已安装模型。
    pub ollama_model: String,
}

pub struct CompareTranslation {
    pub mt_text: String,
    pub mt_service: String,
    pub mt_error: String,
    pub ai_text: String,
    pub ai_service: String,
    pub ai_error: String,
}

struct Column {
    text: String,
    service: String,
    error: String,
}

/// A preset for an OpenAI-compatible chat endpoint. Most cloud providers share
/// this wire format; a new provider is usually just one more entry here.
struct OpenAiPreset {
    url: &'static str,
    model: &'static str,
    empty_error: &'static str,
    needs_key: bool,
}

fn openai_preset(source: &str) -> Option<OpenAiPreset> {
    Some(match source {
        "free" => OpenAiPreset {
            url: "https://text.pollinations.ai/openai",
            model: "openai",
            empty_error: "AI 没有返回内容。",
            needs_key: false,
        },
        "deepseek" => OpenAiPreset {
            url: "https://api.deepseek.com/chat/completions",
            model: "deepseek-flash",
            empty_error: "DeepSeek 没有返回内容。",
            needs_key: true,
        },
        "openai" => OpenAiPreset {
            url: "https://api.openai.com/v1/chat/completions",
            model: "gpt-4o-mini",
            empty_error: "OpenAI 没有返回内容。",
            needs_key: true,
        },
        "moonshot" => OpenAiPreset {
            url: "https://api.moonshot.cn/v1/chat/completions",
            model: "moonshot-v1-8k",
            empty_error: "Moonshot 没有返回内容。",
            needs_key: true,
        },
        "gemini" => OpenAiPreset {
            url: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
            model: "gemini-2.0-flash",
            empty_error: "Gemini 没有返回内容。",
            needs_key: true,
        },
        "ollama" => OpenAiPreset {
            url: "http://localhost:11434/v1/chat/completions",
            model: "llama3",
            empty_error: "Ollama 没有返回内容。",
            needs_key: false,
        },
        _ => return None,
    })
}

fn provider_key<'a>(source: &str, keys: &'a ProviderKeys) -> Option<&'a str> {
    match source {
        "openai" => Some(&keys.openai),
        "moonshot" => Some(&keys.moonshot),
        "gemini" => Some(&keys.gemini),
        "deepseek" => Some(&keys.deepseek),
        _ => None,
    }
}

fn provider_label(source: &str) -> &'static str {
    match source {
        "openai" => "OpenAI",
        "moonshot" => "Moonshot",
        "gemini" => "Gemini",
        "deepseek" => "DeepSeek",
        "ollama" => "Ollama",
        "free" => "免费模型",
        "deepl" => "DeepL",
        _ => "AI",
    }
}

/// Run a prompt through an OpenAI-compatible provider preset.
async fn chat_with_preset(
    source: &str,
    prompt: &str,
    keys: &ProviderKeys,
    on_chunk: &(dyn Fn(String) + Send + Sync),
    stream: bool,
) -> Result<String, String> {
    let preset =
        openai_preset(source).ok_or_else(|| "当前引擎不支持这个操作。".to_string())?;
    let key = provider_key(source, keys);
    if preset.needs_key && key.map(|k| k.is_empty()).unwrap_or(true) {
        return Err(format!(
            "还没有 {} API Key。Key 只保存在本机。",
            provider_label(source)
        ));
    }
    // Ollama 的模型名由用户选择，而不是预置默认值；未选择时取第一个已安装模型。
    let model: String;
    if source == "ollama" {
        model = if !keys.ollama_model.trim().is_empty() {
            keys.ollama_model.trim().to_string()
        } else {
            let models = detect_ollama().await?;
            models
                .into_iter()
                .next()
                .ok_or_else(|| "Ollama 已连接，但还没有任何模型。请先 `ollama pull` 拉取。".to_string())?
        };
    } else {
        model = preset.model.to_string();
    }
    chat_translation(
        preset.url,
        &model,
        prompt,
        key,
        preset.empty_error,
        on_chunk,
        stream,
    )
    .await
}

/// Pick an override key if one was supplied, otherwise the stored key.
fn pick_key<'a>(override_key: &'a str, stored: &'a str) -> &'a str {
    if override_key.is_empty() {
        stored
    } else {
        override_key
    }
}

/// Test a provider connection with a minimal request. `override_key` lets the
/// settings page test a freshly-typed key before it is saved. Returns a short
/// success message (with latency) or a user-facing error.
pub async fn test_connection(
    source: &str,
    override_key: &str,
    keys: &ProviderKeys,
) -> Result<String, String> {
    let started = std::time::Instant::now();
    let ms = || started.elapsed().as_millis();

    if source == "ollama" {
        let models = detect_ollama().await?;
        if models.is_empty() {
            return Err("Ollama 已连接，但还没有任何模型。".into());
        }
        return Ok(format!("✓ 已连接 Ollama · {} 个模型 · {} ms", models.len(), ms()));
    }

    if source == "deepl" {
        let key = pick_key(override_key, &keys.deepl);
        if key.is_empty() {
            return Err("还没有 DeepL API Key。Key 只保存在本机。".into());
        }
        let col = deepl_column("Hello", "zh-CN", key, None).await;
        if col.error.is_empty() {
            return Ok(format!("✓ 连接成功 · {} ms", ms()));
        }
        return Err(col.error);
    }

    let preset =
        openai_preset(source).ok_or_else(|| "未知引擎。".to_string())?;
    let key = match source {
        "openai" => pick_key(override_key, &keys.openai),
        "moonshot" => pick_key(override_key, &keys.moonshot),
        "gemini" => pick_key(override_key, &keys.gemini),
        "deepseek" => pick_key(override_key, &keys.deepseek),
        _ => "",
    };
    if preset.needs_key && key.is_empty() {
        return Err(format!("还没有 {} API Key。Key 只保存在本机。", provider_label(source)));
    }
    let key_ref = if key.is_empty() { None } else { Some(key) };
    let result = chat_translation(
        preset.url,
        preset.model,
        "Reply with the single word: OK",
        key_ref,
        preset.empty_error,
        &|_| {},
        false,
    )
    .await;
    match result {
        Ok(_) => Ok(format!("✓ 连接成功 · {} ms", ms())),
        Err(err) => Err(redact(err, &[override_key, key])),
    }
}

/// List the models currently available from a running local Ollama instance.
pub async fn detect_ollama() -> Result<Vec<String>, String> {
    let response = HTTP_CLIENT
        .get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| "未检测到 Ollama。请确认已安装并运行，或访问 https://ollama.com。".to_string())?;
    if !response.status().is_success() {
        return Err("Ollama 返回了错误状态。".into());
    }
    let body: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let models = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(models)
}

pub fn cancel_ai() {
    let next = AI_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = CANCEL_TX.send(next);
}

pub fn resolve_target(text: &str, direction: &str) -> &'static str {
    match direction {
        "zh-en" => "en",
        "en-zh" => "zh-CN",
        _ => target_lang(text),
    }
}

pub async fn translate_compare(
    text: &str,
    target: &str,
    mt_source: &str,
    ai_source: &str,
    style: Option<&str>,
    mode: &str,
    polish_level: &str,
    keys: &ProviderKeys,
    model_dir: &Path,
) -> CompareTranslation {
    let (mt, ai) = tokio::join!(
        mt_column(text, target, mt_source, model_dir, &keys.deepl, &|_| {}),
        ai_column(text, target, ai_source, style, mode, polish_level, keys, &|_| {}),
    );
    let secrets = [keys.deepl.as_str(), keys.deepseek.as_str()];
    CompareTranslation {
        mt_text: mt.text,
        mt_service: mt.service,
        mt_error: redact(mt.error, &secrets),
        ai_text: ai.text,
        ai_service: ai.service,
        ai_error: redact(ai.error, &secrets),
    }
}

pub async fn translate_mt(
    text: &str,
    target: &str,
    source: &str,
    model_dir: &Path,
    deepl_key: &str,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> (String, String, String) {
    let column = mt_column(text, target, source, model_dir, deepl_key, on_status).await;
    (column.text, column.service, column.error)
}

pub async fn translate_ai(
    text: &str,
    target: &str,
    source: &str,
    style: Option<&str>,
    mode: &str,
    polish_level: &str,
    keys: &ProviderKeys,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> (String, String, String) {
    cancel_ai();
    let column = ai_column(text, target, source, style, mode, polish_level, keys, on_chunk).await;
    let secrets = [keys.deepl.as_str(), keys.deepseek.as_str()];
    (
        column.text,
        column.service,
        redact(column.error, &secrets),
    )
}

fn normalize_for_translation(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn mt_column(
    text: &str,
    target: &str,
    source: &str,
    model_dir: &Path,
    deepl_key: &str,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Column {
    let text = normalize_for_translation(text);
    match source {
        "google" => google_column(&text, target, model_dir, on_status).await,
        "mymemory" => mymemory_column(&text, target).await,
        "offline" => offline_column(&text, target, model_dir, on_status).await,
        "deepl" => deepl_column(&text, target, deepl_key, None).await,
        _ => failed("不认识的翻译源。"),
    }
}

async fn google_column(
    text: &str,
    target: &str,
    model_dir: &Path,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Column {
    let req = TranslateRequest {
        text,
        target,
        style: None,
    };
    match GoogleGtx.translate(req).await {
        Ok(translated) => ready(translated, "google"),
        Err(google_err) => {
            let req = TranslateRequest {
                text,
                target,
                style: None,
            };
            match MyMemory.translate(req).await {
                Ok(translated) => ready(translated, "mymemory"),
                Err(fallback_err) => {
                    match offline_column(text, target, model_dir, on_status).await {
                        column if column.error.is_empty() && !column.text.is_empty() => column,
                        column => failed(format!(
                            "Google：{google_err}\nMyMemory：{fallback_err}\n离线：{}",
                            column.error
                        )),
                    }
                }
            }
        }
    }
}

async fn deepl_column(text: &str, target: &str, key: &str, style: Option<&str>) -> Column {
    if key.is_empty() {
        return failed("还没有 DeepL API Key。Key 只保存在本机。");
    }
    let url = if key.ends_with(":fx") {
        "https://api-free.deepl.com/v2/translate"
    } else {
        "https://api.deepl.com/v2/translate"
    };
    let target_lang = if target == "en" { "EN" } else { "ZH" };
    let mut payload = serde_json::json!({
        "text": [text],
        "target_lang": target_lang,
    });
    if target == "en" {
        match style {
            Some("formal") | Some("academic") => {
                payload["formality"] = serde_json::json!("prefer_more");
            }
            Some("colloquial") => {
                payload["formality"] = serde_json::json!("prefer_less");
            }
            _ => {}
        }
    }
    let request = HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .json(&payload)
        .timeout(Duration::from_secs(8));
    let parsed = send_json(request, &[key]).await.and_then(|body| {
        let translated = body
            .pointer("/translations/0/text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if translated.is_empty() {
            Err("DeepL 没有返回译文。".to_string())
        } else {
            Ok(translated)
        }
    });
    from_result(parsed, "deepl")
}

async fn mymemory_column(text: &str, target: &str) -> Column {
    let req = TranslateRequest {
        text,
        target,
        style: None,
    };
    from_result(MyMemory.translate(req).await, "mymemory")
}

async fn offline_column(
    text: &str,
    target: &str,
    model_dir: &Path,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Column {
    match crate::offline::translate(model_dir, text, target, on_status).await {
        Ok(translated) if !translated.trim().is_empty() => ready(translated, "offline"),
        Ok(_) => failed("离线翻译没有返回译文。"),
        Err(err) => failed(err),
    }
}

async fn ai_column(
    text: &str,
    target: &str,
    source: &str,
    style: Option<&str>,
    mode: &str,
    polish_level: &str,
    keys: &ProviderKeys,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> Column {
    // Protect URLs, @mentions, code, placeholders and emoji so the model leaves
    // them intact. Only translate/polish need this; explain is meta-language.
    let protected = if mode == "translate" || mode == "polish" {
        let p = crate::writeback::protect::protect(text);
        if p.has_fragments() {
            Some(p)
        } else {
            None
        }
    } else {
        None
    };
    let effective = protected.as_ref().map(|p| p.text.as_str()).unwrap_or(text);

    // Stream chunks back with complete tokens already restored.
    let wrapped_chunk = |chunk: String| {
        let out = match &protected {
            Some(p) => crate::writeback::protect::partial_restore(&chunk, p.saved()),
            None => chunk,
        };
        on_chunk(out);
    };

    let mut column = match source {
        "deepl" if mode == "translate" => {
            deepl_column(effective, target, &keys.deepl, style).await
        }
        "deepl" => failed("DeepL 只能翻译，不能解释或润色。请改用其他引擎。"),
        s => {
            let prompt = ai_prompt(mode, effective, target, style, polish_level);
            match chat_with_preset(s, &prompt, keys, &wrapped_chunk, true).await {
                Ok(text) => ready(text, s),
                Err(err) if err == CANCELLED => Column {
                    text: String::new(),
                    service: CANCELLED.into(),
                    error: String::new(),
                },
                Err(err) => failed(err),
            }
        }
    };

    // Translate mode: if the model returned text in the source language (a
    // rewrite instead of a translation), retry once with a strict prompt that
    // names the target language explicitly.
    if mode == "translate"
        && !column.text.is_empty()
        && openai_preset(source).is_some()
    {
        let src_lang = crate::suggest::detect_lang(text);
        let out_lang = crate::suggest::detect_lang(&column.text);
        let same_language = src_lang != crate::suggest::Lang::Other && out_lang == src_lang;
        if same_language {
            let strict = strict_translation_prompt(text, target);
            if let Ok(new_text) = chat_with_preset(source, &strict, keys, &wrapped_chunk, true).await
            {
                if !new_text.is_empty() {
                    column.text = new_text;
                }
            }
        }
    }

    // Restore protected fragments in the final text. Strict restore first; fall
    // back to a lenient one that drops stray markers if the model mangled them.
    if let Some(p) = &protected {
        if !column.text.is_empty() {
            column.text = match crate::writeback::protect::restore(&column.text, p.saved()) {
                Some(restored) => restored,
                None => crate::writeback::protect::restore_loose(&column.text, p.saved()),
            };
        }
    }
    column
}

fn strict_translation_prompt(text: &str, target: &str) -> String {
    let language = if target == "en" {
        "English"
    } else {
        "Simplified Chinese"
    };
    format!(
        "Translate the following text into {language}. Output only the translation, nothing else.\n\n{text}"
    )
}

fn translation_prompt(text: &str, target: &str, style: Option<&str>) -> String {
    let language = if target == "en" {
        "English"
    } else {
        "Simplified Chinese"
    };
    let tone = match style {
        Some("academic") => "Use an academic tone.",
        Some("colloquial") => "Use a colloquial, spoken tone.",
        Some("formal") => "Use a formal tone.",
        Some("idiomatic") => "Use natural idiomatic wording.",
        Some("literal") => "Translate literally and stay close to the source wording.",
        _ => "Use a natural tone.",
    };
    format!(
        "Translate the following text to {language}. {tone} Output only the translation, with no notes or quotes.\n\
         Any ⟦number⟧ marker in the text is a placeholder: copy it to the output exactly as-is, do not translate or reorder it.\n\n{text}"
    )
}

fn ai_prompt(mode: &str, text: &str, target: &str, style: Option<&str>, polish_level: &str) -> String {
    match mode {
        "explain" => explanation_prompt(text),
        "polish" => polish_prompt(text, polish_level),
        _ => translation_prompt(text, target, style),
    }
}

fn explanation_prompt(text: &str) -> String {
    format!(
        "你是词典和语言老师。对下面这段文字，用简体中文解释：\n\
         1. 先一句话说明它的含义；\n\
         2. 如果是单词或短语，给出词性、音标和一到两个例句；\n\
         3. 说明它在这个语境里是哪个意思。\n\
         最多写几行，不要长篇大论。\n\n{text}"
    )
}

fn polish_prompt(text: &str, level: &str) -> String {
    let level_rule = match level {
        "light" => "只改拼写和语法错误，其余一律不动。",
        "heavy" => "可以大幅改写句子结构和措辞，让表达更流畅，但原意和所有信息必须保留。",
        _ => "改措辞、语序和句子结构，删掉啰嗦和重复，让表达自然精准。",
    };
    format!(
        "You are a careful copy editor. Polish the text below in the same language as the input.\n\
         Rules:\n\
         - Keep the meaning, technical terms, symbols, formulas, citations and numbers unchanged.\n\
         - Fix typos, grammar, word choice and awkward phrasing.\n\
         - {level_rule}\n\
         - Do not add or remove information. Keep the paragraph structure.\n\
         - If the text is already fine, output it unchanged.\n\
         - Any ⟦number⟧ marker is a placeholder: copy it exactly as-is.\n\
         - Output only the polished text, nothing else.\n\n{text}"
    )
}

fn explain_diff_prompt(original: &str, polished: &str) -> String {
    format!(
        "下面是一段原文和润色后的文字。请用简体中文简要说明做了哪些修改、为什么这样改。\n\
         要求：\n\
         - 用一句话先概括主要改动；\n\
         - 再列出 1 到 3 个具体改动点（原文的哪个词/句子 → 改成了什么 → 为什么）；\n\
         - 总共不超过三行，不要复述全文。\n\n\
         原文：{original}\n\n润色后：{polished}"
    )
}

/// Explain what changed in a polish, using the selected AI engine. Non-streaming.
pub async fn explain_diff(
    original: &str,
    polished: &str,
    source: &str,
    keys: &ProviderKeys,
) -> Result<String, String> {
    let prompt = explain_diff_prompt(original, polished);
    if openai_preset(source).is_none() {
        return Err("当前引擎不支持解释改动。请改用其他引擎。".into());
    }
    chat_with_preset(source, &prompt, keys, &|_| {}, false).await
}

async fn chat_translation(
    url: &str,
    model: &str,
    prompt: &str,
    bearer: Option<&str>,
    empty_error: &str,
    on_chunk: &(dyn Fn(String) + Send + Sync),
    stream: bool,
) -> Result<String, String> {
    cancel_ai();
    let epoch = AI_GEN.load(Ordering::SeqCst);
    let mut cancel_rx = CANCEL_TX.subscribe();
    let payload = serde_json::json!({
        "model": model,
        "stream": stream,
        "messages": [{ "role": "user", "content": prompt }]
    });
    // 本机 Ollama 是 CPU/GPU 推理，冷启动可能很慢，超时放宽到 5 分钟；
    // 云端服务 45 秒仍足够。
    let timeout = if url.starts_with("http://localhost:11434") || url.starts_with("http://127.0.0.1:11434") {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(45)
    };
    let mut request = HTTP_CLIENT
        .post(url)
        .json(&payload)
        .timeout(timeout);
    if let Some(key) = bearer {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let secrets: Vec<&str> = bearer.into_iter().collect();
    let response = tokio::select! {
        _ = cancel_rx.changed() => {
            return Err(CANCELLED.into());
        }
        response = request.send() => response.map_err(|err| redact(err.to_string(), &secrets))?,
    };
    if AI_GEN.load(Ordering::SeqCst) != epoch {
        return Err(CANCELLED.into());
    }
    let status = response.status();
    if !status.is_success() {
        let body = read_error_body(response).await;
        return Err(http_error(status, &body, &secrets));
    }
    if !stream {
        let body: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
        let translated = body
            .pointer("/choices/0/message/content")
            .and_then(|value| value.as_str())
            .map(model_output)
            .unwrap_or_default();
        if translated.is_empty() {
            return Err(empty_error.to_string());
        }
        on_chunk(translated.clone());
        return Ok(translated);
    }
    let mut stream = response.bytes_stream();
    let mut pending = String::new();
    let mut acc = String::new();
    loop {
        if AI_GEN.load(Ordering::SeqCst) != epoch {
            return Err(CANCELLED.into());
        }
        let item = tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_err() || AI_GEN.load(Ordering::SeqCst) != epoch {
                    return Err(CANCELLED.into());
                }
                continue;
            }
            item = stream.next() => item,
        };
        let Some(item) = item else { break };
        let bytes = item.map_err(|err| redact(err.to_string(), &secrets))?;
        pending.push_str(&String::from_utf8_lossy(&bytes));
        absorb_sse(&mut pending, &mut acc, on_chunk);
    }
    if acc.is_empty() {
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(pending.trim()) {
            acc = body
                .pointer("/choices/0/message/content")
                .and_then(|value| value.as_str())
                .map(model_output)
                .unwrap_or_default();
            if !acc.is_empty() {
                on_chunk(acc.clone());
            }
        }
    }
    let translated = model_output(&acc);
    if translated.is_empty() {
        Err(empty_error.to_string())
    } else {
        Ok(translated)
    }
}

fn absorb_sse(pending: &mut String, acc: &mut String, on_chunk: &(dyn Fn(String) + Send + Sync)) {
    while let Some(split) = pending.find("\n\n") {
        let block = pending[..split].to_string();
        pending.replace_range(..split + 2, "");
        for line in block.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let piece = json
                .pointer("/choices/0/delta/content")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    json.pointer("/choices/0/message/content")
                        .and_then(|value| value.as_str())
                });
            if let Some(piece) = piece {
                acc.push_str(piece);
                on_chunk(acc.clone());
            }
        }
    }
}

fn model_output(raw: &str) -> String {
    raw.trim().trim_matches('"').trim().to_string()
}

fn ready(text: String, service: &str) -> Column {
    Column {
        text,
        service: service.to_string(),
        error: String::new(),
    }
}

fn failed(error: impl Into<String>) -> Column {
    Column {
        text: String::new(),
        service: String::new(),
        error: error.into(),
    }
}

fn from_result(result: Result<String, String>, service: &str) -> Column {
    match result {
        Ok(text) => ready(text, service),
        Err(err) if err == CANCELLED => Column {
            text: String::new(),
            service: CANCELLED.into(),
            error: String::new(),
        },
        Err(err) => failed(err),
    }
}

pub fn target_lang(text: &str) -> &'static str {
    if text.chars().any(is_cjk) {
        "en"
    } else {
        "zh-CN"
    }
}

pub fn direction_label(target: &str) -> &'static str {
    if target == "en" {
        "中文 → English"
    } else {
        "English → 中文"
    }
}

pub(crate) fn is_cjk(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_can_be_forced() {
        assert_eq!(resolve_target("hello", "zh-en"), "en");
        assert_eq!(resolve_target("你好", "en-zh"), "zh-CN");
        assert_eq!(resolve_target("你好", "auto"), "en");
        assert_eq!(resolve_target("hello", "auto"), "zh-CN");
    }

    #[test]
    fn academic_style_is_part_of_the_prompt() {
        let prompt = translation_prompt("你好", "en", Some("academic"));
        assert!(prompt.contains("academic"));
        assert!(prompt.contains("你好"));
    }

    #[test]
    fn modes_pick_the_right_prompt() {
        assert!(ai_prompt("explain", "hello", "zh-CN", None, "standard").contains("词典"));
        assert!(ai_prompt("polish", "hello", "zh-CN", None, "standard").contains("copy editor"));
        assert!(ai_prompt("polish", "hello", "zh-CN", None, "light").contains("只改拼写和语法"));
        assert!(ai_prompt("translate", "hello", "zh-CN", None, "standard").contains("Translate"));
    }

    #[test]
    fn explain_diff_prompt_includes_both_texts() {
        let prompt = explain_diff_prompt("原文", "润色后");
        assert!(prompt.contains("原文"));
        assert!(prompt.contains("润色后"));
        assert!(prompt.contains("为什么"));
    }

    #[test]
    fn sse_chunks_accumulate() {
        use std::sync::Mutex;
        let mut pending = "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n".to_string();
        let mut acc = String::new();
        let seen = Mutex::new(String::new());
        absorb_sse(&mut pending, &mut acc, &|text| {
            *seen.lock().unwrap() = text;
        });
        assert_eq!(acc, "He");
        assert_eq!(seen.lock().unwrap().as_str(), "He");
    }

    #[test]
    fn deepseek_uses_a_saved_key_without_a_master_password() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let keys = ProviderKeys {
            deepl: String::new(),
            deepseek: String::new(),
            openai: String::new(),
            moonshot: String::new(),
            gemini: String::new(),
            ollama_model: String::new(),
        };
        let (_text, _service, error) = runtime.block_on(translate_ai(
            "你好",
            "en",
            "deepseek",
            None,
            "translate",
            "standard",
            &keys,
            &|_| {},
        ));
        assert!(error.contains("还没有 DeepSeek API Key"), "{error}");

        let keys = ProviderKeys {
            deepl: String::new(),
            deepseek: "sk-test-value".into(),
            openai: String::new(),
            moonshot: String::new(),
            gemini: String::new(),
            ollama_model: String::new(),
        };
        let (_text, _service, error) = runtime.block_on(translate_ai(
            "你好",
            "en",
            "deepseek",
            None,
            "translate",
            "standard",
            &keys,
            &|_| {},
        ));
        assert!(!error.contains("还没有 DeepSeek API Key"), "{error}");
        assert!(!error.contains("主密码"), "{error}");
    }
}

async fn http_get_json(url: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let response = HTTP_CLIENT
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let body = read_error_body(response).await;
        return Err(http_error(status, &body, &[]));
    }
    response.json().await.map_err(|err| err.to_string())
}

async fn send_json(
    request: reqwest::RequestBuilder,
    secrets: &[&str],
) -> Result<serde_json::Value, String> {
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => return Err(redact(err.to_string(), secrets)),
    };
    let status = response.status();
    if !status.is_success() {
        let body = read_error_body(response).await;
        return Err(http_error(status, &body, secrets));
    }
    match response.json().await {
        Ok(value) => Ok(value),
        Err(err) => Err(redact(err.to_string(), secrets)),
    }
}

async fn read_error_body(response: reqwest::Response) -> String {
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return String::new(),
    };
    let end = bytes.len().min(8 * 1024);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn http_error(status: reqwest::StatusCode, body: &str, secrets: &[&str]) -> String {
    let snippet = truncate_chars(&redact(body.to_string(), secrets), 180);
    if snippet.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status} {snippet}")
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut flattened = String::new();
    for ch in text.chars() {
        if ch.is_control() {
            if !flattened.ends_with(' ') {
                flattened.push(' ');
            }
        } else {
            flattened.push(ch);
        }
    }
    let trimmed = flattened.trim();
    let mut end = trimmed.len();
    for (count, (idx, _)) in trimmed.char_indices().enumerate() {
        if count == max_chars {
            end = idx;
            break;
        }
    }
    trimmed[..end].trim().to_string()
}

fn redact(message: String, secrets: &[&str]) -> String {
    if message.is_empty() {
        return message;
    }
    let mut needles = Vec::new();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        needles.push((*secret).to_string());
        let encoded = urlencoding::encode(secret);
        if encoded.as_ref() != *secret {
            needles.push(encoded.into_owned());
        }
    }
    needles.sort_by_key(|needle| Reverse(needle.len()));
    needles.dedup();
    let mut out = message;
    for needle in &needles {
        if !needle.is_empty() && out.contains(needle) {
            out = out.replace(needle, "[redacted]");
        }
    }
    out
}
