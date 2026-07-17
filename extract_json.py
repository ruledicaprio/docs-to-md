# extract_json.py
import sys
import json
import os

# Windows consoles default to cp1252 when spawned as a subprocess — force UTF-8
# so the emoji status output doesn't crash the pipeline.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")

from llama_cpp import Llama

# Point to the model you just downloaded (override with the MODEL_PATH env var)
MODEL_PATH = os.environ.get("MODEL_PATH", "./qwen2.5-1.5b-instruct-q4_k_m.gguf")

def extract_id_data(markdown_path: str):
    if not os.path.exists(markdown_path):
        print(f"❌ Markdown file not found: {markdown_path}")
        return

    with open(markdown_path, 'r', encoding='utf-8') as f:
        md_content = f.read()

    print("⏳ Loading Qwen 2.5 1.5B GGUF model (this may take a few seconds on first run)...")
    
    # Initialize the local LLM
    llm = Llama(
        model_path=MODEL_PATH,
        n_ctx=2048,          # Context window
        n_threads=4,         # Adjust to your CPU core count
        n_gpu_layers=-1,     # Offload all layers to your GTX 970 VRAM if CUDA is enabled
        verbose=False
    )

    # Strict system prompt for JSON extraction
    prompt = f"""<|im_start|>system
You are an expert, highly accurate identity document parser. Your task is to extract specific fields from the provided OCR Markdown text into a strict, valid JSON object. 
If a field is not found or is illegible, use null. Do not invent data.
<|im_end|>
<|im_start|>user
Extract these fields: document_type, issuing_country, document_number, surname, given_names, nationality, date_of_birth, sex, date_of_expiry, mrz_line.

OCR Markdown Text:
{md_content}

Output ONLY valid JSON. No markdown formatting, no explanations, no code blocks.
<|im_end|>
<|im_start|>assistant"""

    print("🔍 Running local inference...")
    result = llm(
        prompt,
        max_tokens=500,
        temperature=0.0,     # 0.0 ensures deterministic, factual extraction
        stop=["<|im_end|>"]
    )
    
    raw_output = result['choices'][0]['text'].strip()
    
    # Clean up potential markdown artifacts just in case the model slips
    if raw_output.startswith("```json"):
        raw_output = raw_output[7:]
    if raw_output.endswith("```"):
        raw_output = raw_output[:-3]
        
    try:
        parsed_json = json.loads(raw_output)
        json_path = os.path.splitext(markdown_path)[0] + '.json'
        
        with open(json_path, 'w', encoding='utf-8') as f:
            json.dump(parsed_json, f, indent=2)
            
        print(f"✅ Successfully extracted and saved JSON to: {json_path}")
        print("\n--- Extracted Data Preview ---")
        print(json.dumps(parsed_json, indent=2))
        
    except json.JSONDecodeError as e:
        print(f"❌ Failed to parse JSON output from model.")
        print(f"Error: {e}")
        print("Raw model output:\n", raw_output)

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python extract_json.py <path_to_markdown.md>")
    else:
        extract_id_data(sys.argv[1])