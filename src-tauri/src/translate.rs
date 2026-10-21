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
    keys: &ProviderKeys,
    model_dir: &Path,
) -> CompareTranslation {
    let (mt, ai) = tokio::join!(
        mt_column(text, target, mt_source, model_dir, &|_| {}),
        ai_column(text, target, ai_source, style, keys, &|_| {}),
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
    on_status: &(dyn Fn(String) + Send + Sync),
) -> (String, String, String) {
    let column = mt_column(text, target, source, model_dir, on_status).await;
    (column.text, column.service, column.error)
}

pub async fn translate_ai(
    text: &str,
    target: &str,
    source: &str,
    style: Option<&str>,
    keys: &ProviderKeys,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> (String, String, String) {
    cancel_ai();
    let column = ai_column(text, target, source, style, keys, on_chunk).await;
    let secrets = [keys.deepl.as_str(), keys.deepseek.as_str()];
    (
        column.text,
        column.service,
        redact(column.error, &secrets),
    )
}

async fn mt_column(
    text: &str,
    target: &str,
    source: &str,
    model_dir: &Path,
    on_status: &(dyn Fn(String) + Send + Sync),
) -> Column {
    match source {
        "google" => google_column(text, target, model_dir, on_status).await,
        "mymemory" => mymemory_column(text, target).await,
        "offline" => offline_column(text, target, model_dir, on_status).await,
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
        return failed("还没有 DeepL API Key。保存时要输入主密码，主密码不会保存。");
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
    keys: &ProviderKeys,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> Column {
    match source {
        "free" => free_column(text, target, style, on_chunk).await,
        "deepseek" => deepseek_column(text, target, style, &keys.deepseek, on_chunk).await,
        "deepl" => deepl_column(text, target, &keys.deepl, style).await,
        _ => failed("不认识的对照翻译。"),
    }
}

async fn free_column(
    text: &str,
    target: &str,
    style: Option<&str>,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> Column {
    let result = chat_translation(
        "https://text.pollinations.ai/openai",
        "openai",
        &translation_prompt(text, target, style),
        None,
        "AI 没有返回译文。",
        on_chunk,
        true,
    )
    .await;
    from_result(result, "free")
}

async fn deepseek_column(
    text: &str,
    target: &str,
    style: Option<&str>,
    key: &str,
    on_chunk: &(dyn Fn(String) + Send + Sync),
) -> Column {
    if key.is_empty() {
        return failed("还没有 DeepSeek API Key。保存时要输入主密码，主密码不会保存。");
    }
    let result = chat_translation(
        "https://api.deepseek.com/chat/completions",
        "deepseek-chat",
        &translation_prompt(text, target, style),
        Some(key),
        "DeepSeek 没有返回译文。",
        on_chunk,
        true,
    )
    .await;
    from_result(result, "deepseek")
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
        "Translate the following text to {language}. {tone} Output only the translation, with no notes or quotes.\n\n{text}"
    )
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
    let mut request = HTTP_CLIENT
        .post(url)
        .json(&payload)
        .timeout(Duration::from_secs(45));
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

fn is_cjk(ch: char) -> bool {
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
