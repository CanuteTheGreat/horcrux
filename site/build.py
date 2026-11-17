#!/usr/bin/env python3
"""Build the static product site into site/dist from site/site.json, the repo's docs/ and branding/.

Every link is relative (or points to a different host on purpose), so the site can be served from any
host or path. To move to its own domain, serve this same output there and make the old host answer
with one 301 redirect. Needs the `markdown` package.  Usage: python3 site/build.py
"""
import html, json, os, re, shutil, sys, posixpath
import markdown
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gating

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CFG = json.load(open(os.path.join(HERE, "site.json")))
DIST = os.path.join(HERE, "dist")
# Public source = the GitHub mirror. The site never links to the owner's personal git host (site/check.py fails the
# build if any built file mentions it). ISO downloads are stable URLs on this site (/download/..., nginx 302s them to
# the GitHub release assets, see nginx.conf), so the visible links stay on the product domain.
SRC = f"https://github.com/CanuteTheGreat/{CFG['repo']}"
BLOB = SRC + "/blob/main/"
TREE = SRC + "/tree/main/"

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
    # CI plumbing is noise for readers (and not served by the public mirror's UI the way docs are): drop the link, keep the text
    def unlink(m):
        url = m.group(1)
        if re.match(r"^(?:[a-z][a-z0-9+.-]*:|#|//)", url, re.I):
            return m.group(0)
        norm = posixpath.normpath(posixpath.join("docs", url.partition("#")[0]))
        return m.group(2) if norm.startswith((".forgejo/", ".github/", ".gitea/")) else m.group(0)
    body = re.sub(r'<a href="([^"]*)">(.*?)</a>', unlink, body, flags=re.S)
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
        base = TREE if os.path.isdir(os.path.join(ROOT, norm)) else BLOB
        return f'{attr}="{base}{norm}{frag}"'
    return re.sub(r'(href|src)="([^"]*)"', sub, body)

