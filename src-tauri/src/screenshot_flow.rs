//! Screenshot-translation flow shared by the OCR overlay steps (O4–O8).
//! Translated text and screenshots stay in memory and are cleared on close.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::paragraph::Paragraph;
use crate::translate;

/// One translated block plus its bounding box, positioned for the O5
/// "cover in place" step. `translated` is empty for skipped blocks and for
/// blocks whose translation failed (the caller falls back to the original).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslatedBlock {
    pub index: usize,
    pub original: String,
    pub translated: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub skip: bool,
    pub error: String,
}

/// A block ready to be painted over the screenshot (O5). Only blocks with a
/// non-empty `translated` are painted; skipped and failed blocks leave the
/// original image showing.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderBlock {
    pub original: String,
    pub translated: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub skip: bool,
    /// Background colour as `#rrggbb`, sampled from the block's border.
    pub bg_color: String,
    /// Text colour (`#000000` or `#ffffff`) chosen by background luminance.
    pub text_color: String,
    /// True when the border is uniform enough to fill opaquely.
    pub solid_bg: bool,
}

/// Attach background/text colours to each block by sampling the screenshot.
pub fn prepare_render(
    blocks: Vec<TranslatedBlock>,
    pixels: &[u8],
    img_w: i32,
    img_h: i32,
) -> Vec<RenderBlock> {
    blocks
        .into_iter()
        .map(|b| {
            let (r, g, bl, solid) =
                crate::screenshot::sample_background(pixels, img_w, img_h, b.x, b.y, b.width, b.height);
            let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * bl as f32;
            let text_color = if luminance > 140.0 { "#000000" } else { "#ffffff" };
            RenderBlock {
                original: b.original,
                translated: b.translated,
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
                skip: b.skip,
                bg_color: format!("#{r:02x}{g:02x}{bl:02x}"),
                text_color: text_color.to_string(),
                solid_bg: solid,
            }
        })
        .collect()
}

/// Swappable "translate one text" step, so the reorder and error-isolation
/// logic can be unit-tested without any network call.
#[async_trait]
pub trait BlockTranslator: Send + Sync {
    async fn translate(&self, text: &str, target: &str) -> Result<String, String>;
}

/// A real translator backed by the machine-translation engines.
pub struct MtTranslator {
    pub source: String,
    pub model_dir: PathBuf,
    pub deepl_key: String,
}

#[async_trait]
impl BlockTranslator for MtTranslator {
    async fn translate(&self, text: &str, target: &str) -> Result<String, String> {
        let on_status = |_: String| {};
        let (out, _service, err) = translate::translate_mt(
            text,
            target,
            &self.source,
            &self.model_dir,
            &self.deepl_key,
            &on_status,
        )
        .await;
        if !err.is_empty() {
            return Err(err);
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            return Err("翻译返回空结果。".into());
        }
        Ok(out)
    }
}

pub fn mt_translator(source: String, model_dir: PathBuf, deepl_key: String) -> Arc<dyn BlockTranslator> {
    Arc::new(MtTranslator {
        source,
        model_dir,
        deepl_key,
    })
}

/// AI 段落翻译（截图盖回原位只能显示一路译文，按设置选用）。
pub struct AiTranslator {
    pub source: String,
    pub style: Option<String>,
    pub native: String,
    pub keys: translate::ProviderKeys,
}

#[async_trait]
impl BlockTranslator for AiTranslator {
    async fn translate(&self, text: &str, target: &str) -> Result<String, String> {
        let on_chunk = |_: String| {};
        let (out, _service, err) = translate::translate_ai(
            text,
            target,
            &self.source,
            self.style.as_deref(),
            "translate",
            "standard",
            "",
            &self.native,
            &self.keys,
            &on_chunk,
        )
        .await;
        if !err.is_empty() {
            return Err(err);
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            return Err("翻译返回空结果。".into());
        }
        Ok(out)
    }
}

pub fn ai_translator(
    source: String,
    style: Option<String>,
    native: String,
    keys: translate::ProviderKeys,
) -> Arc<dyn BlockTranslator> {
    Arc::new(AiTranslator {
        source,
        style,
        native,
        keys,
    })
}

/// 盖回原位只能显示一路译文：ai-first / compare 用 AI；mt-first 用机器翻译；
/// auto 则有可用 AI（含免费 / Ollama）时用 AI，否则机器翻译。
pub fn prefer_ai_for_overlay(display_mode: &str, ai_source: &str, keys: &translate::ProviderKeys) -> bool {
    let ai_ready = match ai_source {
        "free" | "ollama" => true,
        "deepseek" => !keys.deepseek.is_empty(),
        "openai" => !keys.openai.is_empty(),
        "moonshot" => !keys.moonshot.is_empty(),
        "gemini" => !keys.gemini.is_empty(),
        "deepl" => !keys.deepl.is_empty(),
        _ => false,
    };
    match display_mode {
        "ai-first" | "compare" => ai_ready,
        "mt-first" => false,
        "auto" => ai_ready,
        _ => false,
    }
}

