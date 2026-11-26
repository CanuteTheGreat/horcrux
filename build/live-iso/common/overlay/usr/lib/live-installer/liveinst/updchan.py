"""Update channel chosen at install time (profile-driven; products without `update_channels` never show it).

Profile keys:
  update_channels      = ["stable", "testing", "development"]   first entry = the default
  update_channel_file  = "/etc/<product>/<product>.toml"         the product's own config file, section [update], key channel
  update_channel_info  = { stable = "tagged releases", ... }      optional one-line description per channel
Answer file: [update] channel = "testing"
"""
import re


def channels(profile):
    return [str(x) for x in (profile.get("update_channels") or [])]


def enabled(profile):
    return bool(channels(profile)) and bool(profile.get("update_channel_file"))


def default(profile):
    ch = channels(profile)
    return ch[0] if ch else ""


def problem(profile, ch):
    if not enabled(profile):
        return None if not ch else "this product has no update channel choice"
    if ch not in channels(profile):
        return f"update channel {ch!r}: one of {', '.join(channels(profile))}"
    return None


def options(profile):
    info = profile.get("update_channel_info", {}) or {}
    return [(c, f"{c} - {info[c]}" if c in info else c) for c in channels(profile)]


def from_answers(c, a, profile):
    u = a.get("update", {}) or {}
    c["update_channel"] = str(u.get("channel", "") or default(profile)) if enabled(profile) else str(u.get("channel", "") or "")


def apply_toml(text, ch):
    """`channel = "<ch>"` in the [update] section of a TOML text: replaces an existing (or commented-out) channel line in that
    section, else adds it right under the header, else appends the section. Everything else is kept byte for byte."""
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines) if l.strip() == "[update]"), None)
    new = f'channel = "{ch}"            # written by the installer (stable | testing | development)'
    if start is None:
        return text.rstrip("\n") + ("\n\n" if text.strip() else "") + "[update]\n" + new + "\n"
    end = next((i for i in range(start + 1, len(lines)) if re.match(r"\s*\[", lines[i])), len(lines))
    for i in range(start + 1, end):
        if re.match(r"\s*channel\s*=", lines[i]):
            lines[i] = new
            return "\n".join(lines) + "\n"
    for i in range(start + 1, end):
        if re.match(r"\s*#\s*channel\s*=", lines[i]):
            lines.insert(i + 1, new)
            return "\n".join(lines) + "\n"
    lines.insert(start + 1, new)
    return "\n".join(lines) + "\n"


def read_toml(text):
    m = re.search(r'(?ms)^\[update\]\s*$(.*?)(?=^\s*\[|\Z)', text)
    if not m:
        return ""
    v = re.search(r'(?m)^\s*channel\s*=\s*"([^"]*)"', m.group(1))
    return v.group(1) if v else ""
