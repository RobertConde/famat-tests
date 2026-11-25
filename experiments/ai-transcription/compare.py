"""Split each model's transcripts into questions, measure cross-model agreement, build a review page.

Usage: uv run compare.py [--models a,b,c] [--threshold 0.97]
Writes report.md and review.html (open it locally; it reads pages/ next to it).
"""

import argparse
import difflib
import html
import itertools
import json
import random
import re
from collections import Counter
from pathlib import Path

HERE = Path(__file__).parent
PAGES = HERE / "pages"
RUNS = HERE / "runs"

# "1." / "**1)**" at line start, or a heading like "ALGEBRA I -- TEAM QUESTION #1".
QUESTION_START = re.compile(
    r"^\s*(?:#+\s*)?(?:\*\*)?\s*(\d{1,2})\s*(?:\*\*)?\s*[.)]|^.*\bQUESTION\s*\\?#\s*(\d{1,2})\b", re.I
)


def split_questions(text: str) -> dict[int, str]:
    """Cut a Test transcript at question numbers, accepting only the next number in sequence."""
    questions: dict[int, list[str]] = {}
    current, expected = None, 1
    for line in text.splitlines():
        m = QUESTION_START.match(line)
        if m and int(m[1] or m[2]) == expected:
            current, expected = expected, expected + 1
            questions[current] = []
        if current is not None:
            questions[current].append(line)
    return {n: "\n".join(lines).strip() for n, lines in questions.items()}


CHOICE = re.compile(r"(?:^|\s)\(?([A-Ea-e])[.)]\s")


def canonical_choices(text: str) -> str:
    """Reorder answer choices by letter, so a two-column layout (A C / B D) matches a list (A B C D)."""
    parts = CHOICE.split(text)
    if len(parts) < 5:  # fewer than two choices: leave as is
        return text
    stem, pairs = parts[0], list(zip(parts[1::2], parts[2::2]))
    return stem + "".join(f" {k.lower()}. {v}" for k, v in sorted(pairs, key=lambda kv: kv[0].lower()))


def normalize(text: str) -> str:
    """Strip presentation so only content is compared: markup, delimiters, spacing, sizing commands."""
    t = QUESTION_START.sub("", text, count=1)
    t = re.sub(r"!\[[^\]]*\]\([^)]*\)|<img>.*?</img>", "", t, flags=re.S)  # figure placeholders
    t = canonical_choices(t)
    t = re.sub(r"\\(?:text|mathrm|operatorname|mathbf|textbf|mathit|textit)\s*\{([^{}]*)\}", r"\1", t)
    t = re.sub(r"\\(?:left|right|displaystyle|,|;|!|quad|qquad)", "", t)
    t = re.sub(r"\\[()\[\]]", "", t)
    t = re.sub(r"\\[lcd]?dots|…|\.{3,}", "...", t)
    t = t.replace("\\dfrac", "\\frac").replace("\\tfrac", "\\frac").replace("\\cdot", "*").replace("\\times", "*")
    t = re.sub(r"[$*_#|`]|<[^>]+>", "", t)
    t = re.sub(r"\(\s*([a-e])\s*\)|\b([a-e])\s*\)", r"\1\2.", t)  # (a) / a) -> a.
    t = re.sub(r"[{}\s]", "", t)
    return t.lower()


def strip_furniture(pages: list[str]) -> list[str]:
    """Drop running headers/footers: "Page N of M", and lines within 3 lines of a page edge that repeat
    at the edge of another page of the same Test. Question headings are always kept."""
    key = lambda line: re.sub(r"[\W\d_]+", "", line.lower())
    edges = [[l for l in p.splitlines() if l.strip()] for p in pages]
    edges = [set(key(l) for l in ls[:3] + ls[-3:] if len(key(l)) > 8) for ls in edges]
    repeated = {k for k, c in Counter(k for e in edges for k in e).items() if c >= 2}
    footer = re.compile(r"\bpage\s*\d+\s*(?:of\s*\d+)?\s*$", re.I)
    out = []
    for page, edge in zip(pages, edges):
        lines = [l for l in page.splitlines() if l.strip()]
        keep = [l for i, l in enumerate(lines)
                if QUESTION_START.match(l)
                or not (footer.search(l) or ((i < 3 or i >= len(lines) - 3) and key(l) in repeated))]
        out.append("\n".join(keep))
    return out


def similarity(a: str, b: str) -> float:
    return difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()