def md_to_html(text):
    md = markdown.Markdown(extensions=["tables", "fenced_code", "toc", "sane_lists"],
                           extension_configs={"toc": {"permalink": False}})
    out = md.convert(text)
    # tables sit in a scroll box so wide ones scroll sideways on phones instead of widening the page
    return out.replace("<table>", '<div class="tw"><table>').replace("</table>", "</table></div>")

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
Source: <a href="{SRC}">GitHub</a>.</p>
<p>Commercial support: planned after 1.0. Nothing is for sale today.</p></div></footer>
</body></html>
"""

WRITTEN_HTML = []

def write(path, content):
    if path.endswith(".html") and path != "404.html":
        WRITTEN_HTML.append(path)
    full = os.path.join(DIST, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    open(full, "w", encoding="utf-8").write(content)

def doc_page(src_name, active):
    text = open(os.path.join(ROOT, "docs", src_name), encoding="utf-8").read()
    d, title = PAGES[src_name]
    body = fix_links(md_to_html(text), d)
    edit = f'<p class="edit"><a href="{BLOB}docs/{src_name}">View or edit this page in the repository</a></p>'
    write(d + "index.html", page(d, title, f'<main class="wrap doc">{body}{edit}</main>', active))

def signing_fpr():
    """Fingerprint of keys/release-signing.asc (gpg --show-keys), spaced like gpg prints it; fails the build without gpg."""
    import subprocess
    out = subprocess.run(["gpg", "--batch", "--with-colons", "--show-keys", os.path.join(ROOT, "keys", "release-signing.asc")],
                         capture_output=True, text=True, check=True).stdout
    fpr = next(l.split(":")[9] for l in out.splitlines() if l.startswith("fpr:"))
    return " ".join(fpr[i:i + 4] for i in range(0, 20, 4)) + "  " + " ".join(fpr[i:i + 4] for i in range(20, 40, 4))


def security_txt(host):
    import datetime
    exp = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=330)).strftime("%Y-%m-%dT00:00:00Z")
    return (f"Contact: mailto:security@errant.solutions\nExpires: {exp}\nEncryption: https://{host}/signing-key.asc\n"
            f"Preferred-Languages: en\nCanonical: https://{host}/.well-known/security.txt\nPolicy: https://{host}/security/\n")


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
           f'<a href="{TREE}docs/">browse them all</a>. Some of them are older notes and may be out of date; the roadmap checklist is the source of truth for status.</p>'
           f'<ul class="doclist">{rows}</ul></main>')
    write("docs/index.html", page("docs/", "Documentation", idx, "docs"))

    # landing
    sh = "".join(f'<figure><img src="{rel("", "docs/images/" + p)}" alt="{html.escape(a)}" loading="lazy"><figcaption>{html.escape(a)}</figcaption></figure>'
                 for p, a in CFG["shots"])
    DLD = rel("", "download/")
    SIGNING_FPR_SPACED = signing_fpr()
    assets = gating.release_assets(CFG["repo"])
    def cell(iso, label, listed):
        if not listed:
            return '<td class="v"><span class="muted">systemd build: coming soon</span></td>'
        ev = gating.evidence_name(iso)
        evl = f' &middot; <a href="{DLD}{ev}">evidence</a>' if assets is None or ev in assets else ""
        # signature + SBOM links only once the build that carries them is published (older dailies have neither)
        sig = f' &middot; <a href="{DLD}{iso}.asc">signature</a>' if assets is not None and iso + ".asc" in assets else ""
        sb = iso[:-4] + ".cdx.json"
        sbom = f' &middot; <a href="{DLD}{sb}">SBOM</a>' if assets is not None and sb in assets else ""
        return (f'<td class="v"><a class="btn sm" href="{DLD}{iso}" title="{iso}">{label} ISO</a> '
                f'<span class="aux"><a href="{DLD}{iso}.sha256">sha256</a>{sig}{sbom}{evl}</span>'
                f'<span class="fn">{iso}</span></td>')
    dl_rows = ""
    for a, f in CFG["isos"]:
        tw = gating.systemd_name(f)
        shown = gating.systemd_listed(f, assets)
        dl_rows += (f'<tr><td>{html.escape(a.replace(" \u00b7 OpenRC", ""))}</td>' + cell(f, "OpenRC", True)
                    + cell(tw, "systemd", shown) + '</tr>')
        print(f"download {tw}: {'listed' if shown else 'hidden (not published)'}")
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
<p><strong>OpenRC or systemd:</strong> the same edition built on either init system. The installed system uses the init of the image you download; the installer does not switch it. Pick systemd if you want upstream GNOME session and systemd tooling, OpenRC for the classic Gentoo init.</p>
<div class="tw"><table class="dl"><thead><tr><th>Image: desktop &middot; architecture</th><th>OpenRC</th><th>systemd</th></tr></thead><tbody>{dl_rows}</tbody></table></div>
<p>{CFG['dl_note']} Verify with <code>sha256sum -c</code>, write to a USB stick with <code>dd</code> (see the <a href="{rel('', 'docs/iso-install/')}">install guide</a>), or boot it in a VM. The test evidence (serial logs and screenshots) is attached to every build.</p>
<h3 id="verify">Verify your download</h3>
<p>Every image, its <code>.sha256</code>, its SBOM (CycloneDX JSON listing every installed Gentoo package and Rust crate) and the test evidence has a detached OpenPGP signature (<code>.asc</code>) made with the Errant Solutions release key. Get the <a href="{rel('', 'signing-key.asc')}">release signing key</a> and check its fingerprint:</p>
<pre><code>{SIGNING_FPR_SPACED}</code></pre>
<pre><code>curl -fsSLO https://{CFG['key']}.errant.solutions/signing-key.asc
gpg --import signing-key.asc
gpg --verify {CFG['key']}-server-openrc-daily-x86_64.iso.asc {CFG['key']}-server-openrc-daily-x86_64.iso
sha256sum -c {CFG['key']}-server-openrc-daily-x86_64.iso.sha256</code></pre>
<p><code>gpg</code> must say <em>Good signature from "Errant Solutions Release Signing"</em> with the fingerprint above. A bad signature means the file was changed or damaged: do not use it. How to report a security problem: <a href="{rel('', 'security/')}">security policy</a>.</p>
<h3>How the image names work</h3>
<p>Every image is named <code>&lt;product&gt;-&lt;desktop or server&gt;-&lt;init&gt;-&lt;channel&gt;-&lt;arch&gt;.iso</code>, for example <code>{CFG['key']}-server-openrc-daily-x86_64.iso</code>.
<strong>Desktop or server</strong> is the graphical environment the image installs: <code>server</code> means no desktop. An image with a desktop gets its environment in the name, for example <code>gnome-light</code> (GNOME on Wayland, small app set), <code>xfce</code> or <code>kde</code>.
<strong>Init</strong> is the init system: <code>openrc</code> or <code>systemd</code>. Both are offered for each edition and architecture; a systemd image appears in the table above once its build has passed the same QEMU tests (until then the table says so).
<strong>Channel</strong> is <code>daily</code> (the rolling test build) or a release tag. <strong>Arch</strong> is <code>x86_64</code> or <code>aarch64</code>.
Older download names (for example <code>{CFG['key']}-daily-x86_64.iso</code>) still work: they redirect (HTTP 301) to the new name.</p></section>
<section class="wrap"><h2>The installer</h2><p>A text-mode wizard, on a VGA screen or a serial console. These are real captures from the CI test. A graphical installer is in progress.</p><div class="shots">{sh}</div></section>
<section class="wrap"><h2>Commercial support</h2>
<p><strong>Planned after 1.0.</strong> The idea, in the style of Proxmox: an optional paid stable update channel with email support in business hours, best effort, no 24/7 guarantee. The free channel stays complete, with every feature. Nothing is for sale now and there is no sign-up. The <a href="{rel('', 'stable-channel/')}">design</a> is public, and it only starts after a stable 1.0 and a period of real use.</p></section>
</main>"""
    write("index.html", page("", CFG["name"], land, "", CFG["tagline"]))
    nf = page("", "Not found", '<main class="wrap doc"><h1>Page not found</h1><p><a href="/">Back to the start page</a>.</p></main>')
    # a 404 can be served from any depth, so its asset links are host-root paths
    nf = re.sub(r'(href|src)="(assets/[^"]*)"', r'\1="/\2"', nf)
    write("404.html", nf)
    host = f"{CFG['key']}.errant.solutions"
    urls = sorted({"/" + (pp[:-len("index.html")] if pp.endswith("index.html") else pp) for pp in WRITTEN_HTML})
    write("sitemap.xml", '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
          + "".join(f"  <url><loc>https://{host}{u}</loc></url>\n" for u in urls) + "</urlset>\n")
    # release signing key + RFC 9116 security.txt (contact = the Errant Solutions security address, same as errant.solutions)
    shutil.copy(os.path.join(ROOT, "keys", "release-signing.asc"), os.path.join(DIST, "signing-key.asc"))
    write(".well-known/security.txt", security_txt(host))
    write("security/index.html", page("security/", "Security", '<main class="wrap doc">' + md_to_html(open(os.path.join(ROOT, "SECURITY.md"), encoding="utf-8").read()) + '</main>', ""))
    write("robots.txt", f"User-agent: *\nAllow: /\n\nSitemap: https://{host}/sitemap.xml\n")
    # IndexNow (Bing, Yandex, Seznam, Naver) proof-of-ownership key file; .forgejo/scripts/indexnow.sh submits after each deploy
    write("5bd2340b776c5d0b43933a1f02f4281e.txt", "5bd2340b776c5d0b43933a1f02f4281e")
    print("built", DIST)

main()
