#!/usr/bin/env python3
"""Build the static product site into site/dist from site/site.json, the repo's docs/ and branding/.

Every link is relative (or points to a different host on purpose), so the site can be served from any
host or path. To move to its own domain, serve this same output there and make the old host answer
with one 301 redirect. Needs the `markdown` package.  Usage: python3 site/build.py
"""
import html, json, os, re, shutil, sys, posixpath
import markdown

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CFG = json.load(open(os.path.join(HERE, "site.json")))
DIST = os.path.join(HERE, "dist")
SRC = f"https://git.canutethegreat.com/CanuteTheGreat/{CFG['repo']}"
BLOB = SRC + "/src/branch/main/"
DL = SRC + "/releases/download/"

def rel(frm, to):
    """relative URL from page dir `frm` ('' = root) to site path `to` (dir pages end with '/')."""
    frm_dir = frm.strip("/")
    r = posixpath.relpath(to.rstrip("/") or ".", frm_dir or ".")
    r = "./" if r == "." else r
    if to.endswith("/") and not r.endswith("/"):
        r += "/"
    return r

# ---- documents rendered from the repo -------------------------------------------------------
PAGES = {}   # source path under docs/ -> (site dir, title)
PAGES["ROADMAP.md"] = ("roadmap/", "Roadmap to 1.0")
PAGES["stable-channel.md"] = ("stable-channel/", "Stable channel design")
for fn, title in CFG["docs"]:
    PAGES[fn] = ("docs/" + re.sub(r"[^a-z0-9]+", "-", fn[:-3].lower()).strip("-") + "/", title)

def fix_links(body, page_dir):
    def sub(m):
        attr, url = m.group(1), m.group(2)
        if re.match(r"^(?:[a-z][a-z0-9+.-]*:|#|//)", url, re.I):
            return m.group(0)
        path, _, frag = url.partition("#")
        if not path:
            return m.group(0)
        norm = posixpath.normpath(posixpath.join("docs", path))   # repo-relative path
        frag = ("#" + frag) if frag else ""
        if norm.startswith("docs/") and norm[5:] in PAGES:
            return f'{attr}="{rel(page_dir, PAGES[norm[5:]][0])}{frag}"'
        if norm.startswith("docs/images/"):
            return f'{attr}="{rel(page_dir, "docs/images/" + norm[12:])}"'
        return f'{attr}="{BLOB}{norm}{frag}"'
    return re.sub(r'(href|src)="([^"]*)"', sub, body)

def md_to_html(text):
    md = markdown.Markdown(extensions=["tables", "fenced_code", "toc", "sane_lists"],
                           extension_configs={"toc": {"permalink": False}})
    return md.convert(text)

def page(page_dir, title, body, active="", desc=None):
    r = lambda to: rel(page_dir, to)
    items = [("download", (r("") .rstrip("/") + "/#download") if page_dir else "#download", "Download"),
             ("docs", r("docs/"), "Docs"), ("roadmap", r("roadmap/"), "Roadmap"),
             ("source", SRC, "Source")]
    cur = ' aria-current="page"'
    nav = "".join(f'<a href="{h}"{cur if k == active else ""}>{t}</a>' for k, h, t in items)
    d = html.escape(desc or CFG["tagline"])
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)} - {CFG['name']}</title>
<meta name="description" content="{d}">
<meta name="theme-color" content="#000000">
<link rel="icon" href="{r('assets/favicon.ico')}" sizes="any">
<link rel="apple-touch-icon" href="{r('assets/apple-touch-icon.png')}">
<link rel="stylesheet" href="{r('assets/style.css')}">
</head><body class="{CFG['key']}">
<header class="top"><div class="wrap bar">
<a class="brand" href="{r('') if page_dir else './'}"><img src="{r('assets/mark.png')}" alt="" width="32" height="32"><span>{CFG['name']}</span></a>
<nav aria-label="Main">{nav}</nav></div></header>
<div class="banner"><div class="wrap">Pre-1.0 software ({CFG['version']}). {CFG['banner']}</div></div>
{body}
<footer><div class="wrap foot">
<p>{CFG['name']} is a free, GPL-3.0 project by <a href="https://errant.solutions/">Errant Solutions</a>.
Source: <a href="{SRC}">Forgejo</a> &middot; <a href="https://github.com/CanuteTheGreat/{CFG['repo']}">GitHub mirror</a>.</p>
<p>Commercial support: planned after 1.0. Nothing is for sale today.</p></div></footer>
</body></html>
"""

def write(path, content):
    full = os.path.join(DIST, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    open(full, "w", encoding="utf-8").write(content)

def doc_page(src_name, active):
    text = open(os.path.join(ROOT, "docs", src_name), encoding="utf-8").read()
    d, title = PAGES[src_name]
    body = fix_links(md_to_html(text), d)
    edit = f'<p class="edit"><a href="{BLOB}docs/{src_name}">View or edit this page in the repository</a></p>'
    write(d + "index.html", page(d, title, f'<main class="wrap doc">{body}{edit}</main>', active))

def main():
    if os.path.isdir(DIST):
        shutil.rmtree(DIST)
    os.makedirs(DIST)
    A = os.path.join(DIST, "assets"); os.makedirs(A)
    br = os.path.join(ROOT, "branding")
    k = CFG["key"]
    for src, dst in [(f"{k}-mark-64.png", "mark.png"), (f"{k}-mark-180.png", "apple-touch-icon.png"),
                     ("favicon.ico", "favicon.ico"), (f"{k}-full-half.png", "logo.png"),
                     (f"{k}-mark-512.png", "mark-512.png")]:
        shutil.copy(os.path.join(br, src), os.path.join(A, dst))
    shutil.copy(os.path.join(HERE, "style.css"), os.path.join(A, "style.css"))
    shutil.copytree(os.path.join(ROOT, "docs", "images"), os.path.join(DIST, "docs", "images"))

    for name in PAGES:
        if os.path.exists(os.path.join(ROOT, "docs", name)):
            doc_page(name, "roadmap" if name == "ROADMAP.md" else ("docs" if name != "stable-channel.md" else ""))
        else:
            print("missing doc", name, file=sys.stderr); sys.exit(1)

    # docs index
    rows = "".join(f'<li><a href="{rel("docs/", PAGES[fn][0])}">{html.escape(t)}</a><span>{html.escape(desc)}</span></li>'
                   for fn, t, desc in CFG["docs_index"])
    idx = (f'<main class="wrap doc"><h1>Documentation</h1><p>Rendered from the <code>docs/</code> folder of the repository, '
           f'rebuilt on every change. The folder holds more files than are listed here: '
           f'<a href="{BLOB}docs/">browse them all</a>. Some of them are older notes and may be out of date; the roadmap checklist is the source of truth for status.</p>'
           f'<ul class="doclist">{rows}</ul></main>')
    write("docs/index.html", page("docs/", "Documentation", idx, "docs"))

    # landing
    sh = "".join(f'<figure><img src="{rel("", "docs/images/" + p)}" alt="{html.escape(a)}" loading="lazy"><figcaption>{html.escape(a)}</figcaption></figure>'
                 for p, a in CFG["shots"])
    dl_rows = "".join(
        f'<tr><td>{html.escape(a)}</td><td><a href="{DL}daily/{f}">{f}</a></td><td><a href="{DL}daily/{f}.sha256">sha256</a></td></tr>'
        for a, f in CFG["isos"])
    feats = "".join(f"<li>{html.escape(x)}</li>" for x in CFG["works"])
    gaps = "".join(f"<li>{html.escape(x)}</li>" for x in CFG["gaps"])
    land = f"""<main>
