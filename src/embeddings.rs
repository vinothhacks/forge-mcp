use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const DEFAULT_EMBEDDING_DIM: usize = 128;

#[derive(Debug, Clone)]
pub struct EmbeddingEngine {
    endpoint_url: Option<String>,
    model: String,
    dimensions: usize,
}

#[derive(Debug, Serialize)]
struct EmbeddingApiRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbeddingApiResponse {
    data: Vec<EmbeddingApiItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingApiItem {
    embedding: Vec<f32>,
}

impl Default for EmbeddingEngine {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl EmbeddingEngine {
    pub fn new(endpoint_url: Option<String>, model: Option<String>) -> Self {
        let model_name = model.unwrap_or_else(|| {
            if endpoint_url.is_some() {
                "text-embedding-3-small".to_string()
            } else {
                "forge-local-hash-v1".to_string()
            }
        });
        Self {
            endpoint_url,
            model: model_name,
            dimensions: DEFAULT_EMBEDDING_DIM,
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Generate an embedding vector for `text`. If an HTTP endpoint is configured, attempts to query
    /// `/v1/embeddings` and falls back to the deterministic local feature-hashing embedder if offline.
    pub fn embed_text(&self, text: &str) -> Vec<f32> {
        if let Some(ref url) = self.endpoint_url {
            if let Ok(vec) = self.embed_via_http(url, text) {
                if !vec.is_empty() {
                    return normalize_l2(vec);
                }
            }
        }
        embed_local_deterministic(text, self.dimensions)
    }

    fn embed_via_http(&self, base_url: &str, text: &str) -> anyhow::Result<Vec<f32>> {
        let endpoint = if base_url.ends_with("/embeddings") {
            base_url.to_string()
        } else {
            format!("{}/v1/embeddings", base_url.trim_end_matches('/'))
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        let resp = client
            .post(&endpoint)
            .json(&EmbeddingApiRequest {
                model: &self.model,
                input: text,
            })
            .send()?;
        if !resp.status().is_success() {
            anyhow::bail!("Embedding HTTP endpoint returned status {}", resp.status());
        }
        let parsed: EmbeddingApiResponse = resp.json()?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|item| item.embedding)
            .ok_or_else(|| anyhow::anyhow!("Empty embedding data from endpoint"))
    }
}

/// Deterministic local feature-hashing embedder combining identifier sub-tokens,
/// camelCase/snake_case stems, token bigrams, and character trigrams.
pub fn embed_local_deterministic(text: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dim];
    if text.is_empty() || dim == 0 {
        return vec;
    }

    let tokens = split_code_tokens(text);
    for (i, tok) in tokens.iter().enumerate() {
        // Primary token weight (higher weight for identifier stems)
        accumulate_feature(&mut vec, &format!("tok:{}", tok), 2.0);

        // Positional bigram
        if i + 1 < tokens.len() {
            accumulate_feature(&mut vec, &format!("bi:{}:{}", tok, tokens[i + 1]), 1.2);
        }

        // Character trigrams within token for fuzzy / morphological similarity
        let chars: Vec<char> = tok.chars().collect();
        if chars.len() >= 3 {
            for win in chars.windows(3) {
                let tri: String = win.iter().collect();
                accumulate_feature(&mut vec, &format!("tri:{}", tri), 0.5);
            }
        }
    }

    normalize_l2(vec)
}

fn accumulate_feature(vec: &mut [f32], feature: &str, weight: f32) {
    let mut hasher = Sha256::new();
    hasher.update(feature.as_bytes());
    let digest = hasher.finalize();
    let dim = vec.len();

    let idx1 = (u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize) % dim;
    let sign1 = if digest[4] & 1 == 0 { 1.0 } else { -1.0 };
    vec[idx1] += sign1 * weight;

    let idx2 = (u32::from_le_bytes([digest[5], digest[6], digest[7], digest[8]]) as usize) % dim;
    let sign2 = if digest[9] & 1 == 0 { 1.0 } else { -1.0 };
    vec[idx2] += sign2 * (weight * 0.5);
}

pub fn split_code_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            // Split camelCase boundary: e.g. `verifyToken` -> `verify`, `token`
            if ch.is_ascii_uppercase()
                && !current.is_empty()
                && current
                    .chars()
                    .last()
                    .map(|c| c.is_ascii_lowercase())
                    .unwrap_or(false)
            {
                let tok = current.to_ascii_lowercase();
                if tok.len() >= 2 && !is_stopword(&tok) {
                    out.push(tok);
                }
                current.clear();
            }
            current.push(ch);
        } else if !current.is_empty() {
            let tok = current.to_ascii_lowercase();
            if tok.len() >= 2 && !is_stopword(&tok) {
                out.push(tok);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        let tok = current.to_ascii_lowercase();
        if tok.len() >= 2 && !is_stopword(&tok) {
            out.push(tok);
        }
    }
    out
}

fn is_stopword(s: &str) -> bool {
    matches!(
        s,
        "the"
            | "and"
            | "for"
            | "with"
            | "that"
            | "this"
            | "from"
            | "where"
            | "what"
            | "how"
            | "does"
            | "are"
            | "let"
            | "mut"
            | "pub"
            | "const"
            | "var"
            | "def"
            | "func"
            | "return"
    )
}

pub fn normalize_l2(mut vec: Vec<f32>) -> Vec<f32> {
    let norm_sq: f32 = vec.iter().map(|v| v * v).sum();
    if norm_sq > 1e-12 {
        let inv = 1.0 / norm_sq.sqrt();
        for v in &mut vec {
            *v *= inv;
        }
    }
    vec
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    (dot as f64).clamp(-1.0, 1.0)
}
