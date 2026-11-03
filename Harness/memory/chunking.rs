//! Deterministic UTF-8-safe source spans. Conservative character bounds avoid
//! assuming a tokenizer until a validated embedding profile supplies one.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Cut one authorized episode. Offsets are bytes in original text; never call
/// across privacy, tool-group or source-revision boundaries.
pub fn chunk_text(text: &str, max_chars: usize, overlap_chars: usize) -> Result<Vec<Chunk>> {
    ensure!(
        (64..=16_000).contains(&max_chars) && overlap_chars < max_chars / 2,
        "invalid chunk bounds"
    );
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    let length = boundaries.len() - 1;
    let mut result = Vec::new();
    let mut start = 0;
    while start < length {
        let mut end = (start + max_chars).min(length);
        if end < length {
            let floor = start + max_chars / 2;
            if let Some(i) = (floor..end)
                .rev()
                .find(|&i| &text[boundaries[i]..boundaries[i + 1]] == "\n")
            {
                end = i + 1;
            }
        }
        let chunk = &text[boundaries[start]..boundaries[end]];
        if !chunk.trim().is_empty() {
            result.push(Chunk {
                start: boundaries[start],
                end: boundaries[end],
                text: chunk.into(),
            });
        }
        if end == length {
            break;
        }
        start = end.saturating_sub(overlap_chars).max(start + 1);
    }
    Ok(result)
}
