#!/bin/sh
# Auto-download + checksum-verify the GGUF model on first container start.
# The ../models volume mount persists it, so this only runs once per host.
set -eu

: "${MODEL_PATH:=/models/qwen2.5-1.5b-instruct-q4_k_m.gguf}"
: "${MODEL_URL:=https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf}"
: "${MODEL_SHA256:=6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e}"

if [ ! -f "$MODEL_PATH" ]; then
  echo "[entrypoint] $MODEL_PATH not found -- downloading from $MODEL_URL ..."
  curl -fL --retry 3 -o "$MODEL_PATH.part" "$MODEL_URL"
  echo "$MODEL_SHA256  $MODEL_PATH.part" | sha256sum -c -
  mv "$MODEL_PATH.part" "$MODEL_PATH"
  echo "[entrypoint] model downloaded and checksum-verified."
fi

exec "$@"
