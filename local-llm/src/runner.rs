//! Inference thread abstraction for llama.cpp.
//!
//! `LlamaContext` is `!Send`, so all inference must happen on the thread
//! that created the context. This module provides [`LlamaRunner`] which
//! owns a shared `LlamaModel` (which *is* `Send + Sync`) and spawns
//! short-lived `std::thread`s for each inference or embedding request,
//! communicating results back via channels.

use std::sync::Arc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
#[allow(deprecated)]
use llama_cpp_2::model::Special;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::error::LocalModelError;

// ─── Configuration ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub context_length: u32,
    pub batch_size: u32,
    pub gpu_layers: u32,
    pub n_threads: u32,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            context_length: 8192,
            batch_size: 512,
            gpu_layers: 0,
            n_threads: 4,
        }
    }
}

// ─── Token Events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TokenEvent {
    Token(String),
    Done {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Error(String),
}

// ─── LlamaRunner ───────────────────────────────────────────────────────────

pub struct LlamaRunner {
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    config: RunnerConfig,
}

impl std::fmt::Debug for LlamaRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaRunner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl LlamaRunner {
    pub fn load(
        model_path: impl AsRef<std::path::Path>,
        config: RunnerConfig,
    ) -> Result<Self, LocalModelError> {
        let backend = LlamaBackend::init().map_err(|e| {
            LocalModelError::loading_message(format!("llama backend init failed: {e}"))
        })?;

        let model_params = LlamaModelParams::default().with_n_gpu_layers(config.gpu_layers);

        let model = LlamaModel::load_from_file(&backend, model_path, &model_params).map_err(
            |e| LocalModelError::loading_message(format!("GGUF model load failed: {e}")),
        )?;

        debug!(
            vocab = model.n_vocab(),
            embd = model.n_embd(),
            ctx_train = model.n_ctx_train(),
            "model loaded"
        );

        Ok(Self {
            backend: Arc::new(backend),
            model: Arc::new(model),
            config,
        })
    }

    /// Apply the model's chat template to a list of messages and return the
    /// formatted prompt string.
    pub fn apply_chat_template(
        &self,
        messages: &[LlamaChatMessage],
        add_assistant: bool,
    ) -> Result<String, LocalModelError> {
        let template = self.model.chat_template(None).map_err(|e| {
            LocalModelError::inference(format!("model has no chat template: {e}"))
        })?;

        self.model
            .apply_chat_template(&template, messages, add_assistant)
            .map_err(|e| {
                LocalModelError::inference(format!("chat template application failed: {e}"))
            })
    }

    /// Tokenize a prompt string into token IDs.
    pub fn tokenize(&self, prompt: &str) -> Result<Vec<llama_cpp_2::token::LlamaToken>, LocalModelError> {
        self.model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| LocalModelError::inference(format!("tokenization failed: {e}")))
    }

    /// Start streaming token generation on a background thread.
    ///
    /// Returns a channel receiver that yields `TokenEvent`s. The inference
    /// runs on a dedicated `std::thread` because `LlamaContext` is `!Send`.
    pub fn generate_stream(
        &self,
        tokens: Vec<llama_cpp_2::token::LlamaToken>,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<TokenEvent> {
        let (tx, rx) = mpsc::channel(64);
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let config = self.config.clone();

        std::thread::spawn(move || {
            let result = run_inference(&backend, &model, &config, &tokens, &tx, &cancel);
            if let Err(e) = result {
                let _ = tx.blocking_send(TokenEvent::Error(e.to_string()));
            }
        });

        rx
    }

    /// Generate embeddings for the given text on a background thread.
    pub fn generate_embedding(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, LocalModelError> {
        let tokens = self
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| LocalModelError::embedding(format!("tokenization failed: {e}")))?;

        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let config = self.config.clone();

        let handle = std::thread::spawn(move || -> Result<Vec<f32>, LocalModelError> {
            let ctx_params = LlamaContextParams::default()
                .with_n_ctx(std::num::NonZeroU32::new(config.context_length))
                .with_n_threads(i32::try_from(config.n_threads).unwrap_or(i32::MAX))
                .with_embeddings(true);

            let mut ctx = model.new_context(&backend, ctx_params).map_err(|e| {
                LocalModelError::embedding(format!("context creation failed: {e}"))
            })?;

            let mut batch = LlamaBatch::new(tokens.len(), 1);
            batch.add_sequence(&tokens, 0, false).map_err(|e| {
                LocalModelError::embedding(format!("batch add failed: {e}"))
            })?;

            ctx.decode(&mut batch).map_err(|e| {
                LocalModelError::embedding(format!("decode failed: {e}"))
            })?;

            let embeddings = ctx.embeddings_seq_ith(0).map_err(|e| {
                LocalModelError::embedding(format!("embedding extraction failed: {e}"))
            })?;

            Ok(embeddings.to_vec())
        });

        handle
            .join()
            .map_err(|_| LocalModelError::embedding("embedding thread panicked".to_string()))?
    }

    /// Generate embeddings for multiple texts.
    pub fn generate_embeddings_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, LocalModelError> {
        texts.iter().map(|t| self.generate_embedding(t)).collect()
    }
}

