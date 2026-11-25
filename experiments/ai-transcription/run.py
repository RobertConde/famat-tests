"""Transcribe every sampled page with one model, saving Markdown+LaTeX per page.

Usage: uv run run.py <model-key> [--limit N]
Writes runs/<model-key>/<tid>/<page>.md and runs/<model-key>/stats.jsonl.
Pages that already have output are skipped, so an interrupted run resumes.
"""

import argparse
import json
import re
import time
from pathlib import Path

import mlx.core as mx

HERE = Path(__file__).parent
PAGES = HERE / "pages"
RUNS = HERE / "runs"

OLMOCR_PROMPT = (
    "Attached is one page of a document that you must process. "
    "Just return the plain text representation of this document as if you were reading it naturally. "
    "Convert equations to LateX and tables to HTML.\n"
    "If there are any figures or charts, label them with the following markdown syntax "
    "![Alt text describing the contents of the figure](page_startx_starty_width_height.png)\n"
    "Return your output as markdown, with a front matter section on top specifying values for the "
    "primary_language, is_rotation_valid, rotation_correction, is_table, and is_diagram parameters."
)
QWEN_PROMPT = (
    "Transcribe this page of a high-school math competition test exactly. "
    "Output Markdown. Write all mathematics in LaTeX, inline as $...$ and displayed as $$...$$. "
    "Keep question numbers and answer-choice letters exactly as printed. "
    "Do not solve, summarize, or add anything that is not on the page."
)

NANONETS_PROMPT = (
    "Extract the text from the above document as if you were reading it naturally. "
    "Return the tables in html format. Return the equations in LaTeX representation. "
    "If there is an image in the document and image caption is not present, add a small description of the image "
    "inside the <img></img> tag; otherwise, add the image caption inside <img></img>. "
    "Watermarks should be wrapped in brackets. Ex: <watermark>OFFICIAL COPY</watermark>. "
    "Page numbers should be wrapped in brackets. Ex: <page_number>14</page_number> or <page_number>9/22</page_number>. "
    "Prefer using \u2610 and \u2611 for check boxes."
)

# key: (Hugging Face repo, prompt, approximate weight size in GB)
MODELS = {
    "paddleocr-vl-1.5": ("mlx-community/PaddleOCR-VL-1.5-bf16", "OCR:", 1.9),
    "glm-ocr": ("mlx-community/GLM-OCR-bf16", "Text Recognition:", 2.2),
    "mineru-2.5-pro": ("opendatalab/MinerU2.5-Pro-2605-1.2B", None, 2.3),
    "nanonets-ocr2-3b-8bit": ("mlx-community/Nanonets-OCR2-3B-8bit", NANONETS_PROMPT, 3.6),
    "olmocr-2-7b-6bit": ("mlx-community/olmOCR-2-7B-1025-6bit", OLMOCR_PROMPT, 6.6),
    "olmocr-2-7b-8bit": ("mlx-community/olmOCR-2-7B-1025-8bit", OLMOCR_PROMPT, 8.6),
    "qwen3-vl-8b-8bit": ("mlx-community/Qwen3-VL-8B-Instruct-8bit", QWEN_PROMPT, 9.2),
}


def clean(key: str, text: str) -> str:
    if key.startswith("olmocr"):
        text = re.sub(r"\A\s*(?:---\n.*?\n---|```yaml\n.*?\n```)\n", "", text, flags=re.S)
    return text.strip()


def mineru_transcriber(repo: str):
    from mineru_vl_utils import MinerUClient
    from PIL import Image

    client = MinerUClient(backend="mlx-engine", model_path=repo)

    def transcribe(image: Path) -> tuple[str, int | None]:
        parts = []
        for block in client.two_step_extract(Image.open(image)):
            content = (block.get("content") or "").strip()
            if not content:
                continue
            if block.get("type") == "equation" and not content.startswith("$$"):
                content = f"$$\n{content}\n$$"
            parts.append(content)
        return "\n\n".join(parts), None

    return transcribe


def vlm_transcriber(repo: str, prompt: str):
    from mlx_vlm import apply_chat_template, generate, load

    model, processor = load(repo)
    formatted = apply_chat_template(processor, model.config, prompt, num_images=1)

    def transcribe(image: Path) -> tuple[str, int | None]:
        result = generate(model, processor, formatted, image=[str(image)],
                          max_tokens=4096, temperature=0.0, repetition_penalty=1.05, verbose=False)
        return result.text, getattr(result, "generation_tokens", None)

    return transcribe


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("model", choices=MODELS)
    ap.add_argument("--limit", type=int, help="only the first N pages (smoke test)")
    args = ap.parse_args()

    repo, prompt, _ = MODELS[args.model]
    manifest = json.loads((PAGES / "manifest.json").read_text())
    pages = [(t["tid"], p) for t in manifest for p in t["pages"]][: args.limit]

    out_dir = RUNS / args.model
    out_dir.mkdir(parents=True, exist_ok=True)
    todo = [(tid, p) for tid, p in pages if not (out_dir / str(tid) / f"{Path(p).stem}.md").exists()]
    if not todo:
        print(f"{args.model}: nothing to do")
        return

    t0 = time.time()
    transcribe = mineru_transcriber(repo) if prompt is None else vlm_transcriber(repo, prompt)
    print(f"{args.model}: loaded in {time.time() - t0:.0f}s, {len(todo)} pages to do", flush=True)

    with open(out_dir / "stats.jsonl", "a") as stats:
        for tid, page in todo:
            mx.reset_peak_memory()
            start = time.time()
            text, tokens = transcribe(PAGES / str(tid) / page)
            seconds = time.time() - start
            dest = out_dir / str(tid) / f"{Path(page).stem}.md"
            dest.parent.mkdir(exist_ok=True)
            dest.write_text(clean(args.model, text) + "\n")
            row = dict(tid=tid, page=page, seconds=round(seconds, 1), tokens=tokens,
                       peak_gb=round(mx.get_peak_memory() / 1e9, 2))
            stats.write(json.dumps(row) + "\n")
            stats.flush()
            print(f"  {tid}/{page}  {seconds:5.1f}s  peak {row['peak_gb']} GB", flush=True)


if __name__ == "__main__":
    main()
