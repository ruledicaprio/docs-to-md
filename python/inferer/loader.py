"""Lazy loader for the quantized Qwen 2.5 GGUF model.

The model is loaded once and kept warm for the lifetime of the inferer process
(fixing the old per-document cold-reload penalty). A dummy warm-up generation is
run at load time so the first real request doesn't eat the initialization cost.
`llama_cpp` is imported lazily so mock mode and tests don't need it installed.
"""

import os
import threading


class ModelLoader:
    def __init__(self, model_path: str | None = None):
        self.model_path = model_path or os.environ.get(
            "MODEL_PATH", "./qwen2.5-1.5b-instruct-q4_k_m.gguf"
        )
        self._llm = None
        # llama.cpp's Llama object isn't safe for concurrent calls (shared
        # KV-cache/context state); the gRPC server's ThreadPoolExecutor can
        # dispatch several RPCs at once, so this is the resource's own
        # serialization, not just relying on callers to behave.
        self._lock = threading.Lock()

    @property
    def loaded(self) -> bool:
        return self._llm is not None

    def load(self) -> None:
        """Load the model into memory and run a one-token warm-up."""
        from llama_cpp import Llama  # imported lazily

        self._llm = Llama(
            model_path=self.model_path,
            n_ctx=2048,
            n_threads=int(os.environ.get("LLAMA_THREADS", "4")),
            n_gpu_layers=-1,  # offload all layers to VRAM when CUDA is available
            verbose=False,
        )
        # Warm-up: force graph/kv-cache init so the first user request is fast.
        self._llm(
            "<|im_start|>system\nok<|im_end|>\n<|im_start|>assistant",
            max_tokens=1,
            temperature=0.0,
        )

    def generate(self, prompt: str) -> str:
        """Run one deterministic (temperature 0) extraction, returning raw text."""
        if self._llm is None:
            self.load()
        with self._lock:
            out = self._llm(
                prompt,
                max_tokens=500,
                temperature=0.0,
                stop=["<|im_end|>"],
            )
        return out["choices"][0]["text"].strip()

    def generate_stream(self, prompt: str):
        """Same generation as `generate`, yielding raw text deltas as they're
        produced. Callers should strip only the accumulated text, not each
        delta (whitespace tokens are meaningful mid-stream). Holds the lock
        for the whole generation, so callers must fully consume (or close)
        the generator rather than abandoning it mid-stream."""
        if self._llm is None:
            self.load()
        with self._lock:
            for chunk in self._llm(
                prompt,
                max_tokens=500,
                temperature=0.0,
                stop=["<|im_end|>"],
                stream=True,
            ):
                yield chunk["choices"][0]["text"]
