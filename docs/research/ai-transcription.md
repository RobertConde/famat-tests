# AI models for splitting Tests and transcribing questions to Typst/LaTeX

Research for [#11](https://github.com/RobertConde/famat-tests/issues/11), part of the map [#1](https://github.com/RobertConde/famat-tests/issues/1). Researched 2026-11-16. Terms follow `GLOSSARY.md` (Test, Document, Document kind, Original, Division/Level, Test type).

**Question.** Can AI models, preferably open-weight ones that can be self-hosted, reliably (1) split a Test's Documents into questions, answer choices, answers and solutions (*segmentation*), and (2) transcribe each question into accurate LaTeX or Typst (*transcription*)? What hardware do they need, and how should we measure accuracy on this archive?

## Short answer

- **Transcription is close to solved for clean printed math, but not proven on our material.** The best open ~1B document-parsing models now score 97–98 CDM on display formulas in OmniDocBench v1.6. That ties or beats the best closed model listed (Gemini 3 Pro, 95.99). Old scans are harder. On olmOCR-Bench's "Old Scans Math" split, the best open models score about 82–89. No public benchmark uses 1986–2022 typewritten or Word-Equation-Editor high-school contest tests, so we have to measure on our own Documents.
- **Segmentation has no public benchmark.** Treat it as two stages. First, a document parser produces Markdown, LaTeX and layout blocks. Then a general vision-language model fills a fixed JSON schema (number, stem, choices, figures, answer, solution). Deterministic checks catch most failures, for example "questions 1..N with no gaps" and "the answer key length equals the question count".
- **Store LaTeX as the canonical form and derive Typst from it.** No surveyed model reports Typst output. Mature open-source converters exist: texmath/pandoc writes Typst, tex2typst handles math, and MiTeX renders LaTeX math inside Typst.
- **Shortlist for the experiment:**
  - Open document parsers: **MinerU2.5-Pro**, **PaddleOCR-VL-1.6**, **GLM-OCR** and **olmOCR-2**.
  - Open general VLM for segmentation and end-to-end transcription: **Qwen3.6-27B**, plus Qwen3.6-35B-A3B as a cheaper MoE option.
  - Closed baseline: **Gemini 3.1 Pro**, with Claude Opus 5.5 as a second closed reference.
  - Optional: **Chandra OCR 2**. It has the best old-scan math score, but its weight license is restrictive.
- **Hardware.**
  - The ~1B parsers run on one NVIDIA GPU with 8–12 GB of VRAM, and MinerU and Marker also run on CPU or Apple Silicon.
  - Qwen3.6-27B needs a 24 GB GPU at 4-bit quantization (an estimate), or a 48–80 GB GPU for FP8 or BF16 with batching.
  - Everything listed has an official or documented container path that works under Podman through NVIDIA CDI.
- **Experiment.**
  - Sample 60 Tests, stratified by era, file format, Division/Level and Test type, with all their Documents (about 600 pages).
  - Label segmentation on all 60, and build gold LaTeX for 300 questions.
  - Score compile rate, normalized exact match, CDM and edit distance, a human-judged **critical-error rate**, and **human correction time** per question.
  - Cost: under $100 of API spend, about 1 GPU-day, and about 55–60 hours of human labeling and review over 2–3 weeks.

## 1. The archive, as it bears on this problem

From `doralmao_famat.sql` (counted 2026-11-16; this is the user's own dump):

- **3,695 Tests and 8,537 Documents.** Document kinds:
  - Test: 3,465
  - Solutions: 3,316
  - Answers: 956
  - Condensed Test: 581
  - Combined Test+Solutions: 95
  - Errata: 78
  - Other, blank or "-": 46
- **Tests by year bucket** (11 rows have year 0):

  | 1985–89 | 1990–94 | 1995–99 | 2000–04 | 2005–09 | 2010–14 | 2015–19 | 2020–22 |
  |---|---|---|---|---|---|---|---|
  | 63 | 379 | 527 | 584 | 668 | 668 | 579 | 216 |

- **Division/Level:**
  - Precalculus/Alpha: 758
  - Calculus/Mu: 728
  - Theta: 462
  - Geometry: 373
  - Algebra I: 359
  - Algebra II: 350
  - Open: 341
  - Statistics: 216
  - Unknown ("?"): 61
  - Named one-offs: Dostal 20, Nunn 17, Hiller 10
- **Test type:**
  - Individual: 1,136
  - Team: 1,110
  - About 1,450 others, mostly topic tests (Applications, Logs and Exponents, Ciphering, Gemini, and so on).

The SQL holds no file formats or page counts, because the Documents export is pending. Everything below that depends on them is an **estimate**:

- **Pages:** about 30k–60k. This assumes a Test of 4–8 pages, Solutions of 4–10 pages, and Answers of 1 page.
- **Questions:** about 80k–100k. This assumes about 30 per Individual test, about 15 per Team test, and about 25 per topic test.

Two features of the archive matter for the AI work. Individual and topic tests are mostly multiple choice. Team and Ciphering rounds are often free response, so the segmentation schema must allow a question with no choices. Answers and Solutions are separate Documents in most Tests, so segmentation has to **link** the answer and solution to each question across Documents.

Format handling should branch on the Original:

| Original | Best path | Why |
|---|---|---|
| `.docx` with native Word equations (OMML) | pandoc / texmath: OMML → LaTeX (and Typst) directly, no OCR | texmath reads OMML and writes TeX and Typst ([texmath README](https://github.com/jgm/texmath)) |
| `.doc`, or `.docx` whose equations are Equation Editor 3.0 / MathType OLE objects | Render to PDF (e.g. LibreOffice headless), then the OCR path | OLE equation objects are pictures as far as converters are concerned (assumption to confirm on the sample) |
| Born-digital PDF | OCR/VLM path. Use the text layer only as a cross-check. | PDF text layers lose math structure |
| Scanned PDF | OCR/VLM path | — |

## 2. Benchmarks that matter

### OmniDocBench (end-to-end document parsing, includes an exam-paper subset)

[OmniDocBench](https://github.com/opendatalab/OmniDocBench) was published at CVPR 2025 by Ouyang et al. ([arXiv 2412.07626](https://arxiv.org/abs/2412.07626)). It has 981 annotated pages across nine document types, including **114 "Exam Paper" pages** (paper Table S3). The current leaderboard is v1.6. Its overall score is ((1 − text edit) × 100 + table TEDS + formula CDM) / 3, and the leaderboard was last updated 2026-11-12. Selected rows, copied from the repo README on 2026-11-16:

| Model | Type | Size | Overall ↑ | Text edit ↓ | Formula CDM ↑ | Table TEDS ↑ |
|---|---|---|---|---|---|---|
| MiniCPM-OCR 1.0 | specialized | 1.3B | 96.44 | 0.032 | **98.08** | 94.40 |
| PaddleOCR-VL-1.6 | specialized | 0.9B | 96.34 | 0.033 | 97.53 | 94.76 |
| MinerU2.5-Pro | specialized | 1.2B | 95.75 | 0.036 | 97.45 | 93.42 |
| GLM-OCR | specialized | 0.9B | 95.22 | 0.044 | 97.18 | 92.83 |
| Gemini 3 Pro | closed general | — | 92.91 | 0.064 | 95.99 | 89.15 |
| Gemini 3 Flash | closed general | — | 92.62 | 0.066 | 95.16 | 89.29 |
| dots.ocr | specialized | 3B | 90.77 | 0.048 | 89.95 | 87.18 |
| Qwen3-VL-235B | open general | 235B | 89.78 | 0.063 | 92.55 | 83.07 |
| GPT-5.2 | closed general | — | 86.59 | 0.114 | 88.21 | 82.95 |
| MinerU (classic pipeline) | pipeline | — | 86.47 | 0.055 | 83.07 | 81.88 |
| olmOCR | specialized | 7B | 85.74 | 0.139 | 88.10 | 83.00 |
| Mistral OCR | closed specialized | — | 85.66 | 0.097 | 89.91 | 76.78 |
| Marker | pipeline | — | 78.44 | 0.157 | 85.24 | 65.77 |

The top of the board (OXR-1.0, TeleOCR, OvisOCR2) is crowded with models released in the last few months that have little deployment history. We prefer the established families that sit within about 1 point of them.

Newer Qwen models report OmniDocBench **v1.5**, which is not directly comparable to v1.6. From the [Qwen3.6-35B-A3B model card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B):

| Model | OmniDocBench v1.5 |
|---|---|
| Qwen3.6-35B-A3B | 89.9 |
| Qwen3.5-35B-A3B | 89.3 |
| Qwen3.5-27B | 88.9 |
| Claude Sonnet 4.5 | 85.8 |
| Gemma4-31B | 80.1 |

GLM-OCR reports 94.62 on v1.5 ([model card](https://huggingface.co/zai-org/GLM-OCR)).

The exam-paper column of the original paper's Table 3 (text edit distance, lower is better) uses 2024-era models. It is still useful as a warning:

| Model | Exam-paper edit distance |
|---|---|
| MinerU (pipeline) | 0.159 |
| Qwen2-VL-72B | 0.195 |
| GOT-OCR | 0.204 |
| InternVL2-76B | 0.247 |
| Mathpix | 0.278 |
| GPT-4o | 0.281 |
| Marker | 0.452 |
| **Nougat** | **0.930** |

Nougat collapses on anything that isn't an arXiv paper.

Caveat: many OmniDocBench pages are Chinese, and its exam papers are not US contest tests.

### olmOCR-Bench (unit tests, includes old scanned math)

[olmOCR-Bench](https://github.com/allenai/olmocr/tree/main/olmocr/bench) has more than 7,000 pass/fail tests over 1,400 documents. Its **"Old Scans Math"** split comes from random pages of old public-domain math textbooks on the Internet Archive. Annotators marked each formula. Each reference formula is rendered with KaTeX, and the test passes if the output contains it, checked by the relative positions of symbols. Equivalent LaTeX spellings therefore pass. This is the closest public proxy for our 1980s–90s scans.

| Model | arXiv math | Old Scans Math | Overall |
|---|---|---|---|
| Chandra OCR 2 ([Datalab README](https://github.com/datalab-to/chandra), self-reported) | — | **89.1** | 85.8 |
| Infinity-Parser 7B | 84.4 | 83.8 | 82.5 |
| olmOCR-2-7B-1025 ([model card](https://huggingface.co/allenai/olmOCR-2-7B-1025)) | 83.0 | 82.1–82.3 | 82.3–82.4 |
| DeepSeek-OCR | 77.2 | 73.6 | 75.7 |
| PaddleOCR-VL (v1.0) | 85.7 | 71.0 | 80.0 |
| GLM-OCR ([model card](https://huggingface.co/zai-org/GLM-OCR), ZAI API) | 80.7 | 68.3 | 75.2 |
| Mistral OCR API | 77.2 | 67.5 | 72.0 |
| Marker 1.10.1 | 83.8 | 66.8 | 76.1 |
| MinerU 2.5.4 | 76.6 | 54.6 | 75.2 |

Rows without a citation come from the [olmOCR README](https://github.com/allenai/olmocr). That README's column headers were garbled when extracted, so we mapped columns by order; check against the README before quoting.

These results warn that **OmniDocBench rank does not carry over to old scans**. MinerU2.5 and PaddleOCR-VL lead OmniDocBench but trail olmOCR-2 and Chandra on Old Scans Math. That is the main reason the experiment includes both families.

### Formula recognition (isolated formulas)

- **UniMER-Test** ([UniMERNet repo](https://github.com/opendatalab/UniMERNet), arXiv 2404.15254) has 23,757 formula images in four subsets:
  - SPE, simple printed: 6,762
  - CPE, complex printed: 5,921
  - SCE, screen capture: 4,742
  - HWE, handwritten: 6,332
- **CDM (Character Detection Matching)**, by Wang et al. ([arXiv 2409.03643](https://arxiv.org/abs/2409.03643), CVPR 2025), is the metric to use. It renders the predicted and reference LaTeX, matches characters by visual features and position, and scores the images. Equivalent LaTeX spellings therefore score as equal, which BLEU and edit distance get wrong. The authors report that CDM agrees with human judgment better than BLEU or edit distance. CDM's open implementation is the "symbol error rate on rendered output" the ticket asks for.

## 3. Candidates

### Dedicated document/math OCR (stage 1: page → Markdown + LaTeX + layout)

| Model | Size / license | Strengths | Hardware & throughput (as published) |
|---|---|---|---|
| **MinerU2.5-Pro** ([card](https://huggingface.co/opendatalab/MinerU2.5-Pro-2604-1.2B), [repo](https://github.com/opendatalab/MinerU)) | ~1.2B; model Apache-2.0; the MinerU tool uses the "MinerU Open Source License" (Apache-2.0 plus conditions) | OmniDocBench v1.6: 95.75 overall, CDM 97.45. Real5-OmniDocBench "Scanning" 92.11. The tool reads PDF, images, DOC/DOCX and more, and outputs Markdown, LaTeX, JSON layout blocks and more. | 8 GB+ VRAM (Volta or newer) for the GPU tiers. A CPU-only ONNX/llama.cpp tier and MLX on Apple Silicon. About 2.12 pages/s on one A100 with vLLM async (card). Docker images are provided. |
| **PaddleOCR-VL-1.6** ([card](https://huggingface.co/PaddlePaddle/PaddleOCR-VL-1.6)) | 0.9B; Apache-2.0; released 2026-05-28 | OmniDocBench v1.6: 96.34 overall, CDM 97.53. Its own card lists olmOCR-Bench Old Scans 38.6. | vLLM server or PaddleOCR Docker image. No VRAM figure is published; at 0.9B, expect it to fit in 8 GB. |
| **GLM-OCR** ([card](https://huggingface.co/zai-org/GLM-OCR)) | 0.9B; MIT (layout module PP-DocLayoutV3 is Apache-2.0) | OmniDocBench v1.6: 95.22, CDM 97.18 | vLLM, SGLang, Ollama. 1.86 pages/s with a single replica at single concurrency. |
| **olmOCR-2-7B-1025** ([card](https://huggingface.co/allenai/olmOCR-2-7B-1025), [repo](https://github.com/allenai/olmocr)) | ~8B (fine-tuned Qwen2.5-VL-7B with GRPO RL aimed at equations); weights and toolkit Apache-2.0 | Old Scans Math about 82, overall about 82.4 on olmOCR-Bench. Weaker on OmniDocBench (85.74). | NVIDIA GPU with ≥12 GB (tested on RTX 4090, L40S, A100, H100) and 30 GB of disk. The authors quote "<$200 per million pages". |
| **Chandra OCR 2** ([repo](https://github.com/datalab-to/chandra)) | Code Apache-2.0. Weights use a modified OpenRAIL-M: "free for research, personal use, and startups under $2M funding/revenue". Commercial use beyond that needs a license, and the weights may not be used competitively with Datalab's API. | Best published Old Scans Math (89.1), handwriting | 1.44 pages/s on one H100 80GB at 96 concurrent sequences |
| Marker ([repo](https://github.com/datalab-to/marker)) | Code Apache-2.0; weights modified OpenRAIL-M (under $5M) | Fast (4.5–57 pages/s depending on mode). JSON block tree with polygons. Optional `--use_llm` correction pass. | Runs on CPU, MPS or GPU. OmniDocBench 78.44. Not shortlisted. |
| Nougat ([repo](https://github.com/facebookresearch/nougat)) | Weights CC-BY-NC | Trained on arXiv and PMC | **Not recommended.** It scored 0.930 edit distance on OmniDocBench exam papers, and its weights are non-commercial. |
| GOT-OCR 2.0 | — | 0.204 on exam papers in 2024 | **Superseded** by the models above |

### Open general vision-language models (stage 2: segmentation and structuring; also end-to-end transcription)

| Model | Size / license | Evidence | Hardware |
|---|---|---|---|
| **Qwen3.6-27B** ([card](https://huggingface.co/Qwen/Qwen3.6-27B)) | 27B dense; Apache-2.0; April 2026; native vision | CC-OCR 81.2 and OCRBench 89.4, against Claude 4.5 Opus at CC-OCR 76.9 and Gemma4-31B at 75.7 and 86.1. 262k context, so a whole Test plus its Answers and Solutions fits in one request. | vLLM ≥0.19 or SGLang ≥0.5.10, with the official `vllm/vllm-openai` image. Weight memory is roughly params × bytes (our estimate): BF16 about 54 GB, FP8 about 27 GB, 4-bit about 14–16 GB plus KV cache. 4-bit therefore fits a 24 GB card (RTX 3090/4090) with modest context, and FP8 fits a 48 GB card. |
| **Qwen3.6-35B-A3B** ([card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B)) | 35B MoE with 3B active; Apache-2.0 | OmniDocBench v1.5 89.9, CC-OCR 81.9 | 4-bit about 18–20 GB (estimate). It runs much faster than the 27B dense model per token, and is usable on Apple Silicon or CPU offload via llama.cpp or KTransformers. |
| Qwen3-VL family ([repo](https://github.com/QwenLM/Qwen3-VL)) | 2B to 235B-A22B; Apache-2.0 | 235B scores 89.78 on OmniDocBench v1.6. Supports grounding with bounding boxes, which is useful for cropping question regions and figures. | 235B FP8 is shown at tensor-parallel 8 on H100/H200, which is out of reach for self-hosting. Superseded by Qwen3.6 for our use. |
| Gemma 4 (31B, 26B-A4B) | — | OmniDocBench v1.5 80.1 and 74.4 (Qwen card's comparison) | Not shortlisted: weaker on documents |

### Closed baseline

| Model | Evidence | Price (batch, per 1M tokens) | Page cost |
|---|---|---|---|
| **Gemini 3.1 Pro** (newest Pro; [pricing](https://aistudio.google.com/docs/pricing)) | Gemini 3 Pro is the best closed model on OmniDocBench v1.6 (92.91, CDM 95.99) | $1 in / $6 out (≤200k context) | A PDF page costs 560 tokens at the default "medium" media resolution. Google says OCR quality "typically saturates at medium" ([media resolution docs](https://aistudio.google.com/docs/media-resolution)). |
| Claude Opus 5.5 ([pricing](https://platform.claude.com/docs/en/about-claude/pricing), [vision](https://platform.claude.com/docs/en/build-with-claude/vision)) | No OmniDocBench row. Claude Sonnet 4.5 scores 85.8 on v1.5 and Claude 4.5 Opus 76.9 on CC-OCR (from Qwen's cards). | $2 in / $10 out | High-resolution tier: up to 2,576 px on the long edge and 4,784 visual tokens. A 200-dpi letter page is about 4,800 tokens. |

**Per-page cost estimate** (assumes about 1k prompt tokens and about 3k output tokens including reasoning):

- Gemini 3.1 Pro batch: about **$0.02/page**
- Claude Opus 5.5 batch: about **$0.04/page**

For the whole archive (30k–60k pages, estimated), that is about $600–1,200 for Gemini and about $1,200–2,400 for Opus. The experiment's ~600 pages cost $12–25 per model per run.

## 4. Segmentation and transcription pipeline (what the experiment tests)

1. **Ingest.** Route each Original by format, as in the table in §1. Render PDFs at 200–300 dpi.
2. **Parse each page** with a stage-1 parser to get Markdown with `$…$` and `$$…$$` LaTeX, plus layout blocks with bounding boxes and reading order. Figures are cropped as images. Transcribing geometry diagrams into TikZ or CeTZ is out of scope.
3. **Segment.** Give stage 2 (Qwen3.6-27B, or the closed baseline) the parsed Markdown and page images for the whole Test with its Answers and Solutions Documents. It fills a strict JSON schema: `{number, stem_latex, choices: {A..E} | null, figure_crops[], answer, solution_latex, source_pages[]}`. Use constrained or JSON-schema decoding, which vLLM supports.
4. **Validate deterministically.**
   - Question numbers are contiguous from 1.
   - Choice labels are complete.
   - The answer-key count equals the question count, and each answer letter is one of the choices.
   - Every block on the page is assigned to exactly one question or to the header or instructions.
   - Each `stem_latex` compiles.
   - Any failure goes to the human queue.
5. **Run two independent transcribers and compare.** Run two different stage-1 models, for example MinerU2.5-Pro and olmOCR-2. If their normalized LaTeX for a question differs, or its CDM against the other output is below a threshold, flag the question for review. Questions where the two agree go to spot-checks only. The experiment measures how much review time this saves.
6. **Produce Typst.** Keep LaTeX canonical and generate Typst with one of:
   - pandoc's `typst` writer (math via texmath) ([pandoc manual](https://pandoc.org/MANUAL.html), [texmath](https://github.com/jgm/texmath))
   - [tex2typst](https://github.com/qwinsi/tex2typst) (Apache-2.0, math only)
   - [MiTeX](https://github.com/mitex-rs/mitex) (renders LaTeX math inside Typst via `#mitex`/`#mi`, Apache-2.0)

   The experiment records the Typst compile rate for each route.

## 5. Proposed accuracy experiment

### 5.1 Ground-truth sample

**Unit.** A Test together with all its Documents (Test or Condensed Test, Answers, Solutions), because segmentation has to link across Documents.

**Size.** 60 Tests, about 600 pages, about 1,500 questions in total.

- Segmentation labels cover all 60 Tests.
- Gold transcriptions cover **300 questions**, 5 drawn at random from each Test.
- With 300 items, a 95% interval on a rate near 90% is about ±3.4 points. Every model runs on the same items, so paired tests (McNemar on critical-error rates, Wilcoxon on CDM) are far more sensitive than that interval suggests.

**Strata** (finalize after the Documents export gives file formats):

| Stratum | Levels | Allocation |
|---|---|---|
| Era | 1986–94, 1995–2004, 2005–14, 2015–22 | 15 Tests each. This oversamples the early era (12% of Tests) because old scans are the hard case. |
| Original format | scanned PDF, born-digital PDF, `.doc`/OLE equations, `.docx`/OMML | At least 8 Tests per format that exists. Fill the rest proportionally within each era. |
| Division/Level | Algebra I, Geometry, Algebra II, Precalculus/Alpha, Calculus/Mu, Statistics, Theta, Open | At least 5 Tests each, roughly in proportion to the counts in §1. Geometry is guaranteed for figures and Statistics for tables. |
| Test type | Individual (multiple choice), Team, Ciphering/other free-response, topic | At least 25 Individual, 15 Team, 5 free-response other, and the rest topic |
| Document kinds present | separate Answers/Solutions vs Combined Test+Solutions | At least 5 Combined |

Draw the sample with a seeded random selection inside each cell and record the seed. Keep 10 Tests aside as a **held-out set** that nobody looks at while tuning prompts.

**Building the gold** (this also serves as the correction-time study):

1. For each of the 300 questions, a randomly assigned system's output is the starting draft. The reviewer is **blind** to which system produced it. The reviewer corrects it to exact gold against the Original, and the review tool logs active editing time.
2. A second reviewer verifies every gold item. A 20% overlap is corrected independently by both reviewers to measure agreement (Cohen's κ on the error categories).
3. For 30 questions, a reviewer transcribes **from scratch**, with no draft. This gives the human-only time baseline and checks whether drafts anchor reviewers on their errors.
4. Gold conventions are written down before labeling starts:
   - `\frac` rather than `\dfrac` when display style isn't meaningful
   - choices stored without the "(A)" labels
   - "NOTA" and "None of these" kept verbatim
   - units in `\text{}`
   - a fixed preamble with amsmath, amssymb and siunitx

**Segmentation labels for all 60 Tests.** For each question, record the page, the first and last block, the choice spans, the answer letter or value, the solution span, and figure boxes. This takes about 10–15 minutes per Test with a simple tool.

### 5.2 Systems compared

| ID | Stage 1 (transcription) | Stage 2 (segmentation) |
|---|---|---|
| S1 | MinerU2.5-Pro | Qwen3.6-27B |
| S2 | PaddleOCR-VL-1.6 | Qwen3.6-27B |
| S3 | GLM-OCR | Qwen3.6-27B |
| S4 | olmOCR-2-7B | Qwen3.6-27B |
| S5 | Qwen3.6-27B end-to-end (images in, JSON out) | same model |
| S6 | Gemini 3.1 Pro end-to-end | same model |
| S7 (optional) | Claude Opus 5.5 end-to-end, and/or Chandra OCR 2 + Qwen3.6-27B | — |
| S8 | Ensemble: best two stage-1 models with disagreement flagging (§4 step 5) | Qwen3.6-27B |

Every system gets the same prompts, temperature 0 where the model allows it, and the same page rasters. Run each system twice to measure run-to-run variance; this matters for the closed models.

### 5.3 Metrics

**Segmentation** (per Test, then averaged):

- **Question-count exact match:** the share of Tests where the number of questions found equals the true number.
- **Boundary F1:** precision and recall of question spans at block level, where a span counts as correct if its IoU with the true span is at least 0.9.
- **Choice accuracy:** the share of multiple-choice questions where all choice labels are present and each choice is attached to the right question.
- **Answer linkage accuracy:** the share of questions whose answer, from the Answers or Solutions Document, is correct and attached to the right question.
- **Solution linkage accuracy:** the same, for solution spans.
- **Validator catch rate:** the share of segmentation errors that §4 step 4 flags. This decides whether unflagged output can skip review.

**Transcription** (per question, against the 300 gold items):

1. **Compiles.** The share of outputs that compile in a fixed LaTeX template (e.g. Tectonic or TeX Live). Also record the share that compile in Typst through each route in §4 step 6.
2. **Normalized exact match.** Parse both sides with KaTeX or texmath, then normalize whitespace, `\left`/`\right`, `\dfrac`→`\frac` and similar, and redundant braces. Score the full question and each math span separately.
3. **CDM.** Use the official implementation, applied to each math span and to the whole rendered question. Report the mean and the share of questions at CDM = 1 (the CDM ExpRate).
4. **Text edit distance.** Normalized Levenshtein distance on the non-math text, as in OmniDocBench.
5. **Critical-error rate (primary metric).** The share of questions with at least one error that changes the mathematics or the answer, such as a wrong digit, sign, exponent, variable, operator, missing condition, or swapped choice. The reviewer labels each error as *critical*, *cosmetic* or *formatting-only*. A question bank can tolerate cosmetic errors but not critical ones.
6. **Human correction time.** Median and 90th-percentile active seconds per question to reach gold, for each system and compared with transcribing from scratch. This turns accuracy into cost.
7. **Figure handling.** The share of figure-bearing questions whose figure is detected and cropped intact.

**Decision rule** (proposed; pick the thresholds before running):

- A system is "good enough to draft" if its critical-error rate is at most 5% **and** its median correction time is under 40% of from-scratch time.
- The ensemble's agreement filter is "good enough to skip review" if questions where both models agree have a critical-error rate of at most 1%, measured on at least 100 agreeing questions.
- Report every number by stratum, especially era × format. The likely outcome is a split policy: born-digital Tests go through with spot-checks, and pre-2000 scans always get reviewed.

### 5.4 Review workflow

- A three-pane review page: the Original crop, the rendered output (KaTeX or Typst preview), and the editable LaTeX source. Add buttons for **Accept / Fix-cosmetic / Fix-critical / Unusable** and an error-category picker.
- Use a self-hosted labeling tool (e.g. Label Studio) or a small custom page in the future question-bank app. It must log active time per item and hide the system's identity.
- Present items in random order so that no reviewer sees the same question twice. Two or more reviewers, ideally math teachers, split the work. One of them writes and owns the gold conventions.

### 5.5 Cost and time estimate (rough)

| Item | Estimate |
|---|---|
| Closed-model API: about 600 pages × 2 runs × (Gemini 3.1 Pro ~$0.02 + Opus 5.5 ~$0.04 per page, batch) | **about $75** |
| Self-hosted compute | **About 1 GPU-day.** The ~1B models process 600 pages in minutes on an 8–24 GB GPU. Qwen3.6-27B segmentation and end-to-end runs take a few hours on a 48–80 GB GPU, or overnight on a 24 GB card at 4-bit. Without owned hardware, rent one 80 GB GPU for a day (check current rates). |
| Segmentation labeling, 60 Tests × 10–15 min | 10–15 h |
| Gold correction, 300 questions × ~5 min (also the timing data) | ~25 h |
| Second-reviewer verification, plus 20% independent overlap | ~12 h |
| From-scratch baseline, 30 questions × ~10 min | ~5 h |
| Harness: scripts for metrics, CDM and compile checks | ~1–2 days of engineering, or agent time |
| **Total** | **Under $100 of API spend, about 1 GPU-day, about 55–60 human hours, 2–3 calendar weeks part-time** |

**Full-archive projection** (only to frame the decision; the experiment refines it):

- Stage 1 over 30k–60k pages at about 2 pages/s on one A100-class GPU: about 4–8 GPU-hours.
- Gemini 3.1 Pro end-to-end: about $600–1,200.
- The dominant cost is **human review** of about 80k–100k questions. At 30 s per agreeing question (spot-check) and 3 min per flagged one, a 20% flag rate is about 1,100–1,400 hours. The experiment's main job is to pin down the flag rate and the per-item times.

## 6. Hardware summary for a Podman host

| Tier | Example hardware | What runs |
|---|---|---|
| CPU / Apple Silicon | Any modern x86, or an M-series Mac with 32–64 GB | MinerU ONNX/llama.cpp/MLX tiers and Marker. Quantized Qwen3.6-35B-A3B via llama.cpp, slowly. Fine for trying prompts, too slow for the archive. |
| Single 8–12 GB NVIDIA GPU | RTX 3060 12 GB / 4070 | MinerU2.5-Pro, PaddleOCR-VL-1.6, GLM-OCR, and olmOCR-2 at 12 GB+ |
| Single 24 GB GPU | RTX 3090 / 4090 | All stage-1 models, plus Qwen3.6-27B or 35B-A3B at 4-bit with modest context (estimate) |
| 48–80 GB GPU | RTX 6000 Ada / L40S / A100 / H100 | Qwen3.6-27B at FP8 or BF16 with batching and long context. The most practical single-box setup for the full archive. |

**Containers.** vLLM ships the `vllm/vllm-openai` image, and its docs give a Podman variant using `--device nvidia.com/gpu=all` ([vLLM Docker docs](https://docs.vllm.ai/en/latest/deployment/docker.html)). NVIDIA GPU passthrough under Podman uses CDI. Generate the spec with `sudo nvidia-ctk cdi generate --output=/var/run/cdi/nvidia.yaml`; toolkit v1.18+ refreshes it automatically. Then run with `--device nvidia.com/gpu=all --security-opt=label=disable` ([NVIDIA CDI docs](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/cdi-support.html)). MinerU and PaddleOCR-VL also publish Docker images. GPU passthrough into Podman's Linux VM on macOS is not covered by these docs, so on a Mac run the models natively.

## 7. Open points and caveats

- **File formats are unknown** until the Documents export lands. The format strata and the `.doc` OLE-equation assumption must be checked against it.
- **Benchmark numbers are self-reported or leaderboard-reported, and the field moves monthly.** The OmniDocBench leaderboard changed on 2026-11-12. Re-check the shortlist when the experiment starts; the harness makes adding a model cheap.
- **No benchmark covers our genre:** 1986–2022 US high-school contest tests, typewritten or Word-equation pages, multiple choice with separate answer keys. That is why the experiment exists.
- **Typst:** no model is benchmarked on Typst output. Conversion from LaTeX is the recommended route, and the experiment measures its compile rate.
- **Licenses:** the Chandra and Marker weights carry revenue-threshold restrictions. FAMAT likely qualifies for free use as a nonprofit or under the threshold, but confirm before relying on them. Nougat's weights are non-commercial.
- **The v1 data model:** keep each Original immutable and addressable (already settled in #1). Then a later question-bank table can point at (Document, page, bounding box) for every question's source crop, which the review workflow needs.

## Sources

- OmniDocBench leaderboard (v1.6): https://github.com/opendatalab/OmniDocBench
- OmniDocBench paper (CVPR 2025), Tables 3 and S3: https://arxiv.org/abs/2412.07626, https://arxiv.org/html/2412.07626
- olmOCR, olmOCR-Bench, and olmOCR-2 model card: https://github.com/allenai/olmocr, https://github.com/allenai/olmocr/tree/main/olmocr/bench, https://huggingface.co/allenai/olmOCR-2-7B-1025
- MinerU, and the MinerU2.5-Pro card: https://github.com/opendatalab/MinerU, https://huggingface.co/opendatalab/MinerU2.5-Pro-2604-1.2B
- PaddleOCR-VL-1.6: https://huggingface.co/PaddlePaddle/PaddleOCR-VL-1.6
- GLM-OCR: https://huggingface.co/zai-org/GLM-OCR
- Chandra OCR 2: https://github.com/datalab-to/chandra
- Marker: https://github.com/datalab-to/marker
- Nougat: https://github.com/facebookresearch/nougat, https://arxiv.org/abs/2308.13418
- Qwen3-VL: https://github.com/QwenLM/Qwen3-VL
- Qwen3.6-27B: https://huggingface.co/Qwen/Qwen3.6-27B
- Qwen3.6-35B-A3B: https://huggingface.co/Qwen/Qwen3.6-35B-A3B
- UniMERNet / UniMER-Test: https://github.com/opendatalab/UniMERNet
- CDM: https://arxiv.org/abs/2409.03643
- Gemini pricing and media resolution: https://aistudio.google.com/docs/pricing, https://aistudio.google.com/docs/media-resolution
- Claude pricing and vision: https://platform.claude.com/docs/en/about-claude/pricing, https://platform.claude.com/docs/en/build-with-claude/vision
- pandoc: https://pandoc.org/MANUAL.html
- texmath: https://github.com/jgm/texmath
- tex2typst: https://github.com/qwinsi/tex2typst
- MiTeX: https://github.com/mitex-rs/mitex
- vLLM Docker/Podman: https://docs.vllm.ai/en/latest/deployment/docker.html
- NVIDIA CDI for Podman: https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/cdi-support.html
