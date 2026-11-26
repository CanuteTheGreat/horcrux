#!/usr/bin/env python3
"""Visual regression snapshots of the key pages (docs/design/ui-system.md "Verification").

ux_crawl.py --shots DIR writes one full-page PNG per route and combo (light/dark x desktop/wide/phone).
This keeps the first screen of the KEY pages as committed baselines in docs/qa/ui-shots/ and reports how much
each page changed against them.

  ui_snapshots.py --shots DIR --write      refresh the baselines (commit them with the UI change)
  ui_snapshots.py --shots DIR [--report F] compare; prints one line per page, writes a JSON report

The comparison is a report, not a gate: live numbers (CPU %, uptime, host names) differ on every run. A page
whose changed-pixel share jumps (default > 25 %) is listed as CHANGED so a reviewer opens its pair.
Needs Pillow."""
import argparse, json, os, sys

KEY = ["home", "vms", "vms-vm-101", "storage", "resources", "tasks", "system-health", "system-logs", "monitoring",
       "auth-users", "containers", "backup", "network", "ha"]
COMBOS = ["light-desktop", "dark-desktop", "light-phone", "dark-phone"]
VIEW = {"desktop": (1280, 800), "phone": (390, 844)}


def first_screen(path, combo):
    from PIL import Image
    im = Image.open(path).convert("RGB")
    w, h = VIEW[combo.split("-")[1]]
    return im.crop((0, 0, min(w, im.width), min(h, im.height)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--shots", required=True)
    ap.add_argument("--baseline", default=os.path.join(os.path.dirname(__file__), "..", "..", "docs", "qa", "ui-shots"))
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--report")
    ap.add_argument("--changed", type=float, default=25.0)
    a = ap.parse_args()
    try:
        from PIL import Image, ImageChops
    except ImportError:
        print("ui_snapshots: Pillow missing, skipped")
        return 0
    rows = []
    for combo in COMBOS:
        for page in KEY:
            src = os.path.join(a.shots, combo, page + ".png")
            if not os.path.exists(src):
                continue
            cur = first_screen(src, combo)
            dst = os.path.join(a.baseline, combo, page + ".png")
            if a.write:
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                cur.quantize(colors=128, method=Image.Quantize.MEDIANCUT).save(dst, optimize=True)
                rows.append({"combo": combo, "page": page, "written": dst})
                continue
            if not os.path.exists(dst):
                rows.append({"combo": combo, "page": page, "state": "NEW"})
                continue
            base = Image.open(dst).convert("RGB")
            if base.size != cur.size:
                rows.append({"combo": combo, "page": page, "state": "CHANGED", "pct": 100.0, "why": f"size {base.size} -> {cur.size}"})
                continue
            diff = ImageChops.difference(base, cur.quantize(colors=128, method=Image.Quantize.MEDIANCUT).convert("RGB")).convert("L")
            hist = diff.histogram()
            changed = sum(hist[24:])  # ignore quantization noise
            pct = 100.0 * changed / (cur.width * cur.height)
            rows.append({"combo": combo, "page": page, "state": "CHANGED" if pct > a.changed else "same", "pct": round(pct, 1)})
    for r in rows:
        print(f"{r.get('state', 'written'):8} {r['combo']:14} {r['page']:16} {r.get('pct', '')}")
    if a.report:
        with open(a.report, "w") as f:
            json.dump(rows, f, indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
