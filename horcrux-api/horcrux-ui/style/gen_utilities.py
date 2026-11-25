#!/usr/bin/env python3
"""Generate style/utilities.css: the utility classes the pages use (Tailwind-style names), mapped onto
the UI system tokens (docs/design/ui-system.md). Pages written with these names rendered unstyled
before (only .ha-page had a scoped copy). Run after adding a new utility name to a page:

    python3 horcrux-api/horcrux-ui/style/gen_utilities.py

The class list is read from src/**/*.rs, so only names a page actually uses are emitted.
"""
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "src")

SPACE = {"0": "0", "0.5": "2px", "1": "var(--s-1)", "1.5": "6px", "2": "var(--s-2)", "2.5": "10px", "3": "var(--s-3)",
         "4": "var(--s-4)", "5": "var(--s-5)", "6": "var(--s-6)", "8": "var(--s-8)", "10": "40px", "12": "48px", "20": "80px"}
SIZE = {"4": "16px", "5": "20px", "6": "24px", "12": "48px", "24": "96px", "28": "112px", "40": "160px", "96": "384px",
        "full": "100%", "11/12": "91.666%", "1/3": "33.333%"}
MAXW = {"xs": "20rem", "md": "28rem", "lg": "32rem", "2xl": "42rem", "3xl": "48rem", "4xl": "56rem"}
FS = {"xs": "var(--fs-xs)", "sm": "var(--fs-sm)", "md": "var(--fs-md)", "lg": "var(--fs-lg)", "xl": "var(--fs-xl)",
      "2xl": "var(--fs-2xl)", "4xl": "30px", "6xl": "40px"}
# colour families -> tones. Text shades: 400-600 muted/tone text, 700-900 strong.
TONE = {"blue": "info", "indigo": "accent", "purple": "accent", "green": "ok", "red": "crit", "yellow": "warn"}


def text_color(fam, shade):
    s = int(shade)
    if fam == "gray":
        if s <= 100:
            return "var(--c-on-solid)"
        if s <= 300:
            return "var(--c-muted-soft)"
        return "var(--c-muted)" if s <= 600 else "var(--c-ink)"
    if fam == "white":
        return "var(--c-on-solid)"
    t = TONE[fam]
    return f"var(--c-{t})" if t != "accent" else "var(--c-accent)"


def bg_color(fam, shade):
    s = int(shade) if shade else 0
    if fam == "white":
        return "var(--c-surface)"
    if fam == "black":
        return "#000"
    if fam == "gray":
        if s <= 100:
            return "var(--c-sunken)"
        if s <= 300:
            return "var(--c-border)"
        return "var(--c-neutral-solid)" if s <= 600 else "var(--c-chrome)"
    t = TONE[fam]
    if s <= 100:
        return f"var(--c-{t}-bg)" if t != "accent" else "var(--c-accent-subtle)"
    return f"var(--c-{t}-solid)" if t != "accent" else "var(--c-accent-solid)"


def border_color(fam, shade):
    if fam == "gray":
        return "var(--c-border)" if int(shade) <= 200 else "var(--c-border-strong)"
    return f"var(--c-{TONE[fam]})"


FIXED = {
    "flex": "display:flex", "inline-flex": "display:inline-flex", "grid": "display:grid", "block": "display:block",
    "inline": "display:inline", "hidden": "display:none", "flex-1": "flex:1 1 0%", "flex-wrap": "flex-wrap:wrap",
    "flex-shrink-0": "flex-shrink:0", "items-center": "align-items:center", "items-end": "align-items:flex-end",
    "items-start": "align-items:flex-start", "justify-between": "justify-content:space-between",
    "justify-center": "justify-content:center", "justify-end": "justify-content:flex-end", "mx-auto": "margin-left:auto;margin-right:auto",
    "font-bold": "font-weight:600", "font-semibold": "font-weight:600", "font-medium": "font-weight:500",
    "font-mono": "font-family:var(--font-mono)", "text-center": "text-align:center", "text-left": "text-align:left",
    "text-right": "text-align:right", "uppercase": "text-transform:uppercase", "tracking-wider": "letter-spacing:.04em",
    "whitespace-nowrap": "white-space:nowrap", "whitespace-normal": "white-space:normal", "whitespace-pre-wrap": "white-space:pre-wrap",
    "truncate": "overflow:hidden;text-overflow:ellipsis;white-space:nowrap", "overflow-hidden": "overflow:hidden",
    "overflow-x-auto": "overflow-x:auto", "overflow-y-auto": "overflow-y:auto", "relative": "position:relative",
    "fixed": "position:fixed", "inset-0": "inset:0", "z-50": "z-index:50", "rounded": "border-radius:var(--r-sm)",
    "rounded-md": "border-radius:var(--r-md)", "rounded-lg": "border-radius:var(--r-md)", "rounded-full": "border-radius:999px",
    "shadow": "box-shadow:var(--e-1)", "shadow-lg": "box-shadow:var(--e-2)", "border": "border:1px solid var(--c-border)",
    "border-b": "border-bottom:1px solid var(--c-border)", "border-t": "border-top:1px solid var(--c-border)",
    "min-w-0": "min-width:0", "min-w-full": "min-width:100%", "h-full": "height:100%", "max-h-64": "max-height:16rem",
    "max-h-96": "max-height:24rem", "max-h-\\[90vh\\]": "max-height:90vh", "opacity-25": "opacity:.25", "opacity-75": "opacity:.75",
    "bg-opacity-50": "--bg-o:.5", "transition-shadow": "transition:box-shadow .15s", "text-muted": "color:var(--c-muted)",
    "text-white": "color:var(--c-on-solid)", "top-5": "top:20px", "top-10": "top:40px", "top-20": "top:80px",
    "animate-spin": "animation:hx-spin 1s linear infinite", "grid-cols-1": "grid-template-columns:repeat(1,minmax(0,1fr))",
    "grid-cols-2": "grid-template-columns:repeat(2,minmax(0,1fr))", "grid-cols-4": "grid-template-columns:repeat(4,minmax(0,1fr))",
}


