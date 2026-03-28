use async_trait::async_trait;
use brain_core::error::BrainError;
use reqwest::Client;
use serde::Deserialize;

use brain_core::ports::EmbeddingPort;

pub struct OpenAiEmbedder {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    dims: usize,
    model_id: String,
}

impl OpenAiEmbedder {
    pub fn new(base_url: String, api_key: String, model: String, dims: usize) -> Self {
        let model_id = format!("openai:{model}");
        Self {
            client: Client::new(),
            base_url,
            api_key,
            model,
            dims,
            model_id,
        }
    }
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingPort for OpenAiEmbedder {
    async fn embed(&self, text: &str) -> brain_core::error::Result<Vec<f32>> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = serde_json::json!({
            "input": text,
            "model": self.model,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| BrainError::Embedding(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "failed to read body".into());
            return Err(BrainError::Embedding(format!(
                "API returned {status}: {body}"
            )));
        }

        let parsed: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| BrainError::Embedding(e.to_string()))?;

        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| BrainError::Embedding("empty response data".into()))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_embedding_response(vec: Vec<f32>) -> serde_json::Value {
        serde_json::json!({
            "object": "list",
            "data": [{
                "object": "embedding",
                "index": 0,
                "embedding": vec,
            }],
            "model": "text-embedding-3-small",
            "usage": { "prompt_tokens": 5, "total_tokens": 5 }
        })
    }

    #[tokio::test]
    async fn test_embed_returns_vector() {
        let server = MockServer::start().await;
        let expected = vec![0.1, 0.2, 0.3];

        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(mock_embedding_response(expected.clone())),
            )
            .mount(&server)
            .await;

        let embedder = OpenAiEmbedder::new(
            server.uri(),
            "test-key".into(),
            "text-embedding-3-small".into(),
            1536,
        );

        let result = embedder.embed("hello world").await.unwrap();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_embed_sends_correct_request() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(mock_embedding_response(vec![0.0; 3])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let embedder = OpenAiEmbedder::new(
            server.uri(),
            "test-key".into(),
            "text-embedding-3-small".into(),
            1536,
        );

        let _ = embedder.embed("test input").await.unwrap();
        // wiremock will verify the expected request was received on drop
    }

    #[tokio::test]
    async fn test_embed_handles_api_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .mount(&server)
            .await;

        let embedder = OpenAiEmbedder::new(
            server.uri(),
            "test-key".into(),
            "text-embedding-3-small".into(),
            1536,
        );

        let result = embedder.embed("hello").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, BrainError::Embedding(_)),
            "expected BrainError::Embedding, got {err:?}"
        );
    }

    #[test]
    fn test_model_id_format() {
        let embedder = OpenAiEmbedder::new(
            "http://localhost".into(),
            "key".into(),
            "text-embedding-3-small".into(),
            1536,
        );
        assert_eq!(embedder.model_id(), "openai:text-embedding-3-small");
    }

    #[test]
    fn test_dimensions() {
        let embedder = OpenAiEmbedder::new(
            "http://localhost".into(),
            "key".into(),
            "text-embedding-3-small".into(),
            1536,
        );
        assert_eq!(embedder.dimensions(), 1536);

        let embedder_large = OpenAiEmbedder::new(
            "http://localhost".into(),
            "key".into(),
            "text-embedding-3-large".into(),
            3072,
        );
        assert_eq!(embedder_large.dimensions(), 3072);
    }
}
