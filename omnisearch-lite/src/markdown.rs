//! Minimal Markdown parser for the AI chat panel.
//!
//! Handles the "essential set": ATX headings (# / ## / ###), fenced code blocks
//! (```), unordered/ordered list items, paragraphs with inline formatting
//! (**bold**, *italic*, `code`, [link](url)).
//!
//! Output is a flat `Vec<MdBlock>`. There is no inline HTML handling and no
//! nested lists; the goal is to make AI responses readable, not to be a
//! CommonMark-conformant renderer.

#[derive(Debug, Clone)]
pub enum MdInline {
    Plain(String),
    Bold(String),
    Italic(String),
    Code(String),
    /// `url` is retained for future "open link" rendering; today we only draw the label.
    #[allow(dead_code)]
    Link {
        label: String,
        url: String,
    },
}

#[derive(Debug, Clone)]
pub enum MdBlock {
    Heading {
        level: u8,
        runs: Vec<MdInline>,
    },
    Paragraph {
        runs: Vec<MdInline>,
    },
    /// `lang` is retained for future syntax-aware highlighting; today code blocks
    /// are rendered as plain monospace.
    #[allow(dead_code)]
    Code {
        lang: String,
        text: String,
    },
    ListItem {
        runs: Vec<MdInline>,
        ordered: bool,
        index: u32,
    },
    /// A blank separator line (kept so the renderer can add vertical rhythm).
    Spacer,
}

/// Parse a Markdown string into a list of blocks.
pub fn parse(src: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    // Counter for the current ordered list, reset whenever we leave a list.
    let mut ordered_idx = 0u32;
    let mut in_ordered_list = false;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim_end();

        // ── Fenced code block
        let fence = fence_info(trimmed);
        if let Some(lang) = fence {
            let start = i + 1;
            let mut code_lines = Vec::new();
            i = start;
            while i < lines.len() && fence_info(lines[i].trim_start()).is_none() {
                code_lines.push(lines[i]);
                i += 1;
            }
            // Skip the closing fence (if present)
            if i < lines.len() && fence_info(lines[i].trim_start()).is_some() {
                i += 1;
            }
            blocks.push(MdBlock::Code {
                lang,
                text: code_lines.join("\n"),
            });
            in_ordered_list = false;
            continue;
        }

        // ── Blank line → spacer (collapse multiple)
        if trimmed.is_empty() {
            if !matches!(blocks.last(), Some(MdBlock::Spacer)) {
                blocks.push(MdBlock::Spacer);
            }
            in_ordered_list = false;
            i += 1;
            continue;
        }

        // ── ATX heading (# / ## / ###)
        if let Some(rest) = strip_heading(trimmed) {
            in_ordered_list = false;
            let (level, content) = rest;
            blocks.push(MdBlock::Heading {
                level,
                runs: parse_inline(content),
            });
            i += 1;
            continue;
        }

        // ── List item: "- ", "* ", "+ ", or "N. "
        if let Some((content, ordered)) = strip_list_item(trimmed) {
            if ordered {
                if !in_ordered_list {
                    ordered_idx = 0;
                }
                ordered_idx = ordered_idx.saturating_add(1);
                in_ordered_list = true;
            } else {
                in_ordered_list = false;
            }
            blocks.push(MdBlock::ListItem {
                runs: parse_inline(content),
                ordered,
                index: if ordered { ordered_idx } else { 0 },
            });
            i += 1;
            continue;
        }
        // Anything that wasn't a list item ends an ordered run.
        in_ordered_list = false;

        // ── Paragraph: gather consecutive non-empty, non-special lines
        let mut para = String::from(trimmed);
        i += 1;
        while i < lines.len() {
            let l = lines[i].trim_end();
            if l.is_empty()
                || fence_info(l).is_some()
                || strip_heading(l).is_some()
                || strip_list_item(l).is_some()
            {
                break;
            }
            para.push('\n');
            para.push_str(l);
            i += 1;
        }
        // Soft-wrap newlines inside a paragraph become spaces for flow text.
        let runs = parse_inline(&para);
        blocks.push(MdBlock::Paragraph { runs });
    }

    blocks
}

