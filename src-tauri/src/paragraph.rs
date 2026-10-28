//! Merge OCR lines into paragraphs using geometric heuristics. All thresholds
//! are relative (multiples of the median line height), never fixed pixels.

use crate::ocr::OcrLineBox;

/// A merged paragraph. `skip` marks blocks that must not be translated
/// (formulae, code, table borders — mostly symbols).
#[derive(Clone, Debug)]
pub struct Paragraph {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub skip: bool,
}

pub fn merge(lines: Vec<OcrLineBox>) -> Vec<Paragraph> {
    if lines.is_empty() {
        return Vec::new();
    }

    let median_h = median_line_height(&lines);
    // 栏宽：用最宽行的宽度作为该栏的参考（写满判断用它）。
    let cols = split_columns(lines, median_h);

    let mut out = Vec::new();
    for col in cols {
        merge_column(col, median_h, &mut out);
    }
    out
}

fn median_line_height(lines: &[OcrLineBox]) -> f32 {
    let mut hs: Vec<f32> = lines.iter().map(|l| l.height.max(1.0)).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = hs.len() / 2;
    if hs.is_empty() {
        1.0
    } else if hs.len() % 2 == 1 {
        hs[mid]
    } else {
        (hs[mid - 1] + hs[mid]) / 2.0
    }
}

/// Split lines into columns by horizontal-interval overlap, left to right.
fn split_columns(lines: Vec<OcrLineBox>, median_h: f32) -> Vec<Vec<OcrLineBox>> {
    let mut sorted = lines;
    sorted.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));

    let mut cols: Vec<Vec<OcrLineBox>> = Vec::new();
    let mut col_right: Vec<f32> = Vec::new();
    let gap = median_h * 2.0; // 水平间隔超过 2 倍行高才视为另一栏

    for line in sorted {
        let line_right = line.x + line.width;
        let mut placed = false;
        for (i, right) in col_right.iter_mut().enumerate() {
            // 与已有栏的水平区间重叠（或靠近）→ 归入该栏。
            if line.x <= *right + gap {
                cols[i].push(line.clone());
                *right = right.max(line_right);
                placed = true;
                break;
            }
        }
        if !placed {
            cols.push(vec![line.clone()]);
            col_right.push(line_right);
        }
    }

    // 每栏内按 y 排序。
    for col in cols.iter_mut() {
        col.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
    }
    // 栏按 x 排序。
    cols.sort_by(|a, b| {
        let ax = a.iter().map(|l| l.x).fold(f32::MAX, f32::min);
        let bx = b.iter().map(|l| l.x).fold(f32::MAX, f32::min);
        ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
    });
    cols
}

fn merge_column(col: Vec<OcrLineBox>, median_h: f32, out: &mut Vec<Paragraph>) {
    let col_width = col.iter().map(|l| l.x + l.width).fold(0.0f32, f32::max)
        - col.iter().map(|l| l.x).fold(f32::MAX, f32::min);

    let mut group: Vec<OcrLineBox> = Vec::new();
    for line in col {
        let starts_item = is_list_item(&line.text);
        let skip = symbol_ratio(&line.text) > 0.3;

        let should_split = if let Some(prev) = group.last() {
            // 行高相差超过 1.3 倍 → 分块。
            let height_ratio = line.height.max(1.0) / prev.height.max(1.0);
            let big_height_jump = height_ratio > 1.3 || height_ratio < 1.0 / 1.3;
            // 垂直间距：下一行顶部到上一行底部。
            let gap = line.y - (prev.y + prev.height);
            let large_gap = gap > median_h * 1.0;
            // 左边缘是否靠近（相对行高）。
            let left_drift = (line.x - prev.x).abs() > median_h * 1.5;
            // 上一行没有句末标点且写满宽度 → 倾向与下一行合并（同一段换行）。
            let continues = !ends_sentence(&prev.text) && prev.x + prev.width >= col_width * 0.85;
            big_height_jump || starts_item || (large_gap && left_drift && !continues)
        } else {
            starts_item
        };

        if should_split {
            flush_group(&mut group, col_width, out);
        }
        group.push(line);
    }
    flush_group(&mut group, col_width, out);
}

