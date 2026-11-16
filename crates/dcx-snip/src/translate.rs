//! Text cleanup and routing for the configured translation provider.
//!
//! The actual HTTP calls live in `providers`; this module owns the OCR text
//! repairs, script filtering and the direction decision. No model is bundled
//! or downloaded by the app.

use crate::providers;

/// Texts longer than this go out in several requests instead of one huge body.
const MAX_CHUNK_CHARS: usize = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    EnZh,
    ZhEn,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::EnZh => "英 → 中",
            Direction::ZhEn => "中 → 英",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Direction::EnZh => Direction::ZhEn,
            Direction::ZhEn => Direction::EnZh,
        }
    }
}

/// Translate OCR text through the configured provider.
pub fn translate(text: &str, dir: Direction) -> Result<String, String> {
    let text = clean_ocr(text);
    let text = select_script(&text, dir);
    if !has_translatable(&text) {
        return Err("没有与当前翻译方向匹配的文字".to_string());
    }
    let out = if text.chars().count() <= MAX_CHUNK_CHARS {
        providers::translate(&text, dir)?
    } else {
        let mut parts = Vec::new();
        for chunk in chunk_text(&text) {
            parts.push(providers::translate(&chunk, dir)?);
        }
        parts.join("\n")
    };
    if has_degenerate_run(&out) {
        return Err("翻译结果异常，请切换方向或缩短选区后重试".to_string());
    }
    Ok(out.trim().to_string())
}

/// OCR puts a space between every Chinese glyph; remove only the spaces that
/// sit between CJK characters or CJK punctuation, keeping English word gaps.
pub fn clean_ocr(text: &str) -> String {
    let mut out_lines = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut cleaned = String::new();
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if ch == ' ' || ch == '\t' {
                let prev = cleaned.chars().last();
                let next = chars[i + 1..]
                    .iter()
                    .find(|c| **c != ' ' && **c != '\t')
                    .copied();
                if prev.is_some_and(is_cjk) && next.is_some_and(is_cjk) {
                    i += 1;
                    continue;
                }
                if !cleaned.ends_with(' ') {
                    cleaned.push(' ');
                }
            } else if ch != '\r' {
                cleaned.push(ch);
            }
            i += 1;
        }
        let mut cleaned = cleaned.trim_end().to_string();
        if cleaned.chars().any(|c| c.is_ascii_alphabetic()) {
            cleaned = normalize_latin_tokens(&cleaned);
        }
        out_lines.push(cleaned);
    }
    while out_lines.len() >= 3 && out_lines.iter().rev().take(3).all(|l| l.trim().is_empty()) {
        out_lines.pop();
    }
    out_lines.join("\n").trim().to_string()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0x3000..=0x303F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F)
}

/// Repair the OCR habit of splitting a word into single-letter tokens
/// ("c a n", "0 r", "0 C R"). Only runs of one-character tokens are joined, so
/// normal words and CJK are untouched.
fn normalize_latin_tokens(line: &str) -> String {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if is_letter_token(tokens[i]) {
            let start = i;
            while i < tokens.len() && is_letter_token(tokens[i]) {
                i += 1;
            }
            let run = &tokens[start..i];
            let has_zero = run.iter().any(|t| *t == "0");
            if run.len() >= 3 || (run.len() >= 2 && has_zero) {
                let upper = run
                    .iter()
                    .any(|t| t.chars().all(|c| c.is_ascii_uppercase()));
                let mut word = String::new();
                for t in run {
                    if *t == "0" {
                        word.push(if upper { 'O' } else { 'o' });
                    } else {
                        word.push_str(t);
                    }
                }
                out.push(word);
            } else {
                out.extend(run.iter().map(|t| t.to_string()));
            }
        } else {
            out.push(tokens[i].to_string());
            i += 1;
        }
    }
    out.join(" ")
}

fn is_letter_token(token: &str) -> bool {
    token == "0"
        || (token.len() == 1
            && token
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic()))
}

