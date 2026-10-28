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

/// Translate every non-skipped paragraph concurrently, one request per block,
/// then reorder the results by index so the reading order is preserved. A block
/// that fails keeps its original text and records the error without blocking
/// the others. Skipped blocks (formulae, code, borders) are left untranslated.
pub async fn translate_paragraphs(
    paragraphs: Vec<Paragraph>,
    target: &'static str,
    translator: Arc<dyn BlockTranslator>,
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

    // Up to 6 in flight; the first block to finish is consumed first, but each
    // result is written back to its own slot so the order never changes.
    let mut stream = futures_util::stream::iter(jobs).buffer_unordered(6);
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
