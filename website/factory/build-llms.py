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

# The year in the headline is this year's. The page's script keeps it current for readers; this
# stamps the copies crawlers and link previews read, which run no script, on every build.
import datetime
index = PUBLIC / "index.html"
index.write_text(re.sub(r"(It's |datetime=\")\d{4}", lambda m: m.group(1) + str(datetime.date.today().year), re.sub(r'(class="this-year" datetime="\d{4}">)\d{4}', lambda m: m.group(1) + str(datetime.date.today().year), index.read_text())))
home = index.read_text()
out = ["# Oya Factory: full description", "",
       "> " + text(re.search(r'<meta name="description" content="(.*?)"', home).group(1)), "",
       "Website: https://oyafactory.com/", "Download: https://github.com/OyadotAI/keel-releases/releases/latest",
       "Source: https://github.com/OyadotAI/keel (MIT licence)", "Made by: Oya (https://oya.dev)", "",
       "## In one line", "", text(re.search(r'<h1 id="hero-title">(.*?)</h1>', home, re.S).group(1)), "",
       text(re.search(r'<p class="lede">(.*?)</p>', home, re.S).group(1)), "",
       "Used by engineers from " + ", ".join(text(n) for n in re.findall(r"<li[^>]*>(?:<svg.*?</svg>)?([^<]+)</li>", re.search(r'<div class="logos">(.*?)</div>', home, re.S).group(1)) if "more" not in n) + " and other companies.", "",
       "## The story of one feature", ""]
for t, h, c in re.findall(r'<p class="ch-time"><span>\d+</span>(.*?)</p>\s*<h2 class="ch-title">(.*?)</h2>\s*<p class="ch-copy">(.*?)</p>', home):
    out += [f"### {text(t)}: {text(h)}", "", text(c), ""]
out += ["## The platform", ""]
for h, p in re.findall(r'<h3>(.*?)</h3>\s*<p>(.*?)</p>', re.search(r'<div class="tiles">(.*?)</section>', home, re.S).group(1), re.S):
    out += [f"### {text(h)}", "", text(p), ""]
out += ["## Also included", ""] + [f"- {text(n)}" for n in re.findall(r"<li>(.*?)</li>", re.search(r'<div class="marquee"><ul>(.*?)</ul>', home).group(1))]
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
# Some agents look under /.well-known first.
(PUBLIC / ".well-known").mkdir(exist_ok=True)
(PUBLIC / ".well-known" / "llms.txt").write_text((PUBLIC / "llms.txt").read_text())
print(f"llms-full.txt: {len(out)} lines")
