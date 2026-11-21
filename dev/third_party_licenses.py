#!/usr/bin/env python3
"""Write THIRD_PARTY_LICENSES.md: the licenses of everything ClaudeRust ships.

Covers the Rust crates compiled into the app (from `cargo metadata`, all
platforms), the browser libraries under ui/vendor, and the Feather icon
paths used in ui/js/icons.js. Identical license texts are printed once,
followed by the packages they apply to.

Run from the repository root after changing dependencies:
    python3 dev/third_party_licenses.py
"""
import hashlib
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "THIRD_PARTY_LICENSES.md")
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense")

FEATHER = """The MIT License (MIT)

Copyright (c) 2013-2023 Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def read(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read().replace("\r\n", "\n").strip() + "\n"


def license_files(pkg_dir):
    try:
        names = sorted(os.listdir(pkg_dir))
    except OSError:
        return []
    out = []
    for n in names:
        p = os.path.join(pkg_dir, n)
        if os.path.isfile(p) and n.lower().startswith(LICENSE_PREFIXES) and os.path.getsize(p) < 200_000:
            out.append(p)
    return out


def main():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT))
    ours = set(meta["workspace_members"])
    texts = {}   # hash -> text
    users = {}   # hash -> [package labels]
    bare = []    # packages that ship no license file
    for pkg in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        if pkg["id"] in ours:
            continue
        label = f'{pkg["name"]} {pkg["version"]} ({pkg.get("license") or "see license file"})'
        files = license_files(os.path.dirname(pkg["manifest_path"]))
        if not files:
            authors = ", ".join(pkg.get("authors") or []) or "its authors"
            bare.append(f'{label} — {pkg.get("repository") or "crates.io"} — copyright {authors}')
            continue
        # "MIT OR Apache-2.0" (or the old "MIT/Apache-2.0"): we take the MIT
        # option, so only its text (and any NOTICE / COPYRIGHT file) is needed
        expr = pkg.get("license") or ""
        alternatives = [a.strip("() ") for a in expr.replace("/", " OR ").split(" OR ")]
        mit_files = [f for f in files if "mit" in os.path.basename(f).lower()]
        if "MIT" in alternatives and mit_files:
            files = mit_files + [f for f in files if os.path.basename(f).lower().startswith(("notice", "copyright"))]
        for f in files:
            t = read(f)
            if "Apache License" in t and "Version 2.0, January 2004" in t and "END OF TERMS AND CONDITIONS" in t:
                # one copy of the Apache terms; a filled-in copyright line from
                # the appendix stays with the package
                terms = t[: t.index("END OF TERMS AND CONDITIONS") + len("END OF TERMS AND CONDITIONS")] + "\n"
                owners = [ln.strip() for ln in t[len(terms):].splitlines() if ln.strip().lower().startswith("copyright") and "[" not in ln]
                h = "apache-2.0"
                texts.setdefault(h, terms)
                users.setdefault(h, []).append(label + (f" — {'; '.join(owners)}" if owners else ""))
                continue
            # the same text re-wrapped or re-indented counts as the same
            h = hashlib.sha256(" ".join(t.split()).encode()).hexdigest()
            texts[h] = t
            users.setdefault(h, []).append(label)

    vendored = [
        ("KaTeX (ui/vendor/katex)", read(os.path.join(ROOT, "ui/vendor/katex/LICENSE"))),
        ("xterm.js and addon-fit (ui/vendor/xterm)", read(os.path.join(ROOT, "ui/vendor/xterm/LICENSE"))),
        ("Feather icons (some icon paths in ui/js/icons.js)", FEATHER),
    ]

    lines = [
        "# Third-party licenses",
        "",
        "ClaudeRust is MIT-licensed (see LICENSE). It includes the software below,",
        "under the licenses that follow. This file is generated by",
        "`dev/third_party_licenses.py`; Settings → About → Open-source licenses",
        "shows it in the app.",
        "",
        "## Libraries bundled with the interface",
        "",
    ]
    for name, text in vendored:
        lines += [f"### {name}", "", "```text", text.rstrip(), "```", ""]
    lines += ["## Rust crates compiled into the app", ""]
    for h in sorted(users, key=lambda k: (-len(users[k]), users[k][0])):
        pkgs = sorted(set(users[h]))
        lines += [f"### License text used by {len(pkgs)} package(s)", ""]
        lines += [f"- {p}" for p in pkgs]
        lines += ["", "```text", texts[h].rstrip(), "```", ""]
    if bare:
        lines += ["### Packages without a license file", "",
                  "These are distributed under the SPDX license named next to them; the",
                  "standard texts of those licenses appear above.", ""]
        lines += [f"- {b}" for b in bare]
        lines.append("")
    with open(OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    crates = len({p for v in users.values() for p in v}) + len(bare)
    print(f"wrote {os.path.relpath(OUT, ROOT)}: {crates} crates, {len(texts)} distinct texts, "
          f"{os.path.getsize(OUT) // 1024} KB", file=sys.stderr)


if __name__ == "__main__":
    main()