<section class="hero"><div class="wrap herogrid">
<div><h1>{CFG['name']}</h1><p class="lead">{CFG['lead']}</p>
<p class="cta"><a class="btn" href="#download">Get a daily build</a> <a class="btn ghost" href="{rel('', 'roadmap/')}">Roadmap to 1.0</a> <a class="btn ghost" href="{rel('', 'docs/')}">Docs</a></p></div>
<img class="logo" src="{rel('', 'assets/logo.png')}" alt="{CFG['name']} logo" width="{CFG['logo_w']}" height="{CFG['logo_h']}"></div></section>
<section class="wrap"><h2>What it is</h2>{CFG['what_html']}</section>
<section class="wrap"><h2>Status: pre-1.0</h2><p>{CFG['status']}</p>
<div class="two"><div><h3>What CI proves today</h3><ul>{feats}</ul></div><div><h3>What is not done</h3><ul>{gaps}</ul></div></div>
<p>The measurable exit criteria for 1.0, and how far each one is, are on the <a href="{rel('', 'roadmap/')}">roadmap checklist</a>.</p></section>
<section class="wrap" id="download"><h2>Download</h2>
<p>Daily builds are test builds of the <code>main</code> branch, replaced in place. A build is published only after QEMU boot, install, reboot and web probe tests passed. They are not releases: use test hardware or a VM. There is no stable release yet. Secure Boot must be off.</p>
<table class="dl"><thead><tr><th>Architecture</th><th>ISO</th><th>Checksum</th></tr></thead><tbody>{dl_rows}</tbody></table>
<p>{CFG['dl_note']} Verify with <code>sha256sum -c</code>, write to a USB stick with <code>dd</code> (see the <a href="{rel('', 'docs/iso-install/')}">install guide</a>), or boot it in a VM. <a href="{DL}daily/{CFG['evidence']}">Test evidence</a> (serial logs and screenshots) is attached to every build.</p></section>
<section class="wrap"><h2>The installer</h2><p>A text-mode wizard, on a VGA screen or a serial console. These are real captures from the CI test. A graphical installer is in progress.</p><div class="shots">{sh}</div></section>
<section class="wrap"><h2>Commercial support</h2>
<p><strong>Planned after 1.0.</strong> The idea, in the style of Proxmox: an optional paid stable update channel with email support in business hours, best effort, no 24/7 guarantee. The free channel stays complete, with every feature. Nothing is for sale now and there is no sign-up. The <a href="{rel('', 'stable-channel/')}">design</a> is public, and it only starts after a stable 1.0 and a period of real use.</p></section>
</main>"""
    write("index.html", page("", CFG["name"], land, "", CFG["tagline"]))
    nf = page("", "Not found", '<main class="wrap doc"><h1>Page not found</h1><p><a href="/">Back to the start page</a>.</p></main>')
    # a 404 can be served from any depth, so its asset links are host-root paths
    nf = re.sub(r'(href|src)="(assets/[^"]*)"', r'\1="/\2"', nf)
    write("404.html", nf)
    write("robots.txt", "User-agent: *\nAllow: /\n")
    print("built", DIST)

main()
