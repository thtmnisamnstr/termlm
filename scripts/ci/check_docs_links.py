#!/usr/bin/env python3
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]

MD_FILES = [ROOT / "README.md"]
MD_FILES.extend(sorted((ROOT / "docs").glob("*.md")))
for name in ["CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md"]:
    p = ROOT / name
    if p.exists():
        MD_FILES.append(p)

pattern = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
failures = []

for md in MD_FILES:
    text = md.read_text(encoding="utf-8")
    for match in pattern.finditer(text):
        raw = match.group(1).strip()
        if not raw:
            continue
        if raw.startswith(("http://", "https://", "mailto:", "#")):
            continue
        link = raw.split(" ")[0]
        link = link.split("#", 1)[0]
        if not link:
            continue
        target = (md.parent / link).resolve() if not pathlib.Path(link).is_absolute() else pathlib.Path(link)
        if not target.exists():
            failures.append(f"{md.relative_to(ROOT)} -> {raw}")

if failures:
    print("Broken documentation links:")
    for item in failures:
        print(f"  - {item}")
    sys.exit(1)

print("doc link check passed")
