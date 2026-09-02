//! Deterministic UTF-8 text chunking around natural boundaries.

use std::fmt::{Display, Formatter};

/// Validated chunking limits for one frozen model contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkConfig {
    /// Maximum tokens in a chunk.
    pub max_tokens: usize,
    /// Target tokens copied from the preceding chunk.
    pub overlap_tokens: usize,
}

impl ChunkConfig {
    /// Validate non-zero size and the public maximum 30 percent overlap.
    pub fn validate(self) -> Result<Self, ChunkError> {
        if self.max_tokens == 0 {
            return Err(ChunkError::InvalidConfig);
        }
        if self.overlap_tokens.saturating_mul(10) > self.max_tokens.saturating_mul(3) {
            return Err(ChunkError::InvalidConfig);
        }
        Ok(self)
    }
}

/// One chunk and its offsets in the normalized UTF-8 input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextChunk {
    /// Zero-based stable ordinal.
    pub ordinal: usize,
    /// Start byte in the normalized input.
    pub start_byte: usize,
    /// Exclusive end byte in the normalized input.
    pub end_byte: usize,
    /// Exact normalized text slice.
    pub text: String,
}

/// Invalid chunk configuration or tokenizer behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkError {
    /// Size was zero or overlap exceeded 30 percent.
    InvalidConfig,
    /// The tokenizer returned zero for non-empty text or was not monotonic.
    InvalidTokenizer,
}

impl Display for ChunkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("chunk configuration is invalid"),
            Self::InvalidTokenizer => formatter.write_str("tokenizer contract is invalid"),
        }
    }
}

impl std::error::Error for ChunkError {}

/// Split normalized text using a frozen tokenizer callback.
///
/// Paragraph, sentence, whitespace, and finally UTF-8 character boundaries are
/// preferred in that order. Returned byte offsets always refer to `input`.
pub fn chunk_text<F>(
    input: &str,
    config: ChunkConfig,
    count_tokens: F,
) -> Result<Vec<TextChunk>, ChunkError>
where
    F: Fn(&str) -> usize,
{
    let config = config.validate()?;
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    if count_tokens(input) == 0 {
        return Err(ChunkError::InvalidTokenizer);
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < input.len() {
        let remaining = &input[start..];
        let end = if count_tokens(remaining) <= config.max_tokens {
            input.len()
        } else {
            start
                + find_end(
                    remaining,
                    config.max_tokens,
                    config.overlap_tokens,
                    &count_tokens,
                )?
        };
        if end <= start {
            return Err(ChunkError::InvalidTokenizer);
        }
        chunks.push(TextChunk {
            ordinal: chunks.len(),
            start_byte: start,
            end_byte: end,
            text: input[start..end].to_owned(),
        });
        if end == input.len() {
            break;
        }
        start = overlap_start(input, start, end, config.overlap_tokens, &count_tokens);
    }
    Ok(chunks)
}

fn find_end<F>(
    text: &str,
    max_tokens: usize,
    overlap_tokens: usize,
    count_tokens: &F,
) -> Result<usize, ChunkError>
where
    F: Fn(&str) -> usize,
{
    let boundaries = text
        .char_indices()
        .map(|(index, character)| (index + character.len_utf8(), character))
        .take_while(|(end, _)| count_tokens(&text[..*end]) <= max_tokens)
        .collect::<Vec<_>>();
    if boundaries.is_empty() {
        return Err(ChunkError::InvalidTokenizer);
    }
    for predicate in [
        paragraph_boundary as fn(char, &str, usize) -> bool,
        sentence_boundary,
        whitespace_boundary,
        any_boundary,
    ] {
        if let Some((end, _)) = boundaries.iter().rev().find(|(end, character)| {
            count_tokens(&text[..*end]) > overlap_tokens && predicate(*character, text, *end)
        }) {
            return Ok(*end);
        }
    }
    Err(ChunkError::InvalidTokenizer)
}

fn paragraph_boundary(_: char, text: &str, end: usize) -> bool {
    text[..end].ends_with("\n\n")
}

fn sentence_boundary(character: char, _: &str, _: usize) -> bool {
    matches!(character, '.' | '!' | '?' | '。' | '！' | '？')
}

fn whitespace_boundary(character: char, _: &str, _: usize) -> bool {
    character.is_whitespace()
}

fn any_boundary(_: char, _: &str, _: usize) -> bool {
    true
}

fn overlap_start<F>(
    input: &str,
    prior_start: usize,
    end: usize,
    overlap_tokens: usize,
    count_tokens: &F,
) -> usize
where
    F: Fn(&str) -> usize,
{
    if overlap_tokens == 0 {
        return end;
    }
    input[prior_start..end]
        .char_indices()
        .map(|(relative, _)| prior_start + relative)
        .find(|candidate| {
            *candidate > prior_start && count_tokens(&input[*candidate..end]) <= overlap_tokens
        })
        .unwrap_or(end)
}
