//! Recommend which action to highlight on the action bar. Pure function, no
//! side effects; the recommendation only affects highlight, never executes.

/// A coarse language guess for the selected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
    Other,
}

/// Scene info gathered at trigger time.
pub struct Hint {
    /// Whether the selection is inside an editable text field.
    pub editable: bool,
    pub lang: Lang,
    pub word_count: usize,
    /// Fraction of symbol characters, a rough proxy for code / formulas.
    pub symbol_ratio: f32,
    pub chars: usize,
}

/// Which action to highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Translate,
    Explain,
    Polish,
    Speak,
}

impl Action {
    pub fn id(&self) -> &'static str {
        match self {
            Action::Translate => "translate",
            Action::Explain => "explain",
            Action::Polish => "polish",
            Action::Speak => "speak",
        }
    }
}

pub fn suggest(h: &Hint, native: Lang) -> Action {
    if h.symbol_ratio > 0.3 {
        return Action::Explain;
    }
    if h.lang == native {
        return if h.editable { Action::Polish } else { Action::Explain };
    }
    if h.editable {
        return Action::Polish;
    }
    Action::Translate
}

/// Detect a coarse language from text without pulling in a full detector.
pub fn detect_lang(text: &str) -> Lang {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            cjk += 1;
        } else if ch.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cjk > latin {
        Lang::Zh
    } else if latin > 0 {
        Lang::En
    } else {
        Lang::Other
    }
}

pub fn symbol_ratio(text: &str) -> f32 {
    let mut total = 0usize;
    let mut symbols = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        let is_symbol = !ch.is_alphanumeric() && !('\u{4E00}'..='\u{9FFF}').contains(&ch);
        if is_symbol {
            symbols += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        symbols as f32 / total as f32
    }
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> Lang {
        Lang::Zh
    }
    fn en() -> Lang {
        Lang::En
    }

    #[test]
    fn code_heavy_text_suggests_explain() {
        let h = Hint {
            editable: false,
            lang: Lang::Other,
            word_count: 10,
            symbol_ratio: 0.5,
            chars: 40,
        };
        assert_eq!(suggest(&h, zh()), Action::Explain);
    }

    #[test]
    fn foreign_readonly_suggests_translate() {
        let h = Hint {
            editable: false,
            lang: en(),
            word_count: 8,
            symbol_ratio: 0.05,
            chars: 60,
        };
        assert_eq!(suggest(&h, zh()), Action::Translate);
    }

    #[test]
    fn native_readonly_suggests_explain() {
        let h = Hint {
            editable: false,
            lang: zh(),
            word_count: 8,
            symbol_ratio: 0.05,
            chars: 60,
        };
        assert_eq!(suggest(&h, zh()), Action::Explain);
    }

    #[test]
    fn native_editable_suggests_polish() {
        let h = Hint {
            editable: true,
            lang: zh(),
            word_count: 8,
            symbol_ratio: 0.05,
            chars: 60,
        };
        assert_eq!(suggest(&h, zh()), Action::Polish);
    }

    #[test]
    fn foreign_editable_suggests_polish() {
        let h = Hint {
            editable: true,
            lang: en(),
            word_count: 8,
            symbol_ratio: 0.05,
            chars: 60,
        };
        assert_eq!(suggest(&h, zh()), Action::Polish);
    }

    #[test]
    fn symbol_ratio_counts_punctuation() {
        assert!(symbol_ratio("int x = a + b; // code") > 0.3);
        assert!(symbol_ratio("这是一个普通的句子。") < 0.3);
    }

    #[test]
    fn detect_lang_prefers_cjk() {
        assert_eq!(detect_lang("你好世界"), Lang::Zh);
        assert_eq!(detect_lang("hello world"), Lang::En);
        assert_eq!(detect_lang("你好a"), Lang::Zh);
    }
}
