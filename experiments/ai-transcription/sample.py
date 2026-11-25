"""Pick a stratified pilot sample of Test documents and render their pages to PNG.

Usage: uv run sample.py --docs <docs dir> --sql <dump.sql> [--seed 7]
Writes pages/<tid>/p<N>.png and pages/manifest.json.
"""

import argparse
import json
import random
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
PAGES = HERE / "pages"

ROW = re.compile(
    r"\((\d+), '((?:[^'\\]|\\.)*)', '((?:[^'\\]|\\.)*)', '((?:[^'\\]|\\.)*)', (\d+), "
    r"'((?:[^'\\]|\\.)*)', '[YN]', '((?:[^'\\]|\\.)*)'\)"
)
DOC = re.compile(r'\\"tid\\": (\d+), \\"doc_title\\": \\"([^\\]*)\\"')

# (era label, year range, how many Tests, allowed extensions)
STRATA = [
    ("pre-2000 scans", range(1986, 2000), 4, {".pdf"}),
    ("2000s pdf", range(2000, 2010), 2, {".pdf"}),
    ("2000s word", range(2000, 2010), 1, {".doc"}),
    ("2010s pdf", range(2010, 2023), 2, {".pdf"}),
    ("2010s word", range(2010, 2023), 1, {".docx"}),
]
MAX_PAGES = 8


def load_tests(sql: Path) -> list[dict]:
    text = sql.read_text(encoding="utf8")
    tests = []
    for m in ROW.finditer(text):
        test_id, competition, level, test_type, year, host, docs = m.groups()
        for tid, title in DOC.findall(docs):
            if title == "Test":
                tests.append(dict(test_id=int(test_id), competition=competition, level=level,
                                  test_type=test_type, year=int(year), host=host, tid=int(tid)))
    return tests


def page_count(pdf: Path) -> int:
    out = subprocess.run(["pdfinfo", str(pdf)], capture_output=True, text=True).stdout
    return int(re.search(r"Pages:\s+(\d+)", out)[1])


def to_pdf(src: Path, work: Path) -> Path:
    if src.suffix.lower() == ".pdf":
        return src
    profile = work / "lo-profile"
    subprocess.run(["soffice", f"-env:UserInstallation=file://{profile}", "--headless",
                    "--convert-to", "pdf", "--outdir", str(work), str(src)],
                   check=True, capture_output=True, timeout=180)
    return work / (src.stem + ".pdf")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--docs", type=Path, required=True)
    ap.add_argument("--sql", type=Path, required=True)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()

    files = {int(f.name.split(" ", 1)[0]): f for f in args.docs.iterdir() if re.match(r"^\d+ ", f.name)}
    tests = [t for t in load_tests(args.sql) if t["tid"] in files]
    rng = random.Random(args.seed)
    if PAGES.exists():
        shutil.rmtree(PAGES)
    PAGES.mkdir()

    manifest, used_levels = [], set()
    for label, years, want, exts in STRATA:
        pool = [t for t in tests if t["year"] in years and files[t["tid"]].suffix.lower() in exts]
        rng.shuffle(pool)
        # Prefer Division/Levels not yet in the sample, so the pilot spans them.
        pool.sort(key=lambda t: t["level"] in used_levels)
        picked = 0
        for t in pool:
            if picked == want:
                break
            with tempfile.TemporaryDirectory() as tmp:
                try:
                    pdf = to_pdf(files[t["tid"]], Path(tmp))
                    n = page_count(pdf)
                except (subprocess.SubprocessError, TypeError, FileNotFoundError):
                    continue
                if n > MAX_PAGES:
                    continue
                out = PAGES / str(t["tid"])
                out.mkdir()
                subprocess.run(["pdftoppm", "-r", "150", "-png", str(pdf), str(out / "p")], check=True)
            pages = sorted(p.name for p in out.glob("*.png"))
            manifest.append({**t, "stratum": label, "file": files[t["tid"]].name, "pages": pages})
            used_levels.add(t["level"])
            picked += 1
            print(f"{label:15} {t['year']} {t['level']:20} {t['test_type']:15} {len(pages)}p  {files[t['tid']].name}")

    (PAGES / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"{len(manifest)} Tests, {sum(len(m['pages']) for m in manifest)} pages")


if __name__ == "__main__":
    main()
