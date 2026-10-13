//! Text that comes from outside (clips, device names, log lines, file contents) is untrusted: it must
//! never act on the terminal and never reorder or hide what the person reads. Everything shown on
//! screen goes through [`clean`] or [`one_line`] first.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn marker(c: char) -> String {
    format!("⟦U+{:04X}⟧", c as u32)
}

/// Characters that are shown as a visible marker instead of being passed on.
fn is_marked(c: char) -> bool {
    matches!(c as u32,
        0x0080..=0x009F            // C1 controls: CSI, OSC, ... in their 8-bit form
        | 0x061C | 0x200B | 0x200E | 0x200F | 0x2060 | 0xFEFF
        | 0x202A..=0x202E          // bidirectional embeddings and overrides
        | 0x2066..=0x2069          // bidirectional isolates
        | 0xE0000..=0xE007F        // tag characters (invisible "hidden text")
    )
}

/// Make `input` safe to draw. Newlines are kept; tabs become four spaces; carriage returns, C0 controls
/// and DEL become their visible "control picture" (`␛`, `␇`, `␡`, ...); C1 controls, bidi controls,
/// zero-width spaces, BOM and tag characters become `⟦U+XXXX⟧`. Zero-width joiners stay (emoji need them).
pub fn clean(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\r' => out.push('␍'),
            '\u{7F}' => out.push('␡'),
            c if (c as u32) < 0x20 => out.push(char::from_u32(0x2400 + c as u32).unwrap_or('�')),
            c if is_marked(c) => out.push_str(&marker(c)),
            c => out.push(c),
        }
    }
    out
}

/// [`clean`], then every newline shown as `⏎` so the result is a single line.
pub fn one_line(input: &str) -> String {
    clean(input).replace('\n', "⏎")
}

/// Display width in terminal columns.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut `s` to at most `max_cols` columns, ending with `…` when something was dropped. Works on
/// grapheme clusters, so a letter and its combining accent, or an emoji sequence, are never split.
pub fn truncate(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if width(s) <= max_cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = width(g);
        if used + w + 1 > max_cols {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

/// Exactly `cols` columns: truncated or padded with spaces.
pub fn pad_to(s: &str, cols: usize) -> String {
    let t = truncate(s, cols);
    let pad = cols.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// The last `max_cols` columns of `s`, whole grapheme clusters only.
pub fn tail(s: &str, max_cols: usize) -> String {
    let mut cols = 0;
    let mut parts: Vec<&str> = Vec::new();
    for g in s.graphemes(true).rev() {
        let w = width(g);
        if cols + w > max_cols {
            break;
        }
        cols += w;
        parts.push(g);
    }
    parts.reverse();
    parts.concat()
}

/// Hard-wrap `s` (already cleaned) into rows of at most `cols` columns; a newline starts a new row.
pub fn wrap_rows(s: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut rows = Vec::new();
    for line in s.split('\n') {
        let mut cur = String::new();
        let mut used = 0;
        for g in line.graphemes(true) {
            let w = width(g);
            if used + w > cols && used > 0 {
                rows.push(std::mem::take(&mut cur));
                used = 0;
            }
            cur.push_str(g);
            used += w;
        }
        rows.push(cur);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_control(s: &str) -> bool {
        s.chars().all(|c| c == '\n' || !c.is_control())
    }

    #[test]
    fn escape_sequences_never_survive() {
        let hostile = "ok \x1b[31mred\x1b[0m \x1b]0;pwned\x07 \u{9b}31m \x08\x7f end";
        let out = clean(hostile);
        assert!(no_control(&out), "{out:?}");
        for needle in ['␛', '␇', '␡'] {
            assert!(out.contains(needle), "{needle} missing in {out:?}");
        }
        assert!(out.contains("⟦U+009B⟧"), "{out:?}");
        assert!(out.starts_with("ok "));
    }

    #[test]
    fn bidi_tag_and_invisible_characters_are_made_visible() {
        let out = clean("abc\u{202E}def\u{2066}x\u{200B}y\u{FEFF}z\u{E0041}");
        for needle in ["⟦U+202E⟧", "⟦U+2066⟧", "⟦U+200B⟧", "⟦U+FEFF⟧", "⟦U+E0041⟧"]
        {
            assert!(out.contains(needle), "{needle} missing in {out:?}");
        }
    }

    #[test]
    fn emoji_joiners_and_ordinary_text_are_left_alone() {
        let family = "👨\u{200D}👩\u{200D}👧";
        assert_eq!(clean(family), family);
        assert_eq!(clean("日本語 עברית e\u{301}"), "日本語 עברית e\u{301}");
    }

    #[test]
    fn tabs_returns_and_newlines() {
        assert_eq!(clean("a\tb"), "a    b");
        assert_eq!(clean("a\r\nb"), "a␍\nb");
        assert_eq!(one_line("a\nb"), "a⏎b");
    }

    #[test]
    fn truncate_respects_display_width_and_never_splits_a_cluster() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        let t = truncate("日本語テキスト", 7);
        assert!(width(&t) <= 7 && t.ends_with('…'), "{t}");
        let t = truncate("e\u{301}e\u{301}e\u{301}", 2);
        assert!(!t.starts_with('\u{301}') && width(&t) <= 2, "{t:?}");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("日本", 1), "…");
    }

    #[test]
    fn pad_to_gives_exact_width() {
        assert_eq!(width(&pad_to("日本", 6)), 6);
        assert_eq!(width(&pad_to("abcdefgh", 4)), 4);
    }

    #[test]
    fn random_input_is_always_safe_and_fits() {
        use rand::{rngs::StdRng, Rng, SeedableRng};
        let pool: Vec<char> = "ab 日本\n\t\r\x1b\x07\u{9b}\u{202e}\u{200b}\u{200d}é\u{301}👨"
            .chars()
            .collect();
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..5_000 {
            let n = rng.gen_range(0..60);
            let s: String = (0..n).map(|_| pool[rng.gen_range(0..pool.len())]).collect();
            assert!(no_control(&clean(&s)));
            let max = rng.gen_range(0..30);
            assert!(width(&truncate(&one_line(&s), max)) <= max);
        }
    }

    #[test]
    fn tail_keeps_the_last_columns_whole_clusters_only() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 5), "ab");
        assert_eq!(tail("日本語", 5), "本語");
        assert_eq!(tail("ae\u{301}", 1), "e\u{301}");
        assert_eq!(tail("abc", 0), "");
    }

    #[test]
    fn wrap_rows_breaks_long_lines_and_keeps_newlines() {
        assert_eq!(wrap_rows("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap_rows("ab\ncd", 4), ["ab", "cd"]);
        assert_eq!(wrap_rows("", 4), [""]);
        assert_eq!(wrap_rows("日本語", 4), ["日本", "語"]);
        for row in wrap_rows(&"x".repeat(1000), 37) {
            assert!(width(&row) <= 37);
        }
    }
}
