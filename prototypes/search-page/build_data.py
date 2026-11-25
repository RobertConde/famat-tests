"""PROTOTYPE (throwaway): build catalogue.json for the search-page prototype.

Groups Documents into Tests by identical labels, the way the migration will,
using the students' sheet + AI labels + unpacked bundles. Approximate, not the
real import. Usage: python3 -I build_data.py <sheet.csv> <sql dump> <docs dir> <out.json>
"""
import csv, json, os, re, sys, collections

sheet_csv, sql, docs_dir, out = sys.argv[1:]
L = "data/labels/"
COLS = ["File Type", "Year", "Comp Type", "Comp Month", "Division", "Test", "Host"]
COMP = {"Invitationals": "Invitational", "Regionals": "Regional", "States": "State", "Nationals": "National",
        "Nationalls": "National", "Mail-in": "Mail-in"}
MONTHLESS = {"State", "National", "Mail-in"}

def clean(c, v):
    v = (v or "").strip()
    if v in ("", "-", "?", "0"): return None
    if c == "Comp Type": return COMP.get(v, v)
    if c == "Test": return v.strip("()")
    return v

names = {int(f.split()[0]): f for f in os.listdir(docs_dir) if re.match(r"^\d+ ", f)}
ext_of = lambda f: os.path.splitext(f)[1].lower().lstrip(".") or "pdf"

docs = {}  # tid -> list of label dicts (one per Test it covers)
for r in list(csv.reader(open(sheet_csv, encoding="utf-8-sig", newline="")))[2:]:
    r = (r + [""] * 11)[:11]
    if r[0].strip().isdigit():
        docs[int(r[0])] = [dict(zip(COLS, r[1:8]))]
hidden = set()
ai = collections.defaultdict(list)
for r in csv.DictReader(open(L + "ai-labels.csv", newline="")):
    t = int(r["File Number"])
    if r["Duplicate Of"] or r.get("Hidden") == "yes": hidden.add(t); continue
    ai[t].append({c: r[c] for c in COLS})
docs.update(ai)
for f in ("attach-1.out.csv", "attach-2.out.csv"):
    for r in csv.DictReader(open(L + f, newline="")):
        if r["Decision"].startswith("duplicate"): hidden.add(int(r["File Number"]))
extra = {}
for r in csv.DictReader(open(L + "unpacked.csv", newline="")):
    t = int(r["File Number"]); docs.setdefault(t, []); 
    if not any(d is None for d in [t]): docs[t].append({c: r[c] for c in COLS})
    extra[t] = ext_of(r["Original Path"])
for z in (8053, 8063, 8627, 8631, 8643, 8651, 8652, 1769, 2306): hidden.add(z)

legacy = {}
for m in re.finditer(r"\((\d+), '(?:[^'\\]|\\.)*', '(?:[^'\\]|\\.)*', '(?:[^'\\]|\\.)*', \d+, '(?:[^'\\]|\\.)*', '[YN]', '((?:[^'\\]|\\.)*)'\)",
                     open(sql, encoding="utf8").read()):
    for t in re.findall(r'\\"tid\\": (\d+)', m[2]): legacy.setdefault(int(t), int(m[1]))

tests = {}
for tid, labs in sorted(docs.items()):
    if tid in hidden: continue
    for lab in labs:
        comp = clean("Comp Type", lab["Comp Type"])
        month = None if comp in MONTHLESS else clean("Comp Month", lab["Comp Month"])
        y = clean("Year", lab["Year"])
        key = (int(y) if y and y.isdigit() else None, comp, month, clean("Division", lab["Division"]),
               clean("Test", lab["Test"]), clean("Host", lab["Host"]))
        kind = clean("File Type", lab["File Type"]) or "Other"
        tests.setdefault(key, []).append((tid, kind, extra.get(tid) or ext_of(names.get(tid, "x.pdf"))))

DIV = ["Algebra I", "Geometry", "Algebra II", "Precalculus/Alpha", "Calculus/Mu", "Statistics", "Theta", "Open", "Euclidean", "Dostal", "Nunn", "Hiller"]
COMPS = ["Invitational", "Regional", "State", "National", "Mail-in"]
MONTHS = ["November", "January", "February", "March", "April"]
KINDS = ["Test", "Condensed Test", "Combined Test+Solutions", "Answers", "Solutions", "Errata", "Other"]
vals = lambda i: sorted({k[i] for k in tests if k[i]})
types = ["Individual", "Team"] + [v for v in vals(4) if v not in ("Individual", "Team")]
hosts = ["FAMAT", "MAO"] + [v for v in vals(5) if v not in ("FAMAT", "MAO")]
divs = DIV + [v for v in vals(3) if v not in DIV]
comps = COMPS + [v for v in vals(1) if v not in COMPS]
exts = sorted({d[2] for ds in tests.values() for d in ds})
ix = lambda lst, v: lst.index(v) if v is not None else -1

next_id, rows, used = 3707, [], set()
for key, ds in tests.items():
    ids = collections.Counter(legacy[t] for t, _, _ in ds if t in legacy)
    tid = ids.most_common(1)[0][0] if ids else None
    if tid is None or tid in used: tid = next_id; next_id += 1
    used.add(tid)
    ds = sorted(set(ds), key=lambda d: (KINDS.index(d[1]) if d[1] in KINDS else 99, d[0]))
    rows.append([tid, key[0], ix(comps, key[1]), ix(MONTHS, key[2]), ix(divs, key[3]), ix(types, key[4]), ix(hosts, key[5]),
                 [[t, KINDS.index(k) if k in KINDS else 6, exts.index(e)] for t, k, e in ds]])
json.dump({"comps": comps, "months": MONTHS, "divs": divs, "types": types, "hosts": hosts, "kinds": KINDS,
           "exts": exts, "tests": rows}, open(out, "w"), separators=(",", ":"))
print(len(rows), "Tests,", sum(len(r[7]) for r in rows), "Documents,", os.path.getsize(out) // 1024, "KB")
