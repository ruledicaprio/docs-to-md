"""Prompt construction for the structured-extraction task.

Carried over from the original `extract_json.py` — a strict system prompt that
forbids markdown fences and demands raw JSON with `null` for missing fields.
"""

import html

SYSTEM = (
    "You are an expert, highly accurate identity document parser. Your task is "
    "to extract specific fields from the provided OCR Markdown text into a "
    "strict, valid JSON object. If a field is not found or is illegible, use "
    "null. Do not invent data."
)

# Fields requested from the model (subset of the canonical schema; provenance
# fields are added downstream, not by the model).
FIELDS = [
    "document_type",
    "issuing_country",
    "document_number",
    "surname",
    "given_names",
    "nationality",
    "date_of_birth",
    "sex",
    "date_of_expiry",
    "mrz_line",
]


def build_prompt(md_content: str) -> str:
    """Build the Qwen2.5 ChatML prompt for one document's OCR Markdown."""
    fields = ", ".join(FIELDS)
    # docling renders MRZ filler chevrons as HTML entities (`<` -> `&lt;`) in its
    # Markdown output. Left escaped, the model echoes `&lt;` verbatim into
    # fields like mrz_line instead of the literal `<` that was printed on the
    # document, so unescape before the model ever sees the text.
    md_content = html.unescape(md_content)
    return (
        "<|im_start|>system\n"
        f"{SYSTEM}\n"
        "<|im_end|>\n"
        "<|im_start|>user\n"
        f"Extract these fields: {fields}.\n\n"
        "OCR Markdown Text:\n"
        f"{md_content}\n\n"
        "Output ONLY valid JSON. No markdown formatting, no explanations, no "
        "code blocks.\n"
        "<|im_end|>\n"
        "<|im_start|>assistant"
    )
