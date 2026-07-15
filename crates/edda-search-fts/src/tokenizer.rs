//! CJK-aware bigram tokenizer (GH-402).
//!
//! Tantivy's default tokenizer emits a contiguous CJK run as a **single**
//! token, so any query that only appears inside a longer run silently returns
//! nothing — fatal for a majority-Chinese corpus. This tokenizer instead emits
//! overlapping character **bigrams** for CJK runs (`把機器` → `把機`, `機器`)
//! while tokenizing ASCII/Latin runs as lowercased words and dropping
//! punctuation/whitespace.
//!
//! Registered on the index (see `schema::register_tokenizers`), it is applied
//! symmetrically at index time and — because `QueryParser::for_index` reuses
//! the field's tokenizer — at query time. So `權威事實` tokenizes to
//! `[權威, 威事, 事實]`, every one of which is present in a document containing
//! `…洗成權威事實`, making the phrase reachable.

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// Name under which this tokenizer is registered on the index.
pub const CJK_TOKENIZER: &str = "cjk";

#[derive(Clone, Default)]
pub struct CjkBigramTokenizer;

/// A token stream backed by a pre-computed token vector.
pub struct PrecomputedTokenStream {
    tokens: Vec<Token>,
    cursor: usize,
}

impl TokenStream for PrecomputedTokenStream {
    fn advance(&mut self) -> bool {
        if self.cursor >= self.tokens.len() {
            return false;
        }
        self.cursor += 1;
        true
    }

    fn token(&self) -> &Token {
        &self.tokens[self.cursor - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.cursor - 1]
    }
}

impl Tokenizer for CjkBigramTokenizer {
    type TokenStream<'a> = PrecomputedTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> PrecomputedTokenStream {
        PrecomputedTokenStream {
            tokens: tokenize(text),
            cursor: 0,
        }
    }
}

/// Characters treated as CJK (ideographs, kana, hangul). Deliberately excludes
/// CJK symbols/punctuation (U+3000–303F) so those act as run separators.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2EFF        // CJK Radicals Supplement
        | 0x2F00..=0x2FDF      // Kangxi Radicals
        | 0x3040..=0x30FF      // Hiragana + Katakana
        | 0x3130..=0x318F      // Hangul Compatibility Jamo
        | 0x31F0..=0x31FF      // Katakana Phonetic Extensions
        | 0x3400..=0x4DBF      // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF      // CJK Unified Ideographs
        | 0xA960..=0xA97F      // Hangul Jamo Extended-A
        | 0xAC00..=0xD7AF      // Hangul Syllables
        | 0x1100..=0x11FF      // Hangul Jamo
        | 0xD7B0..=0xD7FF      // Hangul Jamo Extended-B
        | 0xF900..=0xFAFF      // CJK Compatibility Ideographs
        | 0xFF65..=0xFF9F      // Halfwidth Katakana
        | 0x2_0000..=0x2_A6DF  // CJK Unified Ideographs Extension B
        | 0x2_A700..=0x2_EE5F  // CJK Unified Ideographs Extensions C–I
        | 0x2_F800..=0x2_FA1F  // CJK Compatibility Ideographs Supplement
        | 0x3_0000..=0x3_23AF  // CJK Unified Ideographs Extensions G–H
    )
}

fn mk(offset_from: usize, offset_to: usize, position: usize, text: String) -> Token {
    Token {
        offset_from,
        offset_to,
        position,
        text,
        position_length: 1,
    }
}

fn tokenize(text: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let (byte_start, c) = chars[i];
        if is_cjk(c) {
            // Extent of the contiguous CJK run.
            let mut j = i;
            while j < chars.len() && is_cjk(chars[j].1) {
                j += 1;
            }
            if j - i == 1 {
                // A lone CJK character is emitted by itself.
                let end = byte_start + c.len_utf8();
                tokens.push(mk(byte_start, end, pos, c.to_string()));
                pos += 1;
            } else {
                // Overlapping bigrams across the run.
                for k in i..j - 1 {
                    let (bs, c0) = chars[k];
                    let c1 = chars[k + 1].1;
                    let end = chars[k + 1].0 + c1.len_utf8();
                    let mut s = String::with_capacity(c0.len_utf8() + c1.len_utf8());
                    s.push(c0);
                    s.push(c1);
                    tokens.push(mk(bs, end, pos, s));
                    pos += 1;
                }
            }
            i = j;
        } else if c.is_alphanumeric() {
            // ASCII/Latin/digit run (non-CJK alphanumerics), lowercased.
            let mut j = i;
            while j < chars.len() && chars[j].1.is_alphanumeric() && !is_cjk(chars[j].1) {
                j += 1;
            }
            let last = chars[j - 1].1;
            let end = chars[j - 1].0 + last.len_utf8();
            let word: String = chars[i..j]
                .iter()
                .flat_map(|(_, ch)| ch.to_lowercase())
                .collect();
            tokens.push(mk(byte_start, end, pos, word));
            pos += 1;
            i = j;
        } else {
            // Separator: whitespace or punctuation.
            i += 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        let mut t = CjkBigramTokenizer;
        let mut stream = t.token_stream(s);
        let mut out = Vec::new();
        while stream.advance() {
            out.push(stream.token().text.clone());
        }
        out
    }

    #[test]
    fn cjk_run_emits_overlapping_bigrams() {
        assert_eq!(toks("權威事實"), vec!["權威", "威事", "事實"]);
    }

    #[test]
    fn two_char_cjk_is_single_bigram() {
        assert_eq!(toks("收據"), vec!["收據"]);
    }

    #[test]
    fn lone_cjk_char_emitted_alone() {
        assert_eq!(toks("我"), vec!["我"]);
    }

    #[test]
    fn six_char_run_bigrams() {
        assert_eq!(
            toks("洗成權威事實"),
            vec!["洗成", "成權", "權威", "威事", "事實"]
        );
    }

    #[test]
    fn ascii_identifier_splits_and_lowercases() {
        assert_eq!(toks("ENV_LOCK"), vec!["env", "lock"]);
        assert_eq!(toks("task_nudge.rs"), vec!["task", "nudge", "rs"]);
    }

    #[test]
    fn mixed_script_keeps_both() {
        // CJK run bigrammed, ASCII word kept whole and lowercased.
        assert_eq!(toks("洗成Fact"), vec!["洗成", "fact"]);
        assert_eq!(toks("task rail 收據"), vec!["task", "rail", "收據"]);
    }

    #[test]
    fn punctuation_separates_cjk_runs() {
        // A comma between runs stops bigrams from spanning it.
        assert_eq!(toks("收據,驗收"), vec!["收據", "驗收"]);
    }

    #[test]
    fn byte_offsets_are_valid_utf8_boundaries() {
        let text = "洗成權威";
        let mut t = CjkBigramTokenizer;
        let mut stream = t.token_stream(text);
        while stream.advance() {
            let tok = stream.token();
            // Slicing at the reported offsets must not panic (valid boundaries).
            let slice = &text[tok.offset_from..tok.offset_to];
            assert_eq!(slice, tok.text);
        }
    }
}