// ─── Inference loop (runs on dedicated thread) ─────────────────────────────

fn run_inference(
    backend: &LlamaBackend,
    model: &LlamaModel,
    config: &RunnerConfig,
    tokens: &[llama_cpp_2::token::LlamaToken],
    tx: &mpsc::Sender<TokenEvent>,
    cancel: &CancellationToken,
) -> Result<(), LocalModelError> {
    #[allow(clippy::cast_possible_wrap)]
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(config.context_length))
        .with_n_batch(config.batch_size)
        .with_n_threads(i32::try_from(config.n_threads).unwrap_or(i32::MAX));

    let mut ctx = model.new_context(backend, ctx_params).map_err(|e| {
        LocalModelError::inference(format!("context creation failed: {e}"))
    })?;

    let batch_size = config.batch_size as usize;
    let mut batch = LlamaBatch::new(batch_size, 1);
    let prompt_len = tokens.len();

    // Add prompt tokens to batch
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == prompt_len - 1;
        let pos = i32::try_from(i).unwrap_or(i32::MAX);
        batch
            .add(*token, pos, &[0], is_last)
            .map_err(|e| LocalModelError::inference(format!("batch add failed: {e}")))?;
    }

    // Decode prompt
    ctx.decode(&mut batch).map_err(|e| {
        LocalModelError::inference(format!("prompt decode failed: {e}"))
    })?;

    // Sample tokens
    let mut sampler = LlamaSampler::greedy();
    let mut completion_tokens: u32 = 0;
    let prompt_len_u32 = u32::try_from(prompt_len).unwrap_or(u32::MAX);
    let max_tokens = config.context_length.saturating_sub(prompt_len_u32);

    for _ in 0..max_tokens {
        if cancel.is_cancelled() {
            break;
        }

        let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if model.is_eog_token(new_token) {
            break;
        }

        completion_tokens += 1;

        #[allow(deprecated)]
        let token_bytes = model
            .token_to_bytes(new_token, Special::Tokenize)
            .unwrap_or_default();
        let token_str = String::from_utf8_lossy(&token_bytes).into_owned();

        if tx.blocking_send(TokenEvent::Token(token_str)).is_err() {
            return Ok(());
        }

        let pos = i32::try_from(prompt_len)
            .unwrap_or(0)
            .saturating_add(i32::try_from(completion_tokens).unwrap_or(0))
            .saturating_sub(1);

        batch.clear();
        batch
            .add(new_token, pos, &[0], true)
            .map_err(|e| LocalModelError::inference(format!("batch add failed: {e}")))?;

        ctx.decode(&mut batch).map_err(|e| {
            LocalModelError::inference(format!("decode failed: {e}"))
        })?;
    }

    let _ = tx.blocking_send(TokenEvent::Done {
        prompt_tokens: prompt_len_u32,
        completion_tokens,
    });

    Ok(())
}

// ─── Compile-time assertions ───────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LlamaRunner>();
};