/// Returns the language string (possibly empty) if `line` is an opening code
/// fence of at least three backticks. We do not support the `~~~` variant to
/// keep this simple — backtick fences cover essentially all AI output.
fn fence_info(line: &str) -> Option<String> {
    let t = line.trim_start();
    if t.starts_with("```") {
        Some(t[3..].trim().to_string())
    } else if t == "```" {
        Some(String::new())
    } else {
        None
    }
}

/// Returns `(level, content)` if the line is an ATX heading (level 1..=6).
fn strip_heading(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    let mut level = 0u8;
    for c in t.chars() {
        if c == '#' && level < 6 {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 {
        return None;
    }
    let after = &t[level as usize..];
    if after.is_empty() {
        return Some((level, ""));
    }
    // A heading requires whitespace after the #'s (e.g. "#x" is not a heading).
    if !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    Some((level, after.trim()))
}

/// Returns `(content, ordered)` if the line is a list item, else `None`.
fn strip_list_item(line: &str) -> Option<(&str, bool)> {
    let t = line.trim_start();
    let indent = line.len() - t.len();

    // Unordered: "- ", "* ", "+ "
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            // Avoid mistaking a horizontal rule ("---") for a list.
            return Some((rest.trim_start(), false));
        }
    }
    let _ = indent;

    // Ordered: "N. " (N is one or more digits).
    let bytes = t.as_bytes();
    let mut j = 0;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j > 0
        && j + 1 < bytes.len()
        && (bytes[j] == b'.' || bytes[j] == b')')
        && bytes[j + 1] == b' '
    {
        return Some((t[j + 2..].trim_start(), true));
    }
    None
}

/// Tokenize a single line (or paragraph) into inline runs.
///
/// Recognizes, in priority order: `` `code` ``, `[label](url)`, `**bold**`,
/// `*italic*` / `_italic_`. Anything else is plain text. The tokenizer is
/// greedy-left-to-right and falls back to a literal char when a marker isn't
/// matched, so unmatched markers render verbatim (no data loss).
pub fn parse_inline(s: &str) -> Vec<MdInline> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut runs = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    fn flush(plain: &mut String, runs: &mut Vec<MdInline>) {
        if !plain.is_empty() {
            runs.push(MdInline::Plain(std::mem::take(plain)));
        }
    }

    while i < n {
        let rest = &s[i..];

        // Inline code: `...`
        if rest.starts_with('`') {
            if let Some(close_rel) = find_unescaped(&rest[1..], '`') {
                let inner = &rest[1..1 + close_rel];
                flush(&mut plain, &mut runs);
                runs.push(MdInline::Code(inner.to_string()));
                i += 1 + close_rel + 1;
                continue;
            }
        }

        // Link: [label](url)
        if rest.starts_with('[') {
            if let Some((label, url, consumed)) = parse_link(rest) {
                flush(&mut plain, &mut runs);
                runs.push(MdInline::Link { label, url });
                i += consumed;
                continue;
            }
        }

        // Bold: **...** (check before single-* italic)
        if rest.starts_with("**") {
            if let Some(close_rel) = find_marker(&rest[2..], "**") {
                let inner = &rest[2..2 + close_rel];
                if !inner.is_empty() {
                    flush(&mut plain, &mut runs);
                    runs.push(MdInline::Bold(inner.to_string()));
                    i += 2 + close_rel + 2;
                    continue;
                }
            }
        }

        // Italic: *...*  or  _..._
        let (open_ch, close_ch) = if rest.starts_with('*') {
            ('*', '*')
        } else if rest.starts_with('_') {
            ('_', '_')
        } else {
            ('\0', '\0')
        };
        if open_ch != '\0' {
            if let Some(close_rel) = find_marker(&rest[1..], &close_ch.to_string()) {
                let inner = &rest[1..1 + close_rel];
                // Require a non-space char right after the opening marker and
                // right before the closing marker. This rejects stray markers
                // like "a * b" (opening * is followed by a space) and a dangling
                // "stray *" so they render verbatim instead of as italics.
                let after_open_ok = inner.chars().next().map_or(false, |c| c != ' ');
                let before_close_ok = inner.chars().last().map_or(false, |c| c != ' ');
                if !inner.is_empty() && after_open_ok && before_close_ok {
                    flush(&mut plain, &mut runs);
                    runs.push(MdInline::Italic(inner.to_string()));
                    i += 1 + close_rel + 1;
                    continue;
                }
            }
        }

        // Default: accumulate one char into the plain run.
        let ch = rest.chars().next().unwrap();
        plain.push(ch);
        i += ch.len_utf8();
    }

    flush(&mut plain, &mut runs);
    if runs.is_empty() {
        runs.push(MdInline::Plain(String::new()));
    }
    runs
}