def load(model: str, manifest: list[dict]) -> dict[int, dict[int, str]]:
    out = {}
    for t in manifest:
        parts = [(RUNS / model / str(t["tid"]) / f"{Path(p).stem}.md") for p in t["pages"]]
        if not all(p.exists() for p in parts):
            continue
        out[t["tid"]] = split_questions("\n\n".join(strip_furniture([p.read_text() for p in parts])))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", help="comma-separated; default: every model with complete runs")
    ap.add_argument("--review", help="comma-separated models whose disagreements form the review queue; default: all")
    ap.add_argument("--threshold", type=float, default=0.97, help="normalized similarity counted as agreement")
    args = ap.parse_args()

    manifest = json.loads((PAGES / "manifest.json").read_text())
    models = args.models.split(",") if args.models else sorted(p.name for p in RUNS.iterdir() if p.is_dir())
    data = {m: load(m, manifest) for m in models}
    models = [m for m in models if len(data[m]) == len(manifest)]
    review = args.review.split(",") if args.review else models
    stats = {m: [json.loads(l) for l in (RUNS / m / "stats.jsonl").read_text().splitlines()] for m in models}

    # Reference question count per Test: the most common count across models.
    expected = {t["tid"]: Counter(len(data[m][t["tid"]]) for m in models).most_common(1)[0][0] for t in manifest}

    items, pair_agree = [], Counter()
    for t in manifest:
        tid = t["tid"]
        for n in range(1, expected[tid] + 1):
            texts = {m: data[m][tid].get(n, "") for m in models}
            norm = {m: normalize(x) for m, x in texts.items()}
            agree = {(a, b): similarity(norm[a], norm[b]) >= args.threshold for a, b in itertools.combinations(models, 2)}
            for pair, ok in agree.items():
                pair_agree[pair] += ok
            # Largest group of models that all agree with each other's first member.
            groups = []
            for m in models:
                for g in groups:
                    if agree.get((g[0], m), agree.get((m, g[0]), False)):
                        g.append(m)
                        break
                else:
                    groups.append([m])
            best = max(groups, key=len)
            review_agree = all(agree.get((a, b), agree.get((b, a))) for a, b in itertools.combinations(review, 2))
            items.append(dict(tid=tid, n=n, texts=texts, consensus=best, all_agree=len(best) == len(models),
                              review_agree=review_agree))

    total = len(items)
    lines = [
        "# Pilot: open-model agreement on real Test pages\n",
        f"{len(manifest)} Tests, {sum(len(t['pages']) for t in manifest)} pages, {total} questions "
        f"(question count per Test = the most common count across models). Agreement = normalized similarity "
        f">= {args.threshold}.\n",
        "## Per model\n",
        "| Model | Questions split 1..N as expected | Sec/page (median) | Peak memory GB (max) | Agrees with majority |",
        "|---|---|---|---|---|",
    ]
    for m in models:
        ok_counts = sum(len(data[m][t["tid"]]) == expected[t["tid"]] for t in manifest)
        secs = sorted(r["seconds"] for r in stats[m])
        in_majority = sum(m in it["consensus"] and len(it["consensus"]) > 1 for it in items)
        lines.append(f"| {m} | {ok_counts}/{len(manifest)} Tests | {secs[len(secs) // 2]} | "
                     f"{max(r['peak_gb'] for r in stats[m])} | {in_majority}/{total} ({in_majority / total:.0%}) |")
    lines += ["", "## Pairwise agreement (share of questions)\n", "| | " + " | ".join(models) + " |",
              "|---" * (len(models) + 1) + "|"]
    for a in models:
        row = []
        for b in models:
            k = (a, b) if (a, b) in pair_agree else (b, a)
            row.append("—" if a == b else f"{pair_agree[k] / total:.0%}")
        lines.append(f"| {a} | " + " | ".join(row) + " |")
    n_all = sum(it["all_agree"] for it in items)
    lines += ["", f"**All models agree:** {n_all}/{total} questions ({n_all / total:.0%}). "
              f"**Need review (any disagreement):** {total - n_all}."]
    n_rev = sum(it["review_agree"] for it in items)
    lines += ["", f"**Review models ({', '.join(review)}) all agree:** {n_rev}/{total} ({n_rev / total:.0%}); "
              f"they disagree on {total - n_rev}."]
    (HERE / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))

    # Review queue: every disagreement among the review models, plus up to 50 random agreements as a spot check.
    rng = random.Random(0)
    agreed = [it for it in items if it["review_agree"]]
    spot = rng.sample(agreed, min(50, len(agreed)))
    queue = [it for it in items if not it["review_agree"]] + spot
    write_review(queue, manifest, review)