pub fn has_translatable(text: &str) -> bool {
    text.chars().any(|c| is_cjk(c) || c.is_ascii_alphabetic())
}

pub fn detect_direction(text: &str) -> Direction {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else if c.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cjk > 0 && cjk * 2 >= latin {
        Direction::ZhEn
    } else {
        Direction::EnZh
    }
}

/// Keep only lines that belong to the source script. OCR of a mixed UI
/// otherwise feeds Chinese chrome into an en→zh request (and vice versa),
/// which makes any translator drop or hallucinate whole lines.
fn select_script(text: &str, dir: Direction) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            let mut cjk = 0usize;
            let mut latin = 0usize;
            for c in line.chars() {
                if is_cjk(c) {
                    cjk += 1;
                } else if c.is_ascii_alphabetic() {
                    latin += 1;
                }
            }
            match dir {
                Direction::EnZh => latin >= 2 && latin >= cjk,
                Direction::ZhEn => cjk >= 1 && cjk * 2 >= latin,
            }
        })
        .collect();
    lines.join("\n").trim().to_string()
}

fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if para.chars().count() <= MAX_CHUNK_CHARS {
            chunks.push(para.to_string());
            continue;
        }
        let mut current = String::new();
        for sentence in split_sentences(para) {
            if sentence.chars().count() > MAX_CHUNK_CHARS {
                if !current.is_empty() {
                    chunks.push(current.trim().to_string());
                    current.clear();
                }
                let mut buf = String::new();
                for ch in sentence.chars() {
                    if buf.chars().count() >= MAX_CHUNK_CHARS {
                        chunks.push(buf.trim().to_string());
                        buf.clear();
                    }
                    buf.push(ch);
                }
                if !buf.trim().is_empty() {
                    chunks.push(buf.trim().to_string());
                }
                continue;
            }
            if current.chars().count() + sentence.chars().count() > MAX_CHUNK_CHARS {
                if !current.trim().is_empty() {
                    chunks.push(current.trim().to_string());
                }
                current.clear();
            }
            current.push_str(&sentence);
        }
        if !current.trim().is_empty() {
            chunks.push(current.trim().to_string());
        }
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n') {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

fn has_degenerate_run(text: &str) -> bool {
    let mut last = '\0';
    let mut run = 0usize;
    for c in text.chars() {
        if c == last && !c.is_whitespace() {
            run += 1;
        } else {
            last = c;
            run = 1;
        }
        if run >= 24 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_spaces_are_removed_but_english_gaps_stay() {
        assert_eq!(clean_ocr("长 截 图 560 px 撤 销"), "长截图 560 px 撤销");
        assert_eq!(clean_ocr("Hello   world"), "Hello world");
        assert_eq!(clean_ocr("你 好 ， 世 界"), "你好，世界");
    }

    #[test]
    fn broken_single_letter_words_are_rejoined() {
        assert_eq!(
            clean_ocr("The result c a n be copied 0 r translated."),
            "The result can be copied or translated."
        );
        assert_eq!(clean_ocr("0 C R"), "OCR");
        assert_eq!(clean_ocr("a n d"), "and");
    }

    #[test]
    fn direction_follows_script() {
        assert_eq!(detect_direction("Hello world"), Direction::EnZh);
        assert_eq!(detect_direction("你好，世界"), Direction::ZhEn);
        assert_eq!(detect_direction("Hello 世界"), Direction::EnZh);
    }

    #[test]
    fn translation_keeps_only_the_source_script() {
        let mixed = "DCX offline OCR\n这是中文标题\nThe result can be copied.";
        assert_eq!(
            select_script(mixed, Direction::EnZh),
            "DCX offline OCR\nThe result can be copied."
        );
        assert_eq!(select_script(mixed, Direction::ZhEn), "这是中文标题");
    }

    #[test]
    fn long_text_is_split_below_the_budget() {
        let text = "句子。".repeat(2000);
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK_CHARS));
    }
}