def rule(cls):
    """CSS declarations for one utility class, or None when it is not a utility we know."""
    if cls in FIXED:
        return FIXED[cls]
    m = re.fullmatch(r"([pm])([xytblr]?)-([\d.]+)", cls)
    if m:
        prop = {"p": "padding", "m": "margin"}[m.group(1)]
        v = SPACE.get(m.group(3))
        if v is None:
            return None
        sides = {"": [""], "x": ["-left", "-right"], "y": ["-top", "-bottom"], "t": ["-top"], "b": ["-bottom"], "l": ["-left"], "r": ["-right"]}[m.group(2)]
        return ";".join(f"{prop}{s}:{v}" for s in sides)
    m = re.fullmatch(r"gap-(\d+)", cls)
    if m:
        return f"gap:{SPACE[m.group(1)]}"
    m = re.fullmatch(r"space-([xy])-(\d+)", cls)
    if m:
        return None  # emitted as a child combinator rule below
    m = re.fullmatch(r"([wh])-(.+)", cls)
    if m and m.group(2) in SIZE:
        return f"{'width' if m.group(1) == 'w' else 'height'}:{SIZE[m.group(2)]}"
    m = re.fullmatch(r"max-w-(.+)", cls)
    if m and m.group(1) in MAXW:
        return f"max-width:{MAXW[m.group(1)]}"
    m = re.fullmatch(r"text-(xs|sm|md|lg|xl|2xl|4xl|6xl)", cls)
    if m:
        return f"font-size:{FS[m.group(1)]}"
    m = re.fullmatch(r"text-(gray|blue|indigo|purple|green|red|yellow)-(\d+)", cls)
    if m:
        return f"color:{text_color(m.group(1), m.group(2))}"
    m = re.fullmatch(r"bg-(white|black)", cls)
    if m:
        return f"background-color:{bg_color(m.group(1), None)}"
    m = re.fullmatch(r"bg-(gray|blue|indigo|purple|green|red|yellow)-(\d+)", cls)
    if m:
        d = f"background-color:{bg_color(m.group(1), m.group(2))}"
        if int(m.group(2)) >= 500:
            d += ";color:var(--c-on-solid)"
        return d
    m = re.fullmatch(r"border-(gray|blue|green|red|yellow)-(\d+)", cls)
    if m:
        return f"border-color:{border_color(m.group(1), m.group(2))}"
    m = re.fullmatch(r"divide-gray-\d+", cls)
    if m:
        return None
    return None


def main():
    used = set()
    for dp, _dn, fns in os.walk(SRC):
        for fn in fns:
            if fn.endswith(".rs"):
                for m in re.finditer(r'class="([^"]*)"', open(os.path.join(dp, fn), encoding="utf-8").read()):
                    used.update(m.group(1).split())
    out = ["/* GENERATED by style/gen_utilities.py from the class names in src/ - do not edit by hand.",
           "   Utility names (Tailwind-style) mapped onto the UI system tokens (docs/design/ui-system.md). */",
           "@keyframes hx-spin { to { transform: rotate(360deg); } }"]
    variants = {"hover": ":hover", "focus": ":focus-visible", "disabled": ":disabled"}
    bps = {"md": 768, "lg": 1100}
    media = {k: [] for k in bps}
    for cls in sorted(used):
        prefix, base = (cls.split(":", 1) + [None])[:2] if ":" in cls else (None, cls)
        if prefix and prefix not in variants and prefix not in bps:
            continue
        sel = "." + re.sub(r"([:/.\[\]])", r"\\\1", cls)
        if prefix in variants:
            if base == "ring-2" or base == "ring-blue-500":
                continue
            d = rule(base.replace("[", "\\[").replace("]", "\\]")) if base else None
            if d:
                out.append(f"{sel}{variants[prefix]} {{ {d}; }}")
            continue
        if prefix in bps:
            m = re.fullmatch(r"grid-cols-(\d)", base)
            if m:
                media[prefix].append(f"{sel} {{ grid-template-columns:repeat({m.group(1)},minmax(0,1fr)); }}")
            elif base == "col-span-2":
                media[prefix].append(f"{sel} {{ grid-column:span 2 / span 2; }}")
            elif base == "w-1/3":
                media[prefix].append(f"{sel} {{ width:33.333%; }}")
            continue
        m = re.fullmatch(r"space-([xy])-(\d+)", cls)
        if m:
            side = "left" if m.group(1) == "x" else "top"
            out.append(f"{sel} > * + * {{ margin-{side}:{SPACE[m.group(2)]}; }}")
            continue
        if cls == "divide-y":
            out.append(".divide-y > * + * { border-top:1px solid var(--c-border); }")
            continue
        d = rule(cls.replace("[", "\\[").replace("]", "\\]")) if "[" in cls else rule(cls)
        if d:
            out.append(f"{sel} {{ {d}; }}")
    for bp, rules in media.items():
        if rules:
            out.append(f"@media (min-width: {bps[bp]}px) {{")
            out.extend("    " + r for r in rules)
            out.append("}")
    # a dark overlay (fixed inset-0 bg-black bg-opacity-50) is a modal scrim
    out.append(".fixed.inset-0.bg-black { background-color: rgba(0,0,0,.5); }")
    open(os.path.join(HERE, "utilities.css"), "w").write("\n".join(out) + "\n")
    print(f"utilities.css: {len(out)} lines from {len(used)} class names")


if __name__ == "__main__":
    main()