def write_review(queue: list[dict], manifest: list[dict], models: list[str]) -> None:
    pages = {t["tid"]: t["pages"] for t in manifest}
    meta = {t["tid"]: f"{t['year']} · {t['competition']} · {t['level']} · {t['test_type']}" for t in manifest}
    cards = []
    for i, it in enumerate(queue):
        kind = "models agree (spot check)" if it["review_agree"] else "models disagree"
        imgs = "".join(f'<img src="pages/{it["tid"]}/{p}" loading="lazy">' for p in pages[it["tid"]])
        opts = "".join(
            f'<label class="opt"><input type="radio" name="q{i}" value="{m}"><b>{m}</b>'
            f'<div class="math">{html.escape(it["texts"][m]) or "<i>(missing)</i>"}</div></label>'
            for m in models)
        cards.append(
            f'<section data-id="{it["tid"]}-{it["n"]}" data-kind="{"spot" if it["review_agree"] else "dis"}">'
            f'<h2>Test {it["tid"]} · Q{it["n"]} <small>{meta[it["tid"]]} · {kind}</small></h2>'
            f'<details><summary>Show page images</summary>{imgs}</details>'
            f'<div class="opts">{opts}<label class="opt"><input type="radio" name="q{i}" value="none">'
            f'<b>None are right</b></label></div>'
            f'<textarea placeholder="Corrected LaTeX (optional)"></textarea></section>')
    page = REVIEW_TEMPLATE.replace("{{CARDS}}", "\n".join(cards)).replace("{{COUNT}}", str(len(queue)))
    (HERE / "review.html").write_text(page)
    print(f"\nreview.html: {len(queue)} questions to review")


REVIEW_TEMPLATE = """<!doctype html><html><head><meta charset="utf-8"><title>Pilot review</title>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css">
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js"></script>
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/contrib/auto-render.min.js"></script>
<style>
body{font:15px/1.45 system-ui;margin:0 auto;max-width:1100px;padding:16px}
section{border:1px solid #ccc;border-radius:8px;padding:12px;margin:16px 0}
section.done{opacity:.55} h2 small{font-weight:normal;color:#666;font-size:13px}
details img{max-width:100%;border:1px solid #ddd;margin:4px 0}
.opts{display:grid;grid-template-columns:repeat(auto-fill,minmax(320px,1fr));gap:8px}
.opt{border:1px solid #ddd;border-radius:6px;padding:8px;cursor:pointer;display:block}
.opt:has(input:checked){border-color:#0a7;background:#0a71}
.math{white-space:pre-wrap;margin-top:4px} textarea{width:100%;min-height:48px;margin-top:8px}
header{position:sticky;top:0;background:#fff;padding:8px 0;border-bottom:1px solid #ccc;z-index:1}
</style></head><body>
<header><b>{{COUNT}} questions</b> · <span id="progress"></span> ·
<button onclick="exportJson()">Export decisions (JSON)</button></header>
<p>For each question, open the page images and pick the transcription that matches the page exactly
(math included). Pick "None are right" and type a fix if needed. Decisions save in this browser.</p>
{{CARDS}}
<script>
const KEY='pilot-review-v1';
let state={};try{state=JSON.parse(localStorage.getItem(KEY)||'{}')}catch(e){}
const save=()=>{try{localStorage.setItem(KEY,JSON.stringify(state))}catch(e){};progress()};
function progress(){const s=document.querySelectorAll('section');let d=0;s.forEach(x=>{const ok=!!(state[x.dataset.id]||{}).pick;x.classList.toggle('done',ok);d+=ok});
document.getElementById('progress').textContent=d+' / '+s.length+' reviewed'}
document.querySelectorAll('section').forEach(sec=>{const id=sec.dataset.id,st=state[id]||{};
 sec.querySelectorAll('input').forEach(r=>{if(r.value===st.pick)r.checked=true;
  r.onchange=()=>{state[id]={...(state[id]||{}),pick:r.value,kind:sec.dataset.kind};save()}});
 const ta=sec.querySelector('textarea');ta.value=st.fix||'';ta.oninput=()=>{state[id]={...(state[id]||{}),fix:ta.value,kind:sec.dataset.kind};save()}});
function exportJson(){const b=new Blob([JSON.stringify(state,null,2)],{type:'application/json'});
 const a=document.createElement('a');a.href=URL.createObjectURL(b);a.download='review-decisions.json';a.click()}
window.addEventListener('load',()=>{renderMathInElement(document.body,{delimiters:[
 {left:'$$',right:'$$',display:true},{left:'$',right:'$',display:false},{left:'\\\\(',right:'\\\\)',display:false},{left:'\\\\[',right:'\\\\]',display:true}],throwOnError:false});progress()});
</script></body></html>"""


if __name__ == "__main__":
    main()
