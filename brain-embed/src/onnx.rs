use std::path::Path;
use std::sync::{Arc, Mutex};

use brain_core::error::{BrainError, Result};
use brain_core::ports::{BoxFuture, EmbeddingPort};
use ndarray::{Array2, Array3, Axis};
use ort::session::Session;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

pub struct OnnxEmbedder {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<Tokenizer>,
    dims: usize,
    model_id: String,
}

impl OnnxEmbedder {
    pub fn new(model_dir: &Path, model_name: String) -> anyhow::Result<Self> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to create session builder: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow::anyhow!("Failed to set intra threads: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| anyhow::anyhow!("Failed to load ONNX model: {e}"))?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        let dims = 384; // all-MiniLM-L6-v2
        let model_id = format!("onnx:{model_name}");

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
            dims,
            model_id,
        })
    }
}

impl EmbeddingPort for OnnxEmbedder {
    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>>> {
        let text = text.to_string();
        let session = self.session.clone();
        let tokenizer = self.tokenizer.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_inference(&session, &tokenizer, &text))
                .await
                .map_err(|e| BrainError::Embedding(e.to_string()))?
        })
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

fn run_inference(session: &Mutex<Session>, tokenizer: &Tokenizer, text: &str) -> Result<Vec<f32>> {
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| BrainError::Embedding(format!("Tokenization failed: {e}")))?;

    let ids = encoding.get_ids();
    let attention = encoding.get_attention_mask();
    let seq_len = ids.len();

    let input_ids: Array2<i64> =
        Array2::from_shape_vec((1, seq_len), ids.iter().map(|&v| v as i64).collect())
            .map_err(|e| BrainError::Embedding(e.to_string()))?;
    let attention_mask: Array2<i64> =
        Array2::from_shape_vec((1, seq_len), attention.iter().map(|&v| v as i64).collect())
            .map_err(|e| BrainError::Embedding(e.to_string()))?;
    let token_type_ids: Array2<i64> = Array2::zeros((1, seq_len));

    let input_ids_tensor = TensorRef::from_array_view(&input_ids)
        .map_err(|e| BrainError::Embedding(format!("Failed to create input_ids tensor: {e}")))?;
    let attention_mask_tensor = TensorRef::from_array_view(&attention_mask).map_err(|e| {
        BrainError::Embedding(format!("Failed to create attention_mask tensor: {e}"))
    })?;
    let token_type_ids_tensor = TensorRef::from_array_view(&token_type_ids).map_err(|e| {
        BrainError::Embedding(format!("Failed to create token_type_ids tensor: {e}"))
    })?;

    let mut session = session
        .lock()
        .map_err(|e| BrainError::Embedding(format!("Session lock poisoned: {e}")))?;

    let outputs = session
        .run(ort::inputs! {
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor,
        })
        .map_err(|e| BrainError::Embedding(format!("ONNX inference failed: {e}")))?;

    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| BrainError::Embedding(format!("Failed to extract output tensor: {e}")))?;

    let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
    let embeddings = Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())
        .map_err(|e| BrainError::Embedding(e.to_string()))?;

    Ok(mean_pool(&embeddings, &attention_mask))
}

fn mean_pool(embeddings: &Array3<f32>, attention_mask: &Array2<i64>) -> Vec<f32> {
    let mask = attention_mask.mapv(|v| v as f32);
    let mask_expanded = mask.insert_axis(Axis(2));
    let masked = embeddings * &mask_expanded;
    let summed = masked.sum_axis(Axis(1));
    let counts = mask_expanded.sum_axis(Axis(1));
    let pooled = &summed / &counts;
    let vec = pooled.row(0).to_vec();

    // L2 normalize
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        vec.into_iter().map(|x| x / norm).collect()
    } else {
        vec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires model files at ~/.config/brain-mcp/models/all-MiniLM-L6-v2
    async fn test_onnx_embed_produces_vector() {
        let model_dir = dirs::config_dir()
            .unwrap()
            .join("brain-mcp/models/all-MiniLM-L6-v2");
        if !model_dir.exists() {
            eprintln!("Skipping: model not downloaded at {}", model_dir.display());
            return;
        }
        let embedder = OnnxEmbedder::new(&model_dir, "all-MiniLM-L6-v2".into()).unwrap();
        let result = embedder.embed("hello world").await.unwrap();
        assert_eq!(result.len(), 384);

        // Check it's normalized (magnitude ~ 1.0)
        let mag: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (mag - 1.0).abs() < 0.01,
            "expected magnitude ~1.0, got {mag}"
        );
    }
}