/// Translate every non-skipped paragraph concurrently, one request per block,
/// then reorder the results by index so the reading order is preserved. A block
/// that fails keeps its original text and records the error without blocking
/// the others. Skipped blocks (formulae, code, borders) are left untranslated.
pub async fn translate_paragraphs(
    paragraphs: Vec<Paragraph>,
    target: &'static str,
    translator: Arc<dyn BlockTranslator>,
) -> Vec<TranslatedBlock> {
    translate_paragraphs_with_concurrency(paragraphs, target, translator, 6).await
}

pub async fn translate_paragraphs_with_concurrency(
    paragraphs: Vec<Paragraph>,
    target: &'static str,
    translator: Arc<dyn BlockTranslator>,
    concurrency: usize,
) -> Vec<TranslatedBlock> {
    use futures_util::StreamExt;

    let count = paragraphs.len();
    let mut translated = vec![String::new(); count];
    let mut errors = vec![String::new(); count];

    // Build the futures in a plain loop so each one only captures owned data
    // (text, Arc, 'static target) and never borrows `paragraphs`.
    let mut jobs = Vec::new();
    for (i, p) in paragraphs.iter().enumerate() {
        if p.skip || p.text.trim().is_empty() {
            continue;
        }
        let text = p.text.clone();
        let t = translator.clone();
        jobs.push(async move {
            let result = t.translate(&text, target).await;
            (i, result)
        });
    }

    // AI 路径用 1：translate_ai 会 cancel 上一次请求，不能并行。
    let concurrency = concurrency.max(1);
    let mut stream = futures_util::stream::iter(jobs).buffer_unordered(concurrency);
    while let Some((i, result)) = stream.next().await {
        match result {
            Ok(text) => translated[i] = text,
            Err(err) => errors[i] = err,
        }
    }

    paragraphs
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let error = if p.skip || p.text.trim().is_empty() {
                String::new()
            } else if !errors[i].is_empty() {
                errors[i].clone()
            } else if translated[i].trim().is_empty() {
                "翻译返回空结果。".to_string()
            } else {
                String::new()
            };
            TranslatedBlock {
                index: i,
                original: p.text.clone(),
                translated: translated[i].clone(),
                x: p.x,
                y: p.y,
                width: p.width,
                height: p.height,
                skip: p.skip,
                error,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTranslator;

    #[async_trait]
    impl BlockTranslator for FakeTranslator {
        async fn translate(&self, text: &str, _target: &str) -> Result<String, String> {
            if text == "FAIL" {
                return Err("mock failure".into());
            }
            Ok(format!("T:{text}"))
        }
    }

    fn para(text: &str, y: f32, skip: bool) -> Paragraph {
        Paragraph {
            text: text.into(),
            x: 0.0,
            y,
            width: 100.0,
            height: 20.0,
            skip,
        }
    }

    #[tokio::test]
    async fn keeps_order_and_isolates_failure() {
        let paras = vec![
            para("第一段", 0.0, false),
            para("FAIL", 30.0, false),
            para("第三段", 60.0, false),
        ];
        let blocks = translate_paragraphs(paras, "en", Arc::new(FakeTranslator)).await;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].index, 0);
        assert_eq!(blocks[0].translated, "T:第一段");
        assert!(blocks[0].error.is_empty());

        assert_eq!(blocks[1].index, 1);
        assert!(blocks[1].translated.is_empty());
        assert!(!blocks[1].error.is_empty());
        assert_eq!(blocks[1].original, "FAIL");

        assert_eq!(blocks[2].index, 2);
        assert_eq!(blocks[2].translated, "T:第三段");
        assert!(blocks[2].error.is_empty());
    }

    #[tokio::test]
    async fn skip_block_is_left_untranslated() {
        let paras = vec![
            para("$$\\sum x$$", 0.0, true),
            para("正文", 30.0, false),
        ];
        let blocks = translate_paragraphs(paras, "en", Arc::new(FakeTranslator)).await;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].skip);
        assert!(blocks[0].translated.is_empty());
        assert!(blocks[0].error.is_empty());
        assert_eq!(blocks[1].translated, "T:正文");
    }

    #[test]
    fn prefer_ai_follows_display_mode() {
        let keys = translate::ProviderKeys {
            deepl: String::new(),
            deepseek: "sk".into(),
            openai: String::new(),
            moonshot: String::new(),
            gemini: String::new(),
            ollama_model: String::new(),
        };
        assert!(prefer_ai_for_overlay("ai-first", "deepseek", &keys));
        assert!(prefer_ai_for_overlay("compare", "free", &keys));
        assert!(!prefer_ai_for_overlay("mt-first", "deepseek", &keys));
        assert!(!prefer_ai_for_overlay("ai-first", "deepseek", &translate::ProviderKeys {
            deepl: String::new(),
            deepseek: String::new(),
            openai: String::new(),
            moonshot: String::new(),
            gemini: String::new(),
            ollama_model: String::new(),
        }));
    }

    #[tokio::test]
    async fn empty_block_is_left_alone() {
        let paras = vec![
            para("   ", 0.0, false),
            para("正文", 30.0, false),
        ];
        let blocks = translate_paragraphs(paras, "en", Arc::new(FakeTranslator)).await;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].translated.is_empty());
        assert!(blocks[0].error.is_empty());
        assert_eq!(blocks[1].translated, "T:正文");
    }
}