fn flush_group(group: &mut Vec<OcrLineBox>, _col_width: f32, out: &mut Vec<Paragraph>) {
    if group.is_empty() {
        return;
    }
    let skip = group.iter().any(|l| symbol_ratio(&l.text) > 0.3);
    let text = group
        .iter()
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let x = group.iter().map(|l| l.x).fold(f32::MAX, f32::min);
    let y = group.iter().map(|l| l.y).fold(f32::MAX, f32::min);
    let right = group.iter().map(|l| l.x + l.width).fold(0.0f32, f32::max);
    let bottom = group.iter().map(|l| l.y + l.height).fold(0.0f32, f32::max);
    out.push(Paragraph {
        text,
        x,
        y,
        width: right - x,
        height: bottom - y,
        skip,
    });
    group.clear();
}

/// Whether a line looks like a list item (bullet / number), kept as its own block.
fn is_list_item(text: &str) -> bool {
    let t = text.trim_start();
    let first = t.chars().next().unwrap_or('\0');
    if matches!(first, '•' | '·' | '‣' | '-' | '*' | '◦' | '▪' | '○' | '●') {
        return true;
    }
    // 数字/字母编号："1.", "(1)", "a)", "1)" 等。
    let mut chars = t.chars();
    if chars.next().map_or(false, |c| c.is_ascii_digit()) {
        let rest: String = chars.collect();
        if rest.starts_with('.') || rest.starts_with(')') || rest.starts_with('、') || rest.starts_with('．') {
            return true;
        }
    }
    false
}

/// Whether the text ends with a sentence/clause terminator.
fn ends_sentence(text: &str) -> bool {
    matches!(
        text.trim_end().chars().last(),
        Some('。') | Some('！') | Some('？') | Some('!') | Some('?') | Some('；') | Some(';') | Some('.') | Some('：') | Some(':')
    )
}

/// Fraction of non-word characters — high for formulae, code, table borders.
fn symbol_ratio(text: &str) -> f32 {
    let mut total = 0usize;
    let mut symbols = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        let wordy = ch.is_alphanumeric() || is_cjk(ch);
        if !wordy {
            symbols += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        symbols as f32 / total as f32
    }
}

fn is_cjk(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3040}'..='\u{30FF}').contains(&ch)
        || ('\u{AC00}'..='\u{D7AF}').contains(&ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrLineBox {
        OcrLineBox { text: text.into(), x, y, width: w, height: h }
    }

    #[test]
    fn two_adjacent_lines_merge_into_one_paragraph() {
        let lines = vec![
            line("第一行文字", 10.0, 0.0, 200.0, 20.0),
            line("第二行文字", 10.0, 22.0, 200.0, 20.0),
        ];
        let paras = merge(lines);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].text, "第一行文字\n第二行文字");
    }

    #[test]
    fn heading_with_larger_height_splits_from_body() {
        let lines = vec![
            line("标题", 10.0, 0.0, 300.0, 40.0),
            line("正文内容", 10.0, 50.0, 300.0, 20.0),
            line("正文第二行", 10.0, 72.0, 300.0, 20.0),
        ];
        let paras = merge(lines);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text, "标题");
        assert_eq!(paras[1].text, "正文内容\n正文第二行");
    }

    #[test]
    fn list_item_starts_its_own_paragraph() {
        let lines = vec![
            line("• 第一项", 10.0, 0.0, 200.0, 20.0),
            line("• 第二项", 10.0, 22.0, 200.0, 20.0),
        ];
        let paras = merge(lines);
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn symbol_heavy_block_is_marked_skip() {
        let lines = vec![line("$$\\sum_{i=1}^n i = x$$", 10.0, 0.0, 300.0, 20.0)];
        let paras = merge(lines);
        assert_eq!(paras.len(), 1);
        assert!(paras[0].skip);
    }

    #[test]
    fn normal_text_is_not_skipped() {
        let lines = vec![line("这是一段正常的中文正文", 10.0, 0.0, 300.0, 20.0)];
        let paras = merge(lines);
        assert_eq!(paras.len(), 1);
        assert!(!paras[0].skip);
    }

    #[test]
    fn two_columns_split_then_order_left_to_right() {
        // 左栏 x=0..100，右栏 x=300..400，同一 y。
        let lines = vec![
            line("右栏第二行", 300.0, 22.0, 100.0, 20.0),
            line("左栏第一行", 0.0, 0.0, 100.0, 20.0),
            line("左栏第二行", 0.0, 22.0, 100.0, 20.0),
            line("右栏第一行", 300.0, 0.0, 100.0, 20.0),
        ];
        let paras = merge(lines);
        // 左栏两行合并为一段，右栏两行合并为一段。
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text, "左栏第一行\n左栏第二行");
        assert_eq!(paras[1].text, "右栏第一行\n右栏第二行");
    }
}
