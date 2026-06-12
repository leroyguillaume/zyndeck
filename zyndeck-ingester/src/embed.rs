//! Embedding chunk text into vectors via a locally-served Ollama model.
//!
//! The embed step turns each chunk into a vector the question-answering can do
//! nearest-neighbour search over. Following the project's fully-local stance, the
//! vectors come from an [Ollama](https://ollama.com/) server (BGE-M3 by default;
//! see the root README's models table) reached over plain HTTP — no cloud API.

use std::future::Future;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Dimension of the embedding vectors, fixed by the model (BGE-M3 → 1024) and
/// matched by the `vector(1024)` column they are stored in. Returned vectors are
/// checked against this so a model/schema mismatch fails loudly rather than
/// silently storing the wrong shape.
pub const EMBEDDING_DIM: usize = 1024;

/// Produces embedding vectors for text inputs.
///
/// A trait so the embed step depends on the abstraction rather than on `reqwest`
/// directly. Returns `impl Future + Send` so it can be awaited from the async
/// step body; inject with generics, never `dyn`.
pub trait Embedder {
    /// Embeds a batch of inputs, returning one vector per input in the same
    /// order. An empty input yields an empty result without a network round-trip.
    fn embed(
        &self,
        inputs: Vec<String>,
    ) -> impl Future<Output = anyhow::Result<Vec<Vec<f32>>>> + Send;
}

/// An [`Embedder`] backed by a local Ollama server's `/api/embed` endpoint.
pub struct OllamaEmbedder {
    client: reqwest::Client,
    /// Base URL of the Ollama server, e.g. `http://localhost:11434`.
    base_url: String,
    /// Model tag to embed with, e.g. `bge-m3:567m`.
    model: String,
}

impl OllamaEmbedder {
    /// Builds an embedder targeting `base_url` and `model`.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }
}

/// Request body for Ollama's `/api/embed` (note: `input` accepts a batch).
#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

/// Response body from Ollama's `/api/embed`: one vector per input, in order.
#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Embedder for OllamaEmbedder {
    async fn embed(&self, inputs: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let endpoint = format!("{}/api/embed", self.base_url.trim_end_matches('/'));
        tracing::debug!(%endpoint, model = %self.model, count = inputs.len(), "requesting embeddings");

        let response = self
            .client
            .post(&endpoint)
            .json(&EmbedRequest {
                model: &self.model,
                input: &inputs,
            })
            .send()
            .await
            .context("sending request to the Ollama embeddings API")?
            .error_for_status()
            .context("Ollama embeddings API returned an error status")?;

        let body: EmbedResponse = response
            .json()
            .await
            .context("decoding the Ollama embeddings response")?;

        check_embeddings(inputs.len(), &body.embeddings)?;
        Ok(body.embeddings)
    }
}

/// Verifies the model returned exactly one vector per input, each of the
/// expected dimension — so a wrong model or a partial response is caught before
/// the vectors reach the database.
fn check_embeddings(expected: usize, embeddings: &[Vec<f32>]) -> anyhow::Result<()> {
    if embeddings.len() != expected {
        anyhow::bail!(
            "Ollama returned {} embeddings for {expected} inputs",
            embeddings.len(),
        );
    }
    for (index, vector) in embeddings.iter().enumerate() {
        if vector.len() != EMBEDDING_DIM {
            anyhow::bail!(
                "embedding {index} has dimension {} (expected {EMBEDDING_DIM})",
                vector.len(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_an_embed_response() {
        let json = r#"{"model":"bge-m3:567m","embeddings":[[0.1,0.2],[0.3,0.4]]}"#;
        let parsed: EmbedResponse = serde_json::from_str(json).expect("valid response");
        assert_eq!(parsed.embeddings, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn check_rejects_a_count_mismatch() {
        let embeddings = vec![vec![0.0; EMBEDDING_DIM]];
        assert!(check_embeddings(2, &embeddings).is_err());
    }

    #[test]
    fn check_rejects_a_wrong_dimension() {
        let embeddings = vec![vec![0.0; EMBEDDING_DIM - 1]];
        assert!(check_embeddings(1, &embeddings).is_err());
    }

    #[test]
    fn check_accepts_well_shaped_embeddings() {
        let embeddings = vec![vec![0.0; EMBEDDING_DIM], vec![1.0; EMBEDDING_DIM]];
        assert!(check_embeddings(2, &embeddings).is_ok());
    }
}
