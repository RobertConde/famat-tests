"""Print the visible text of elements with given ids from HTML on stdin.

Usage: curl -s URL | python3 -I scripts/textof.py count results
"""
import re
import sys
from html.parser import HTMLParser


class P(HTMLParser):
    def __init__(self, ids):
        super().__init__()
        self.ids, self.stack, self.out = set(ids), [], {}

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag in ("input", "br", "meta", "link"):
            if self.stack and tag == "input":
                self.out[self.stack[-1][1]].append(f"[input {a.get('type')} name={a.get('name')}]")
            return
        if self.stack:
            self.stack.append((tag, self.stack[-1][1]))
            if tag == "li":
                self.out[self.stack[-1][1]].append("\n  - ")
        elif a.get("id") in self.ids:
            self.stack.append((tag, a["id"]))
            self.out.setdefault(a["id"], [])

    def handle_endtag(self, tag):
        if self.stack:
            self.stack.pop()

    def handle_data(self, data):
        if self.stack:
            self.out[self.stack[-1][1]].append(data)


p = P(sys.argv[1:])
p.feed(sys.stdin.read())
for k in sys.argv[1:]:
    text = re.sub(r"[ \t]+", " ", "".join(p.out.get(k, ["<missing>"]))).strip()
    print(f"#{k}: {text}")
