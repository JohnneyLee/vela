//! Protect fragments that must not be translated: URLs, emails, @mentions,
//! #hashtags, code spans, placeholders, and emoji. Fragments are replaced with
//! numbered tokens like `⟦0⟧` and restored after translation.

use std::sync::LazyLock;

use regex::Regex;

static TOKEN_START: &str = "\u{27e6}";
static TOKEN_END: &str = "\u{27e7}";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // URLs
        Regex::new(r"https?://[^\s]+|www\.[^\s]+").unwrap(),
        // Emails
        Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap(),
        // @mentions
        Regex::new(r"@[\w\u{4e00}-\u{9fff}-]+").unwrap(),
        // #hashtags
        Regex::new(r"#[\w\u{4e00}-\u{9fff}-]+").unwrap(),
        // Fenced code blocks
        Regex::new(r"(?s)```.*?```|`[^`\n]*`").unwrap(),
        // Placeholders: {{...}}, {...}, %s, %d
        Regex::new(r"\{\{.*?\}\}|\{[^{}\n]*\}|%[sd]").unwrap(),
    ]
});

pub struct Protected {
    pub text: String,
    saved: Vec<String>,
}

impl Protected {
    pub fn saved(&self) -> &[String] {
        &self.saved
    }

    pub fn has_fragments(&self) -> bool {
        !self.saved.is_empty()
    }
}

pub fn protect(input: &str) -> Protected {
    let mut saved: Vec<String> = Vec::new();
    let mut working = input.to_string();

    for re in PATTERNS.iter() {
        let mut next = String::with_capacity(working.len());
        let mut last = 0;
        for m in re.find_iter(&working) {
            next.push_str(&working[last..m.start()]);
            let idx = saved.len();
            saved.push(m.as_str().to_string());
            next.push_str(&token(idx));
            last = m.end();
        }
        next.push_str(&working[last..]);
        working = next;
    }

    // Emoji: surrogate pairs and common ranges, replaced one run at a time.
    let mut out = String::with_capacity(working.len());
    let chars: Vec<char> = working.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_emoji(c) {
            let start = i;
            // Consume a full emoji sequence (base + variation selector + ZWJ runs).
            let mut j = i + 1;
            while j < chars.len() {
                let n = chars[j];
                if is_emoji(n)
                    || n == '\u{FE0F}'
                    || n == '\u{200D}'
                    || is_regional(n)
                    || is_skin_tone(n)
                {
                    j += 1;
                } else {
                    break;
                }
            }
            let idx = saved.len();
            let emoji: String = chars[start..j].iter().collect();
            saved.push(emoji);
            out.push_str(&token(idx));
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }

    Protected { text: out, saved }
}

pub fn restore(output: &str, saved: &[String]) -> Option<String> {
    let mut result = output.to_string();
    let mut seen = vec![0usize; saved.len()];
    for (i, item) in saved.iter().enumerate() {
        let needle = token(i);
        let count = result.matches(&needle).count();
        if count != 1 {
            return None;
        }
        seen[i] = 1;
        result = result.replace(&needle, item);
    }
    if seen.iter().all(|&s| s == 1) {
        Some(result)
    } else {
        None
    }
}

/// Replace complete tokens only; leave an unclosed token prefix untouched. Used
/// while a translation is still streaming in.
pub fn partial_restore(output: &str, saved: &[String]) -> String {
    let mut result = output.to_string();
    for (i, item) in saved.iter().enumerate() {
        let needle = token(i);
        result = result.replace(&needle, item);
    }
    result
}

/// Lenient restore: replace complete tokens and drop any leftover marker
/// brackets, so the user never sees stray `⟦` characters.
pub fn restore_loose(output: &str, saved: &[String]) -> String {
    static RESIDUAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new("\u{27e6}[^\u{27e7}]*\u{27e7}?").unwrap()
    });
    let replaced = partial_restore(output, saved);
    RESIDUAL.replace_all(&replaced, "").to_string()
}

fn token(idx: usize) -> String {
    format!("{TOKEN_START}{idx}{TOKEN_END}")
}

fn is_emoji(c: char) -> bool {
    let u = c as u32;
    (0x1F300..=0x1FAFF).contains(&u)
        || (0x2600..=0x27BF).contains(&u)
        || (0x2300..=0x23FF).contains(&u)
        || (0x2B00..=0x2BFF).contains(&u)
        || (0xFE00..=0xFE0F).contains(&u)
}

fn is_regional(c: char) -> bool {
    let u = c as u32;
    (0x1F1E6..=0x1F1FF).contains(&u)
}

fn is_skin_tone(c: char) -> bool {
    let u = c as u32;
    (0x1F3FB..=0x1F3FF).contains(&u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_mentions_and_placeholders_roundtrip() {
        let input = "看看 https://a.com @小王 {名字} 这个";
        let p = protect(input);
        assert!(!p.text.contains("https://"));
        assert!(!p.text.contains("@小王"));
        let restored = restore(&p.text, &p.saved).unwrap();
        assert_eq!(restored, input);
    }

    #[test]
    fn missing_token_fails_restore() {
        let p = protect("hello https://a.com");
        // Simulate an engine that dropped the token.
        let bad = "你好";
        assert!(restore(bad, &p.saved).is_none());
    }

    #[test]
    fn emoji_is_protected() {
        let input = "ok 👍 done";
        let p = protect(input);
        assert!(!p.text.contains('👍'));
        let restored = restore(&p.text, &p.saved).unwrap();
        assert_eq!(restored, input);
    }

    #[test]
    fn partial_restore_leaves_unclosed_token() {
        let p = protect("看看 https://a.com 这个");
        // A streamed prefix: token not yet complete.
        let streaming = "Look at ⟦0".to_string();
        let out = partial_restore(&streaming, p.saved());
        assert_eq!(out, "Look at ⟦0");
        // Once complete, it is replaced.
        let complete = "Look at ⟦0⟧".to_string();
        let out = partial_restore(&complete, p.saved());
        assert_eq!(out, "Look at https://a.com");
    }

    #[test]
    fn restore_loose_drops_stray_markers() {
        let p = protect("hello https://a.com");
        let out = restore_loose("你好 ⟦0⟧ 以及 ⟦", p.saved());
        assert!(out.contains("https://a.com"));
        assert!(!out.contains('\u{27e6}'));
    }
}
