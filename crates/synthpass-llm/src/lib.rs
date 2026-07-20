//! In-process llama.cpp inference for Tier 2 — replaces the Python gRPC
//! sidecar (`python/inferer/`) with a native Rust implementation running the
//! same Qwen2.5-1.5B-Instruct GGUF via `llama-cpp-2`.
//!
//! [`NativeLlm`] loads the model once (~1 GB mmap) and is kept warm for the
//! process lifetime; each [`NativeLlm::extract`] / [`NativeLlm::extract_stream`]
//! call creates a fresh, cheap [`llama_cpp_2::context::LlamaContext`] so
//! generations never leak KV-cache state into one another. All methods are
//! blocking — callers on an async runtime (see `synthpass-pipeline`) must run them
//! via `spawn_blocking`, mirroring how the native OCR engine is wrapped.

pub mod prompt;
pub mod repair;
pub mod verify;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Mutex;
use synthpass_core::Extraction;
use zeroize::Zeroizing;

/// The ChatML stop sequence Qwen emits at the end of a turn.
const STOP_SEQUENCE: &str = "<|im_end|>";
/// Mirrors the Python inferer's `max_tokens=500` cap.
const MAX_NEW_TOKENS: i32 = 500;

pub struct NativeLlm {
    backend: LlamaBackend,
    model: LlamaModel,
    n_ctx: NonZeroU32,
    /// Defense-in-depth serialization of generations, mirroring the Python
    /// loader's `threading.Lock` — the real single-flight guarantee lives in
    /// `synthpass-pipeline`'s semaphore, but this crate does not assume a
    /// well-behaved caller.
    gen_lock: Mutex<()>,
}

impl NativeLlm {
    /// Load the GGUF at `model_path` and prepare a warm, reusable model.
    /// `n_ctx` is the context window size (2048 for the Qwen fine-tune this
    /// workspace ships, matching the Python inferer).
    pub fn load(model_path: &Path, n_ctx: u32) -> Result<Self, String> {
        let backend = LlamaBackend::init().map_err(|e| format!("llama backend init: {e}"))?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| format!("failed to load GGUF at {}: {e}", model_path.display()))?;
        let n_ctx = NonZeroU32::new(n_ctx).ok_or("n_ctx must be non-zero")?;
        Ok(Self {
            backend,
            model,
            n_ctx,
            gen_lock: Mutex::new(()),
        })
    }

    /// Run one deterministic (greedy) extraction, returning the parsed
    /// [`Extraction`].
    pub fn extract(&self, markdown: &str) -> Result<Extraction, String> {
        let raw = self.generate(markdown, |_delta| {})?;
        repair::parse_extraction(&raw)
    }

    /// Same extraction as [`Self::extract`], but calls `on_delta` with each
    /// incremental piece of text as it is generated.
    pub fn extract_stream(
        &self,
        markdown: &str,
        on_delta: impl FnMut(&str),
    ) -> Result<Extraction, String> {
        let raw = self.generate(markdown, on_delta)?;
        repair::parse_extraction(&raw)
    }

    /// Greedy-sample a completion for `markdown`'s ChatML prompt, stopping at
    /// `<|im_end|>`, an end-of-generation token, or [`MAX_NEW_TOKENS`].
    ///
    /// The returned raw text (and the prompt built internally) contain PII
    /// from the source document, and are never returned past
    /// [`extract`]/[`extract_stream`] — both wrap them in [`Zeroizing`] so
    /// they're wiped from memory once [`repair::parse_extraction`] has
    /// consumed them.
    ///
    /// [`extract`]: Self::extract
    /// [`extract_stream`]: Self::extract_stream
    fn generate(
        &self,
        markdown: &str,
        mut on_delta: impl FnMut(&str),
    ) -> Result<Zeroizing<String>, String> {
        let _guard = self
            .gen_lock
            .lock()
            .map_err(|_| "generation lock poisoned")?;

        let prompt_text = Zeroizing::new(prompt::build_prompt(markdown));
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(self.n_ctx))
            .with_n_batch(self.n_ctx.get());
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| format!("failed to create llama context: {e}"))?;

        let tokens = self
            .model
            .str_to_token(&prompt_text, AddBos::Always)
            .map_err(|e| format!("tokenization failed: {e}"))?;
        if tokens.len() as u32 >= self.n_ctx.get() {
            return Err(format!(
                "prompt ({} tokens) does not fit in context window ({})",
                tokens.len(),
                self.n_ctx
            ));
        }

        let mut batch = LlamaBatch::new(self.n_ctx.get() as usize, 1);
        let last_index = tokens.len() as i32 - 1;
        for (i, token) in (0_i32..).zip(tokens) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| format!("batch add: {e}"))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| format!("prompt decode failed: {e}"))?;

        let mut sampler = LlamaSampler::chain_simple([LlamaSampler::greedy()]);
        // Stateful UTF-8 decoder reused across the whole generation: a single
        // token can be a partial multi-byte character, so decoding must carry
        // state from one `token_to_piece` call to the next.
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut n_cur = batch.n_tokens();
        let end_at = n_cur + MAX_NEW_TOKENS;
        let mut output = Zeroizing::new(String::new());

        while n_cur < end_at {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            if self.model.is_eog_token(token) {
                break;
            }

            let piece = self
                .model
                .token_to_piece(token, &mut decoder, false, None)
                .map_err(|e| format!("token_to_piece failed: {e}"))?;
            on_delta(&piece);
            output.push_str(&piece);
            if output.ends_with(STOP_SEQUENCE) {
                // Computed as a separate binding (rather than inline in the
                // `truncate` call) because `output: Zeroizing<String>`
                // doesn't get the same two-phase-borrow leniency through
                // `DerefMut` that a plain `String` receiver would here.
                let new_len = output.len() - STOP_SEQUENCE.len();
                output.truncate(new_len);
                break;
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| format!("batch add: {e}"))?;
            ctx.decode(&mut batch)
                .map_err(|e| format!("decode failed: {e}"))?;
            n_cur += 1;
        }

        Ok(Zeroizing::new(output.trim().to_string()))
    }
}
