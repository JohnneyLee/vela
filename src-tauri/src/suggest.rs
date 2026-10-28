//! Recommend which action to highlight on the action bar. Pure function, no
//! side effects; the recommendation only affects highlight, never executes.

/// A coarse language guess for the selected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
    Ja,
    Other,
}

/// 把设置的母语代码转成 Lang。未知代码按中文处理（默认值也是中文）。
pub fn native_lang(code: &str) -> Lang {
    match code {
        "en" => Lang::En,
        "ja" => Lang::Ja,
        _ => Lang::Zh,
    }
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

/// 根据文字内容 + 母语推荐打开结果窗时的默认动作 id。
/// 纯函数，不读取、不沿用上一次使用过的动作。
pub fn recommend_id(text: &str, editable: bool, native: Lang) -> &'static str {
    let hint = Hint {
        editable,
        lang: detect_lang(text),
        word_count: word_count(text),
        symbol_ratio: symbol_ratio(text),
        chars: text.chars().count(),
    };
    suggest(&hint, native).id()
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
    fn ja() -> Lang {
        Lang::Ja
    }

    fn hint(lang: Lang, editable: bool, text: &str) -> Hint {
        Hint {
            editable,
            lang,
            word_count: word_count(text),
            symbol_ratio: symbol_ratio(text),
            chars: text.chars().count(),
        }
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

    // 母语英文：选中中文是外语 → 译；选中英文是母语 → 释/润。
    #[test]
    fn en_native_chinese_suggests_translate() {
        let h = hint(detect_lang("你好世界"), false, "你好世界");
        assert_eq!(suggest(&h, en()), Action::Translate);
    }

    #[test]
    fn en_native_english_readonly_suggests_explain() {
        let h = hint(detect_lang("hello world"), false, "hello world");
        assert_eq!(suggest(&h, en()), Action::Explain);
    }

    #[test]
    fn en_native_english_editable_suggests_polish() {
        let h = hint(detect_lang("hello world"), true, "hello world");
        assert_eq!(suggest(&h, en()), Action::Polish);
    }

    // 母语英文下的单词（detect_lang 判为英文）是母语 → 释。
    #[test]
    fn en_native_single_word_suggests_explain() {
        let h = hint(detect_lang("serendipity"), false, "serendipity");
        assert_eq!(suggest(&h, en()), Action::Explain);
    }

    // 母语日文：英文是外语 → 译；可编辑 → 润。
    #[test]
    fn ja_native_english_suggests_translate() {
        let h = hint(detect_lang("hello world"), false, "hello world");
        assert_eq!(suggest(&h, ja()), Action::Translate);
    }

    #[test]
    fn ja_native_english_editable_suggests_polish() {
        let h = hint(detect_lang("hello world"), true, "hello world");
        assert_eq!(suggest(&h, ja()), Action::Polish);
    }

    // 母语日文下，纯代码仍是释（符号占比高优先于语言判断）。
    #[test]
    fn ja_native_code_suggests_explain() {
        let h = hint(Lang::Other, false, "int x = a + b;");
        assert_eq!(suggest(&h, ja()), Action::Explain);
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
