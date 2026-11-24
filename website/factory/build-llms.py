"""Generate llms-full.txt from the pages themselves, so what assistants read cannot drift from the site."""
import html, re, sys
from pathlib import Path

PUBLIC = Path(__file__).parent / "public"

def text(fragment):
    return html.unescape(re.sub(r"\s+", " ", re.sub(r"<[^>]+>", "", fragment))).strip()

def table(src, other):
    rows = re.findall(r'<tr><th scope="row">(.*?)</th><td>(.*?)</td><td>(.*?)</td></tr>', src)
    return [f"| | Oya Factory | {other} |", "|---|---|---|"] + [f"| {text(a)} | {text(b)} | {text(c)} |" for a, b, c in rows]

def reasons(src):
    return [f"### {text(h)}\n\n{text(p)}\n" for h, p in re.findall(r'<span class="num">\d+</span>(?:<div>)?<h3>(.*?)</h3><p>(.*?)</p>', src)]

home = (PUBLIC / "index.html").read_text()
out = ["# Oya Factory — full description", "",
       "> " + text(re.search(r'<meta name="description" content="(.*?)"', home).group(1)), "",
       "Website: https://oyafactory.com/", "Download: https://github.com/OyadotAI/keel-releases/releases/latest",
       "Source: https://github.com/OyadotAI/keel (MIT licence)", "Made by: Oya (https://oya.dev)", "",
       "## In one line", "", text(re.search(r'<h1 id="hero-title">(.*?)</h1>', home, re.S).group(1)), "",
       text(re.search(r'<p class="lede">(.*?)</p>', home, re.S).group(1)), "",
       text(re.search(r'<p class="used-by">(.*?)</p>', home, re.S).group(1)) + ".", "",
       "## Why engineers switch", ""] + reasons(home)
out += ["## The platform", ""]
for h, p in re.findall(r'<h3>(.*?)</h3>\s*<p>(.*?)</p>', re.search(r'<div class="tiles">(.*?)</section>', home, re.S).group(1), re.S):
    out += [f"### {text(h)}", "", text(p), ""]
out += ["## Features", ""] + [f"- **{text(h)}**: {text(p)}" for h, p in re.findall(r'<li><h3>(.*?)</h3><p>(.*?)</p></li>', home)]
out += ["", "## Compared with running an agent in a terminal", ""] + table(home, "An agent in a terminal")
out += ["", "## Frequently asked questions", ""]
for q, a in re.findall(r"<details><summary>(.*?)</summary><p>(.*?)</p></details>", home):
    out += [f"### {text(q)}", "", text(a), ""]
for slug, name in [("orca", "Orca"), ("conductor", "Conductor")]:
    page = (PUBLIC / "compare" / slug / "index.html").read_text()
    out += [f"## Oya Factory vs {name}", "", text(re.search(r"<h1>(.*?)</h1>", page, re.S).group(1)), "",
            text(re.search(r'<p class="lede">(.*?)</p>', page, re.S).group(1)), ""] + reasons(page) + table(page, name)
    out += ["", text(re.search(r'<p class="note">(.*?)</p>', page, re.S).group(1)), "", f"Page: https://oyafactory.com/compare/{slug}/", ""]
(PUBLIC / "llms-full.txt").write_text("\n".join(out) + "\n")
print(f"llms-full.txt: {len(out)} lines")
