//! A small multi-line text buffer for the editors (Send, ticket and filter fields). Lines are
//! `String`s; the cursor counts grapheme clusters, so an accented letter or an emoji sequence is one
//! step. The buffer never exceeds `max_bytes` (counting the newlines) and never splits a character.
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone)]
pub struct TextBuf {
    lines: Vec<String>,
    row: usize,
    col: usize,
    max_bytes: usize,
}

impl Default for TextBuf {
    fn default() -> Self {
        TextBuf::new(256)
    }
}

fn gcount(s: &str) -> usize {
    s.graphemes(true).count()
}

/// Byte offset of grapheme `g` (the end of the string when `g` is past the last one).
fn boff(s: &str, g: usize) -> usize {
    s.grapheme_indices(true)
        .nth(g)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

impl TextBuf {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            max_bytes,
        }
    }

    /// A buffer holding `text` (cut at capacity) with the cursor at the start.
    pub fn from_text(text: &str, max_bytes: usize) -> Self {
        let mut b = Self::new(max_bytes);
        b.insert_str(text);
        b.row = 0;
        b.col = 0;
        b
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn byte_len(&self) -> usize {
        self.lines.iter().map(|l| l.len()).sum::<usize>() + self.lines.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// `(row, column)`, the column counted in grapheme clusters.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.lines.len() - 1);
        self.col = self.col.min(gcount(&self.lines[self.row]));
    }

    /// Insert at the cursor. `\r\n` and `\r` become `\n`. Returns the number of bytes inserted
    /// (less than `s.len()` when the capacity ran out).
    pub fn insert_str(&mut self, s: &str) -> usize {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        let mut room = self.max_bytes.saturating_sub(self.byte_len());
        let mut take = String::new();
        let mut inserted = 0;
        for ch in normalized.chars() {
            let n = ch.len_utf8();
            if n > room {
                break;
            }
            room -= n;
            take.push(ch);
            inserted += n;
        }
        let at = boff(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(at);
        let mut pieces = take.split('\n');
        self.lines[self.row].push_str(pieces.next().unwrap_or(""));
        let mut cur = self.row;
        for p in pieces {
            cur += 1;
            self.lines.insert(cur, p.to_string());
        }
        let end_byte = self.lines[cur].len();
        self.lines[cur].push_str(&tail);
        self.row = cur;
        self.col = gcount(&self.lines[cur][..end_byte]);
        inserted
    }

    pub fn insert_char(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut tmp));
    }

    pub fn newline(&mut self) {
        self.insert_str("\n");
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let start = boff(line, self.col - 1);
            let end = boff(line, self.col);
            line.replace_range(start..end, "");
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = gcount(&self.lines[self.row]);
            self.lines[self.row].push_str(&current);
        }
        self.clamp();
    }

    pub fn delete(&mut self) {
        let n = gcount(&self.lines[self.row]);
        if self.col < n {
            let line = &mut self.lines[self.row];
            let start = boff(line, self.col);
            let end = boff(line, self.col + 1);
            line.replace_range(start..end, "");
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
        self.clamp();
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = gcount(&self.lines[self.row]);
        }
    }

    pub fn right(&mut self) {
        if self.col < gcount(&self.lines[self.row]) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.clamp();
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.clamp();
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = gcount(&self.lines[self.row]);
    }

    /// Delete the whitespace before the cursor and then the word before that; at the start of a
    /// line it joins with the previous line like Backspace.
    pub fn delete_word_back(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        let gs: Vec<&str> = self.lines[self.row].graphemes(true).collect();
        let mut i = self.col.min(gs.len());
        while i > 0 && gs[i - 1].trim().is_empty() {
            i -= 1;
        }
        while i > 0 && !gs[i - 1].trim().is_empty() {
            i -= 1;
        }
        let start = boff(&self.lines[self.row], i);
        let end = boff(&self.lines[self.row], self.col);
        self.lines[self.row].replace_range(start..end, "");
        self.col = i;
        self.clamp();
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, Rng, SeedableRng};

    fn typed(s: &str) -> TextBuf {
        let mut b = TextBuf::new(1 << 20);
        b.insert_str(s);
        b
    }

    #[test]
    fn typing_and_newlines_build_lines() {
        let mut b = TextBuf::new(100);
        b.insert_str("ab");
        b.newline();
        b.insert_str("cd");
        assert_eq!(b.text(), "ab\ncd");
        assert_eq!(b.cursor(), (1, 2));
        assert_eq!(b.byte_len(), 5);
    }

    #[test]
    fn crlf_and_bare_cr_become_newlines() {
        assert_eq!(typed("a\r\nb\rc").text(), "a\nb\nc");
    }

    #[test]
    fn backspace_removes_a_whole_grapheme_and_joins_lines() {
        let mut b = typed("ae\u{301}");
        b.backspace();
        assert_eq!(b.text(), "a");
        let mut b = typed("a\nb");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "a");
        assert_eq!(b.cursor(), (0, 1));
        let mut b = typed("👨\u{200D}👩\u{200D}👧x");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn delete_forward_joins_the_next_line() {
        let mut b = typed("a\nb");
        b.up();
        b.end();
        b.delete();
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn arrows_move_across_lines_and_clamp_the_column() {
        let mut b = typed("long line\nx");
        b.up();
        b.end();
        b.down();
        assert_eq!(b.cursor(), (1, 1));
        b.left();
        b.left();
        assert_eq!(b.cursor(), (0, 9));
        b.right();
        assert_eq!(b.cursor(), (1, 0));
    }

    #[test]
    fn capacity_is_respected_and_never_splits_a_character() {
        let mut b = TextBuf::new(4);
        assert_eq!(b.insert_str("日本"), 3);
        assert_eq!(b.text(), "日");
        assert_eq!(b.insert_str("ab"), 1);
        assert_eq!(b.byte_len(), 4);
        assert_eq!(b.insert_str("z"), 0);
    }

    #[test]
    fn delete_word_back_works_on_words_and_trailing_spaces() {
        let mut b = typed("one two  ");
        b.delete_word_back();
        assert_eq!(b.text(), "one ");
        b.delete_word_back();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn paste_in_the_middle_keeps_the_tail() {
        let mut b = typed("ad");
        b.left();
        b.insert_str("b\nc");
        assert_eq!(b.text(), "ab\ncd");
        assert_eq!(b.cursor(), (1, 1));
    }

    #[test]
    fn clear_resets_everything() {
        let mut b = typed("a\nb");
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.cursor(), (0, 0));
    }

    #[test]
    fn random_operations_keep_every_invariant() {
        let pool = [
            "a",
            "é",
            "e\u{301}",
            "日",
            "👨\u{200D}👩\u{200D}👧",
            "\n",
            "\r\n",
            " ",
            "\t",
            "xyz",
            "\u{200D}",
        ];
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..200 {
            let cap = rng.gen_range(0..200);
            let mut b = TextBuf::new(cap);
            for _ in 0..200 {
                match rng.gen_range(0..12) {
                    0..=2 => {
                        b.insert_str(pool[rng.gen_range(0..pool.len())]);
                    }
                    3 => b.newline(),
                    4 => b.backspace(),
                    5 => b.delete(),
                    6 => b.left(),
                    7 => b.right(),
                    8 => b.up(),
                    9 => b.down(),
                    10 => b.home(),
                    _ => {
                        if rng.gen_bool(0.5) {
                            b.end()
                        } else {
                            b.delete_word_back()
                        }
                    }
                }
                assert!(b.byte_len() <= cap, "{} > {cap}", b.byte_len());
                assert_eq!(b.text().len(), b.byte_len());
                let (r, c) = b.cursor();
                assert!(r < b.lines().len());
                assert!(c <= gcount(&b.lines()[r]));
            }
        }
    }
}