/// Find the next unescaped occurrence of `marker`, returning its byte offset
/// from the start of `s`. Returns `None` if not found.
fn find_marker(s: &str, marker: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(pos) = s[from..].find(marker) {
        let abs = from + pos;
        // Count preceding backslashes; odd count means it's escaped.
        let mut bs = 0;
        let mut k = abs;
        while k > 0 && s.as_bytes()[k - 1] == b'\\' {
            bs += 1;
            k -= 1;
        }
        if bs % 2 == 0 {
            return Some(abs);
        }
        from = abs + marker.len();
    }
    None
}

/// Find the next unescaped occurrence of a single-byte `ch`.
fn find_unescaped(s: &str, ch: char) -> Option<usize> {
    find_marker(s, &ch.to_string())
}

/// Try to parse a `[label](url)` starting at the beginning of `s`.
/// Returns `(label, url, bytes_consumed)` on success.
fn parse_link(s: &str) -> Option<(String, String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    // Find closing ']'.
    let mut depth = 1;
    let mut j = 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2, // skip escaped char
            b'[' => {
                depth += 1;
                j += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
    if depth != 0 || j >= bytes.len() {
        return None;
    }
    let label = s[1..j].to_string();
    // Next must be "(...)".
    if bytes.get(j + 1) != Some(&b'(') {
        return None;
    }
    let url_start = j + 2;
    let mut k = url_start;
    while k < bytes.len() && bytes[k] != b')' {
        if bytes[k] == b'\\' {
            k += 1;
        }
        k += 1;
    }
    if k >= bytes.len() {
        return None;
    }
    let url = s[url_start..k].trim().to_string();
    Some((label, url, k + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_and_paragraph() {
        let blocks = parse("# Title\n\nSome **bold** text.");
        assert!(matches!(blocks[0], MdBlock::Heading { level: 1, .. }));
        // A blank line separates the heading from the paragraph (via a Spacer).
        let para = blocks.iter().find_map(|b| match b {
            MdBlock::Paragraph { runs } => Some(runs),
            _ => None,
        });
        let runs = para.expect("expected a paragraph");
        assert!(runs.iter().any(|r| matches!(r, MdInline::Bold(_))));
    }

    #[test]
    fn parses_code_block() {
        let src = "```rust\nfn main() {}\n```\n";
        let blocks = parse(src);
        assert_eq!(blocks.len(), 1);
        if let MdBlock::Code { lang, text } = &blocks[0] {
            assert_eq!(lang, "rust");
            assert!(text.contains("fn main"));
        } else {
            panic!("expected code block");
        }
    }

    #[test]
    fn parses_lists() {
        let blocks = parse("- a\n- b\n1. c\n2. d");
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| {
                if let MdBlock::ListItem { ordered, index, .. } = b {
                    Some((*ordered, *index))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(items, vec![(false, 0), (false, 0), (true, 1), (true, 2)]);
    }

    #[test]
    fn inline_link_and_code() {
        let runs = parse_inline("see [docs](https://x.io) and `code`");
        assert!(runs
            .iter()
            .any(|r| matches!(r, MdInline::Link { url, .. } if url == "https://x.io")));
        assert!(runs.iter().any(|r| matches!(r, MdInline::Code(_))));
    }

    #[test]
    fn unmatched_markers_render_verbatim() {
        let runs = parse_inline("a * b with stray *");
        let joined: String = runs
            .iter()
            .map(|r| match r {
                MdInline::Plain(s) => s.as_str(),
                MdInline::Bold(s) | MdInline::Italic(s) | MdInline::Code(s) => s.as_str(),
                MdInline::Link { label, .. } => label.as_str(),
            })
            .collect();
        assert!(joined.contains("stray *"));
    }
}
