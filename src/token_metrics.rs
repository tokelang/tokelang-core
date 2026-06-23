//! cl100k_base token counting via the in-process, pure-Rust `tiktoken-rs` crate.
//!
//! Previously this module spawned a persistent `python3 -c` worker that imported
//! `tiktoken`. That made accurate counting depend on a Python interpreter *and* the
//! `tiktoken` package being installed at runtime; when either was missing the engine
//! silently fell back to the heuristic proxy tokenizer, so routing decisions (engine.rs
//! feeds `count` into `classify`) were computed against the wrong token numbers — which
//! is exactly how the live DO image quietly ran the proxy. `tiktoken-rs` embeds the
//! cl100k BPE ranks and runs in-process, so the engine — and the static `tokelang-cli`
//! binary that bundles it — always counts real cl100k with no runtime dependency.
//!
//! Counts are byte-identical to Python
//! `tiktoken.get_encoding("cl100k_base").encode(text)` for special-token-free text
//! (`encode_ordinary` here mirrors Python's default `encode`, which disallows special
//! tokens). Verified equal across the full dogfood corpus before this swap landed.

use std::sync::OnceLock;
use tiktoken_rs::{CoreBPE, cl100k_base};

/// Lazily-built cl100k encoder. `cl100k_base()` only assembles the BPE table from data
/// embedded in the crate, so initialization is infallible in practice; the `None` arm is
/// modeled so a hypothetical load failure degrades to the proxy instead of panicking.
fn cl100k() -> Option<&'static CoreBPE> {
    static CL100K: OnceLock<Option<CoreBPE>> = OnceLock::new();
    CL100K.get_or_init(|| cl100k_base().ok()).as_ref()
}

#[derive(Debug, Clone)]
pub enum Tokenizer {
    /// Real cl100k_base, counted in-process via `tiktoken-rs`.
    TiktokenCl100k,
    /// Heuristic offline fallback. Only selected if the embedded cl100k table fails to
    /// load (not observed in practice); kept so counting never hard-fails.
    Proxy,
}

impl Tokenizer {
    pub fn detect() -> Self {
        if cl100k().is_some() {
            Self::TiktokenCl100k
        } else {
            Self::Proxy
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::TiktokenCl100k => "cl100k_base via in-process tiktoken-rs",
            Self::Proxy => "offline proxy tokenizer",
        }
    }

    pub fn count(&self, text: &str) -> usize {
        match self {
            Self::TiktokenCl100k => cl100k()
                .map(|bpe| bpe.encode_ordinary(text).len())
                .unwrap_or_else(|| proxy_count(text)),
            Self::Proxy => proxy_count(text),
        }
    }
}

fn proxy_count(text: &str) -> usize {
    let mut count = 0usize;
    let mut in_word = false;

    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' || character == '-' {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if !character.is_whitespace() {
                count += 1;
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::Tokenizer;

    #[test]
    fn detect_selects_tiktoken() {
        // The cl100k table is embedded, so detection must always resolve to the real
        // tokenizer — never the proxy — regardless of the host environment.
        assert!(matches!(Tokenizer::detect(), Tokenizer::TiktokenCl100k));
    }

    #[test]
    fn cl100k_counts_match_python_tiktoken() {
        let tok = Tokenizer::TiktokenCl100k;
        // Reference values from Python
        // `tiktoken.get_encoding("cl100k_base").encode(text)` len.
        assert_eq!(tok.count(""), 0);
        assert_eq!(tok.count("hello world"), 2);
        assert_eq!(tok.count("greater than or equal to"), 5);
        // Multibyte / non-ASCII whitespace must count without panicking (U+2007 was the
        // segmenter crash trigger; counting it here is a separate, always-safe path).
        assert_eq!(tok.count("100\u{2007}files"), 4);
    }

    #[test]
    fn proxy_is_a_pure_fallback() {
        // The proxy still produces a stable, positive count so `count` never returns 0
        // for non-empty input if the (infallible-in-practice) table ever failed to load.
        assert_eq!(Tokenizer::Proxy.count(""), 0);
        assert!(Tokenizer::Proxy.count("hello world") > 0);
    }
}
