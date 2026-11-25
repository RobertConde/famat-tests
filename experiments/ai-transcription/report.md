# Pilot: open-model agreement on real Test pages

10 Tests, 50 pages, 248 questions (question count per Test = the most common count across models). Agreement = normalized similarity >= 0.97.

## Per model

| Model | Questions split 1..N as expected | Sec/page (median) | Peak memory GB (max) | Agrees with majority |
|---|---|---|---|---|
| glm-ocr | 10/10 Tests | 10.1 | 3.05 | 218/248 (88%) |
| mineru-2.5-pro | 9/10 Tests | 13.6 | 3.21 | 189/248 (76%) |
| nanonets-ocr2-3b-8bit | 9/10 Tests | 23.1 | 5.64 | 148/248 (60%) |
| olmocr-2-7b-6bit | 9/10 Tests | 41.1 | 8.57 | 231/248 (93%) |
| olmocr-2-7b-8bit | 9/10 Tests | 50.5 | 10.48 | 229/248 (92%) |
| paddleocr-vl-1.5 | 10/10 Tests | 6.7 | 2.72 | 189/248 (76%) |
| qwen3-vl-8b-8bit | 9/10 Tests | 54.8 | 11.05 | 199/248 (80%) |

## Pairwise agreement (share of questions)

| | glm-ocr | mineru-2.5-pro | nanonets-ocr2-3b-8bit | olmocr-2-7b-6bit | olmocr-2-7b-8bit | paddleocr-vl-1.5 | qwen3-vl-8b-8bit |
|---|---|---|---|---|---|---|---|
| glm-ocr | — | 70% | 53% | 83% | 82% | 73% | 74% |
| mineru-2.5-pro | 70% | — | 61% | 71% | 70% | 64% | 67% |
| nanonets-ocr2-3b-8bit | 53% | 61% | — | 56% | 56% | 49% | 48% |
| olmocr-2-7b-6bit | 83% | 71% | 56% | — | 98% | 74% | 81% |
| olmocr-2-7b-8bit | 82% | 70% | 56% | 98% | — | 73% | 80% |
| paddleocr-vl-1.5 | 73% | 64% | 49% | 74% | 73% | — | 71% |
| qwen3-vl-8b-8bit | 74% | 67% | 48% | 81% | 80% | 71% | — |

**All models agree:** 91/248 questions (37%). **Need review (any disagreement):** 157.

**Review models (olmocr-2-7b-6bit, qwen3-vl-8b-8bit) all agree:** 202/248 (81%); they disagree on 46.
