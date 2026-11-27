"""Shared donation component for the Errant Solutions product sites (Horcrux, Patronus).

This file and give.css are BYTE-IDENTICAL in both repos (site/give.py, site/give.css): change them in both, then
update site/give.lock in both (python3 site/check.py prints the new hash). The only per-product inputs are the
product name and key (from site.json) and the brand accent colour, which comes from style.css
(--accent / --accent-fg on body.<key>). Layout, spacing, cards and button order are the same everywhere.
"""
GIVE_VERSION = "1"
GH_SPONSORS = "https://github.com/sponsors/CanuteTheGreat"


def support_url(key):
    """The product's support page on errant.solutions (Stripe tip / monthly, BTCPay crypto, business plans)."""
    return f"https://errant.solutions/support/{key}/"


def strip(name, key):
    """Highlighted strip under the hero: one sentence plus three buttons, always in the same order."""
    s = support_url(key)
    return f"""<section class="wrap givebar" aria-label="Donate"><div class="box">
<p class="give-text"><strong>{name} is free and built by one person.</strong> If it is useful to you, a one-time tip or a small monthly sponsorship keeps the daily builds and the test machines going.</p>
<p class="give-btns"><a class="btn" href="#support">&#9829; Ways to give</a><a class="btn ghost" href="{s}#tip">One-time tip</a><a class="btn ghost" href="{s}#monthly">Monthly</a></p>
</div></section>"""


def section(name, key, src):
    """The #support section: intro, four cards (one per way to give), other ways to help."""
    s = support_url(key)
    cards = [
        ("One-time tip", "Any amount, by card or the wallets Stripe offers, on Stripe's own checkout page. Receipt by email, no account needed.", "btn", f"{s}#tip", "Leave a tip"),
        ("Monthly sponsor", "A small amount every month through Stripe. You can cancel it yourself at any time.", "btn", f"{s}#monthly", "Sponsor monthly"),
        ("GitHub Sponsors", "Already on GitHub? Sponsor once or monthly from your GitHub account.", "btn ghost", GH_SPONSORS, "Sponsor on GitHub"),
        ("Bitcoin or Monero", "On our own BTCPay Server: no exchange or payment company in between, no account or email. The page lists the coins that are ready right now.", "btn ghost", f"{s}#crypto", "Donate with crypto"),
    ]
    c = "\n".join(f'<div class="card"><h3>{h}</h3><p>{t}</p><a class="{cls}" href="{u}">{b}</a></div>' for h, t, cls, u, b in cards)
    return f"""<section class="wrap" id="support"><h2>Support {name}: donate</h2>
<p>{name} is free software and stays free: every feature is in the free build, and nobody gets a different version for paying. It is built by one person. Donations pay for the real test machines and hosting behind the daily builds, and for the time to keep building.</p>
<div class="give">
{c}
</div>
<p>Other ways to help, worth as much as money: try a daily build and <a href="{src}/issues">report what breaks</a>, or star the project on <a href="{src}">GitHub</a>.</p></section>"""


def footer_line(key):
    s = support_url(key)
    return (f'<p class="give-foot"><strong>Donate:</strong> <a href="{s}#tip">one-time tip</a> &middot; <a href="{s}#monthly">monthly</a>'
            f' &middot; <a href="{GH_SPONSORS}">GitHub Sponsors</a> &middot; <a href="{s}#crypto">Bitcoin / Monero</a>.'
            f' Support plans for businesses: yearly, best-effort email support, no SLA (<a href="{s}#plans">plans</a>).'
            ' A paid stable update channel is planned after 1.0.</p>')
