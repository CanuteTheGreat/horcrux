"""Graphical installer front-end: GTK4 (PyGObject) under a cage kiosk.

One screen per stage (stages.py supplies the declarative fields; this file only renders them), a step
sidebar, Back/Next, 'Advanced options' expanders, a live disk-layout preview, live link state per NIC,
the USE-flag page with the Portage sanity check, high-contrast and larger-text switches, HiDPI scaling,
an on-screen keyboard for touch-only machines.  Mouse, keyboard (Alt+Left / Alt+Right, Enter, Esc) and touch.

Exit protocol (see /usr/libexec/live-installer-session): run() returns 'reboot' | 'shell' | 'abort' |
'fallback' (no usable display -> the caller starts the text installer) | 'xkb' (keyboard layout changed:
the session restarts the compositor with the new layout and resumes at the next screen).
"""
import json
import os
import re
import sys
import threading
import time

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk, Pango  # noqa: E402

from . import stages as ST  # noqa: E402
from .stages import Field  # noqa: E402
from . import system as S  # noqa: E402
from . import useflags as UF  # noqa: E402
from . import app as APP  # noqa: E402

RESUME = "/run/live-installer/gui-resume.json"
STATUS = "/run/live-installer/gui-status.json"        # written on every page change (test harness / support)

# console keymap name -> (xkb layout, variant)
XKB = {"us": ("us", ""), "uk": ("gb", ""), "de": ("de", ""), "fr": ("fr", ""), "es": ("es", ""), "it": ("it", ""),
       "se": ("se", ""), "no": ("no", ""), "dk": ("dk", ""), "fi": ("fi", ""), "pt-latin1": ("pt", ""),
       "br-abnt2": ("br", ""), "dvorak": ("us", "dvorak"), "colemak": ("us", "colemak"), "us-acentos": ("us", "intl")}


def xkb_for(km):
    return XKB.get(km, (km.split("-")[0], ""))


def css(scale, accent, hc, large):
    f = 13.5 * scale * (1.35 if large else 1.0)
    px = lambda n: int(round(n * scale * (1.2 if large else 1.0)))   # noqa: E731
    if hc:
        bg, fg, panel, side, dim, line, acc, acc_fg = "#000000", "#ffffff", "#000000", "#000000", "#ffffff", "#ffffff", "#ffff00", "#000000"
        err, warn, ok = "#ff6b6b", "#ffd60a", "#4cff4c"
    else:
        bg, fg, panel, side, dim, line, acc, acc_fg = "#f4f5f7", "#1c1f24", "#ffffff", "#20242b", "#59606b", "#cfd4dc", accent, "#ffffff"
        err, warn, ok = "#b42318", "#8a5a00", "#18794e"
    return f"""
* {{ font-family: "DejaVu Sans", sans-serif; }}
window, .page {{ background: {bg}; color: {fg}; font-size: {f:.1f}px; }}
.side {{ background: {side}; color: {'#ffffff' if True else fg}; padding: {px(14)}px {px(10)}px; }}
.side label {{ color: #ffffff; font-size: {f:.1f}px; padding: {px(7)}px {px(8)}px; }}
.side .brand {{ font-size: {f * 1.7:.1f}px; font-weight: 800; padding: {px(4)}px {px(8)}px; }}
.side .sub {{ color: {'#ffffff' if hc else '#aeb6c2'}; font-size: {f * 0.85:.1f}px; padding: 0 {px(8)}px {px(14)}px; }}
.logo {{ background: {acc}; color: {acc_fg}; border-radius: {px(10)}px; font-size: {f * 1.9:.1f}px; font-weight: 800;
        min-width: {px(46)}px; min-height: {px(46)}px; }}
.step {{ color: {'#ffffff' if hc else '#aeb6c2'}; }}
.step.current {{ color: {'#000000' if hc else '#ffffff'}; background: {acc if hc else 'rgba(255,255,255,0.14)'}; border-radius: {px(8)}px; font-weight: 700; }}
.step.done {{ color: {'#ffffff' if hc else '#c9d1dc'}; }}
.title {{ font-size: {f * 1.7:.1f}px; font-weight: 800; }}
.intro, .hint {{ color: {dim}; }}
.hint {{ font-size: {f * 0.85:.1f}px; }}
.flabel {{ font-weight: 700; }}
.card {{ background: {panel}; border: 1px solid {line}; border-radius: {px(10)}px; padding: {px(14)}px; }}
button {{ min-height: {px(40)}px; min-width: {px(40)}px; padding: {px(4)}px {px(16)}px; }}
button.suggested-action {{ background: {acc}; color: {acc_fg}; font-weight: 700; }}
button.destructive-action {{ background: {err if hc else '#c0392b'}; color: {'#000000' if hc else '#ffffff'}; font-weight: 700; }}
entry, dropdown, passwordentry {{ min-height: {px(40)}px; }}
checkbutton, radiobutton {{ min-height: {px(36)}px; }}
check, radio {{ min-width: {px(22)}px; min-height: {px(22)}px; }}
togglebutton:checked {{ background: {acc}; color: {acc_fg}; }}
.hc button, .hc entry, .hc passwordentry, .hc dropdown, .hc textview, .hc list, .hc row {{ background: #000000; color: #ffffff; border: 2px solid #ffffff; }}
.hc button.suggested-action {{ background: #ffff00; color: #000000; }}
.hc check, .hc radio {{ background: #000000; border: 2px solid #ffffff; color: #000000; }}
.hc check:checked, .hc radio:checked {{ background: #ffff00; color: #000000; }}
.hc scrolledwindow, .hc .card, .hc .modal {{ background: #000000; border: 2px solid #ffffff; }}
.hc .side {{ border-right: 2px solid #ffffff; }}
.hc label {{ color: #ffffff; }}
.hc .hint, .hc .intro {{ color: #ffffff; }}
.hc .step.current {{ color: #000000; }}
.err {{ color: {err}; font-weight: 700; }}
.warn {{ color: {warn}; }}
.ok {{ color: {ok}; }}
.dot {{ font-size: {f * 1.3:.1f}px; }}
.dot.up {{ color: {ok}; }} .dot.down {{ color: {dim}; }} .dot.wifi {{ color: #2f80ed; }}
.seg {{ color: #ffffff; font-size: {f * 0.8:.1f}px; padding: {px(8)}px {px(4)}px; border-right: 2px solid {bg}; }}
.seg-esp {{ background: #c2410c; }} .seg-boot {{ background: #1d4ed8; }} .seg-swap {{ background: #7e22ce; }} .seg-root {{ background: {acc if not hc else '#1d4ed8'}; }}
.seg-old {{ background: #6b7280; }}
.issue {{ border-left: {px(5)}px solid {line}; padding: {px(6)}px {px(10)}px; }}
.issue.error {{ border-left-color: {err}; }} .issue.warning {{ border-left-color: {warn}; }} .issue.info {{ border-left-color: #2f80ed; }}
.grp {{ font-weight: 800; padding: {px(10)}px {px(6)}px {px(2)}px; color: {dim}; }}
.mono {{ font-family: "DejaVu Sans Mono", monospace; font-weight: 700; }}
.overlay-dim {{ background: rgba(0,0,0,0.55); }}
.modal {{ background: {panel}; color: {fg}; border-radius: {px(12)}px; padding: {px(22)}px; border: 2px solid {line}; }}
.osk button {{ min-width: {px(46)}px; padding: {px(2)}px {px(6)}px; }}
.bar {{ background: {panel}; border-top: 1px solid {line}; padding: {px(10)}px {px(18)}px; }}
.top {{ background: {panel}; border-bottom: 1px solid {line}; padding: {px(6)}px {px(14)}px; }}
progressbar trough, progressbar progress {{ min-height: {px(20)}px; }}
textview {{ font-family: "DejaVu Sans Mono", monospace; font-size: {f * 0.82:.1f}px; }}
"""


def monitor_scale():
    try:
        env = os.environ.get("LIVEINST_SCALE")
        if env:
            return float(env)
        mons = Gdk.Display.get_default().get_monitors()
        m = mons.get_item(0)
        g = m.get_geometry()
        mm = m.get_width_mm()
        dpi = g.width / (mm / 25.4) if mm else 96
        sc = max(m.get_scale_factor(), 1)
        base = 1.0
        if g.width >= 3000 or dpi >= 200:
            base = 2.0
        elif g.width >= 2200 or dpi >= 150:
            base = 1.5
        elif g.width >= 1700:
            base = 1.25
        return max(base, float(sc))
    except Exception:  # noqa
        return 1.0


def autoname(widget):
    """GTK 4.20 leaves a Button's accessible name empty (only the label inside it is named): give every labelled
    button its label as accessible name, so assistive technology and the AT-SPI test probe can find it."""
    try:
        if isinstance(widget, Gtk.Button) and widget.get_label():
            a11y(widget, widget.get_label())
        child = widget.get_first_child()
        while child is not None:
            autoname(child)
            child = child.get_next_sibling()
    except Exception:  # noqa
        pass


def lbl(text="", css_class=None, wrap=True, xalign=0.0, markup=False):
    l = Gtk.Label()
    if markup:
        l.set_markup(text)
    else:
        l.set_text(text)
    l.set_wrap(wrap)
    l.set_xalign(xalign)
    l.set_halign(Gtk.Align.FILL if wrap else Gtk.Align.START)
    if wrap:
        l.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    for c in (css_class or "").split():
        l.add_css_class(c)
    return l


def a11y(widget, label):
    try:
        widget.update_property([Gtk.AccessibleProperty.LABEL], [label])
    except Exception:  # noqa
        pass


# ==========================================================================
# generic form built from stages.Field
# ==========================================================================
class FormView(Gtk.Box):
    """Renders a list of stages.Field: one row per field, 'Advanced options' expander, live visibility + validation."""

    def __init__(self, win, fields, on_change=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=win.px(10))
        self.win, self.fields, self.on_change = win, fields, on_change
        self.rows = {}        # key -> (container, getter, setter-update)
        self.entries = {}
        self._lock = False
        adv_box = None
        for f in fields:
            row = self.build_row(f)
            if f.advanced:
                if adv_box is None:
                    adv_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=win.px(10))
                    self.expander = Gtk.Expander(label="Advanced options")
                    self.expander.add_css_class("flabel")
                    self.expander.set_child(adv_box)
                    self.expander.set_margin_top(win.px(8))
                    self.adv_box = adv_box
                adv_box.append(row)
            else:
                self.append(row)
            self.rows[f.key] = row
        if adv_box is not None:
            self.append(self.expander)
        self.refresh()

    # -- building --------------------------------------------------------------
    def build_row(self, f):
        win = self.win
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=win.px(4))
        if f.kind == "label":
            l = lbl(f.label, "warn" if f.hint == "warn" else "intro")
            box.append(l)
            return box
        if f.kind == "info":
            l = lbl(f.label, "flabel")
            box.append(l)
            f._w = lbl("", None)
            box.append(f._w)
            return box
        if f.kind == "check":
            w = Gtk.CheckButton(label=f.label)
            w.set_active(bool(f.value))
            w.add_css_class("flabel")
            w.connect("toggled", lambda b, f=f: self.changed(f, b.get_active()))
            a11y(w, f.label)
            f._w = w
            box.append(w)
        elif f.kind == "multi":
            box.append(lbl(f.label, "flabel"))
            f._ws = {}
            for v, t in f.options:
                cb = Gtk.CheckButton(label=t)
                cb.set_active(v in f.value)
                cb.connect("toggled", lambda b, f=f, v=v: self.multi_changed(f, v, b.get_active()))
                f._ws[v] = cb
                box.append(cb)
        else:
            box.append(lbl(f.label, "flabel"))
            if f.kind in ("text", "password"):
                w = Gtk.PasswordEntry(show_peek_icon=True) if f.kind == "password" else Gtk.Entry()
                w.set_text(str(f.value))
                w.set_hexpand(False)
                w.set_size_request(win.px(min(max(f.width, 20), 60) * 9), -1)
                w.connect("changed", lambda e, f=f: self.changed(f, e.get_text()))
                w.connect("activate", lambda e: win.go_next())
                a11y(w, f.label)
                f._w = w
                box.append(w)
            elif f.kind == "choice" and len(f.options) <= 4:
                grp = None
                f._ws = {}
                short = sum(len(t) for _v, t in f.options) < 60
                holder = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL if short else Gtk.Orientation.VERTICAL, spacing=win.px(18))
                box.append(holder)
                for v, t in f.options:
                    rb = Gtk.CheckButton(label=t)
                    if grp is None:
                        grp = rb
                    else:
                        rb.set_group(grp)
                    rb.set_active(v == f.value)
                    if v in f.disabled:
                        rb.set_sensitive(False)
                    rb.connect("toggled", lambda b, f=f, v=v: b.get_active() and self.changed(f, v))
                    f._ws[v] = rb
                    holder.append(rb)
            else:       # list / long choice: a button that opens the in-window searchable picker (keyboard, mouse and touch)
                vals = [v for v, _t in f.options]
                texts = dict(f.options)
                btn = Gtk.Button(label=texts.get(f.value, str(f.value)))
                btn.set_hexpand(False)
                btn.set_halign(Gtk.Align.START)
                btn.set_size_request(win.px(420), -1)
                btn.get_child().set_xalign(0.0) if hasattr(btn.get_child(), "set_xalign") else None
                a11y(btn, texts.get(f.value, str(f.value)))

                def picked(v, f=f, btn=btn, texts=texts):
                    btn.set_label(texts.get(v, str(v)))
                    a11y(btn, texts.get(v, str(v)))
                    self.changed(f, v)
                btn.connect("clicked", lambda b, f=f, picked=picked: win.picker(f.label, f.options, f.value, picked))
                f._w = btn
                box.append(btn)
        if f.hint and f.hint != "warn":
            box.append(lbl(f.hint, "hint"))
        return box

    # -- events ----------------------------------------------------------------
    def changed(self, f, v):
        if self._lock:
            return
        f.value = v
        self.refresh()
        if self.on_change:
            self.on_change(f)

    def multi_changed(self, f, v, on):
        if self._lock:
            return
        f.value = [x for x in f.value if x != v] + ([v] if on else [])
        if self.on_change:
            self.on_change(f)

    def refresh(self):
        vals = ST.form_values(self.fields)
        for f in self.fields:
            row = self.rows[f.key]
            vis = f.visible is None or f.visible(vals)
            row.set_visible(bool(vis))
            if f.kind == "info" and vis:
                v = f.value(vals) if callable(f.value) else f.value
                f._w.set_text(str(v))

    def problem(self, stage, ctx):
        return ST.first_problem(stage, ctx, self.fields)

    def focus_first(self):
        for f in self.fields:
            w = getattr(f, "_w", None)
            if w is not None and f.kind in ("text", "password") and self.rows[f.key].get_visible():
                w.grab_focus()
                return


# ==========================================================================
# pages
# ==========================================================================
class Page:
    id = ""
    title = ""
    intro = ""
    nav_next = "Next"
    can_back = True

    def __init__(self, win, stage=None):
        self.win, self.stage = win, stage
        self.widget = None
        self.sidebar = stage.short if stage else self.id.capitalize()

    def build(self):
        return Gtk.Box()

    def on_show(self):
        pass

    def problem(self):
        return None

    def commit(self):
        pass

    def status(self):
        return {}


class FormPage(Page):
    def __init__(self, win, stage):
        super().__init__(win, stage)
        self.id = stage.id
        self.title, self.intro = stage.title, stage.intro

    def build(self):
        self.fields = self.stage.fields(self.win.ctx)
        self.view = FormView(self.win, self.fields, self.on_change)
        return self.view

    def on_change(self, f):
        pass

    def on_show(self):
        self.view.focus_first()

    def problem(self):
        r = self.view.problem(self.stage, self.win.ctx)
        if r:
            f, msg = r
            return msg
        return None

    def commit(self):
        vals = ST.form_values(ST.visible_fields(self.fields))
        self.stage.commit(self.win.ctx, {f.key: f.value for f in self.fields})


class KeymapPage(FormPage):
    def commit(self):
        super().commit()
        S.apply_live_keymap(self.win.c["keymap"])


class DiskPage(Page):
    id = "disk"

    def __init__(self, win, stage):
        super().__init__(win, stage)
        self.title, self.intro = stage.title, stage.intro
        self.sel = list(win.c["disks"])
        self.cur = None

    def build(self):
        w = self.win
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(12))
        self.fields = self.stage.fields(w.ctx)
        if len(w.p["filesystems"]) <= 1:
            self.title, self.intro = "Target disk", "Choose the disk to install to. Everything on a selected disk will be ERASED."
            self.fields = [f for f in self.fields if f.key != "fs"]
            self.fields[0].visible = None if w.c["fs"] == "zfs" else (lambda v: False)
        self.view = FormView(w, self.fields, self.fs_changed)
        outer.append(self.view)
        outer.append(lbl("Disks", "flabel"))
        self.dbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        sc = Gtk.ScrolledWindow()
        sc.set_min_content_height(w.px(150))
        sc.set_child(self.dbox)
        sc.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        outer.append(sc)
        self.preview = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        self.preview.add_css_class("card")
        outer.append(self.preview)
        self.rebuild_disks()
        return outer

    def need(self):
        return self.stage.need_disks(self.win.c if not hasattr(self, "fields") else {
            "fs": next((f.value for f in self.fields if f.key == "fs"), self.win.c["fs"]),
            "zfs_raid": next((f.value for f in self.fields if f.key == "zfs_raid"), self.win.c["zfs_raid"])})

    def fs_changed(self, f):
        self.rebuild_disks()

    def rebuild_disks(self):
        w = self.win
        while (ch := self.dbox.get_first_child()) is not None:
            self.dbox.remove(ch)
        disks = w.ctx.disks
        multi = self.need() == 2
        self.sel = [d for d in self.sel if any(x["path"] == d for x in disks)]
        if not multi and len(self.sel) > 1:
            self.sel = self.sel[:1]
        grp = None
        self.checks = {}
        for d in disks:
            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(10))
            cb = Gtk.CheckButton()
            if not multi:
                if grp is None:
                    grp = cb
                else:
                    cb.set_group(grp)
            cb.set_active(d["path"] in self.sel)
            cb.connect("toggled", lambda b, p=d["path"]: self.toggled(p, b.get_active()))
            text = f"{d['path']}   {S.fmt_size(d['size'])}   {d['kind']}   {d['model']}"
            a11y(cb, "Disk " + text)
            row.append(cb)
            l = lbl(text, "mono", wrap=False)
            row.append(l)
            parts = ", ".join(f"{p['fstype'] or '?'} {S.fmt_size(p['size'])}" for p in d["parts"][:4]) or "empty"
            row.append(lbl("   existing: " + parts, "hint", wrap=False))
            click = Gtk.GestureClick()
            click.connect("pressed", lambda g, n, x, y, p=d["path"]: self.focus(p))
            row.add_controller(click)
            self.checks[d["path"]] = cb
            self.dbox.append(row)
        self.draw_preview()

    def focus(self, path):
        self.cur = path
        self.draw_preview()

    def toggled(self, path, on):
        multi = self.need() == 2
        if on:
            if path not in self.sel:
                self.sel = (self.sel + [path]) if multi else [path]
        else:
            self.sel = [x for x in self.sel if x != path]
        self.cur = path
        self.draw_preview()
        self.win.update_nav()

    def draw_preview(self):
        w = self.win
        while (ch := self.preview.get_first_child()) is not None:
            self.preview.remove(ch)
        disks = {d["path"]: d for d in w.ctx.disks}
        path = self.cur if self.cur in disks else (self.sel[0] if self.sel else (w.ctx.disks[0]["path"] if w.ctx.disks else None))
        if not path:
            return
        d = disks[path]
        fsv = next((f.value for f in self.fields if f.key == "fs"), w.c["fs"])
        cfg = dict(w.c, fs=fsv)
        segs = ST.disk_layout(cfg, w.p, d["size"], w.ctx.efi, w.ctx.ram_mb)
        mark = "will be erased" if path in self.sel else "not selected"
        self.preview.append(lbl(f"Layout written to {path} ({S.fmt_size(d['size'])}) - {mark}", "flabel"))
        total_px = w.px(760)
        self.preview.append(self.bar([(n, b, f"seg-{k}", f"{n}\n{S.fmt_size(b)}\n{fs}") for (n, b, fs, _m), k in zip(segs, [{"ESP": "esp", "BIOS boot": "esp", "boot": "boot", "swap": "swap"}.get(sg[0], "root") for sg in segs])], d["size"], total_px))
        if d["parts"]:
            self.preview.append(lbl("Existing contents (destroyed):", "hint"))
            self.preview.append(self.bar([(p["path"], p["size"], "seg-old", f"{os.path.basename(p['path'])}\n{S.fmt_size(p['size'])}\n{p['fstype'] or '-'}") for p in d["parts"]], d["size"], total_px))
        else:
            self.preview.append(lbl("The disk holds no partitions.", "hint"))

    def bar(self, segs, total, px_total):
        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        row.set_halign(Gtk.Align.START)
        for name, size, klass, text in segs:
            wpx = max(self.win.px(70), int(px_total * size / max(total, 1)))
            l = lbl(text, "seg " + klass, wrap=False, xalign=0.5)
            l.set_size_request(wpx, -1)
            l.set_justify(Gtk.Justification.CENTER)
            l.set_ellipsize(Pango.EllipsizeMode.END)
            row.append(l)
        return row

    def problem(self):
        w = self.win
        w.ctx.disks = w.ctx.disks
        fsv = next((f.value for f in self.fields if f.key == "fs"), w.c["fs"])
        zr = next((f.value for f in self.fields if f.key == "zfs_raid"), w.c["zfs_raid"])
        vals = {f.key: f.value for f in self.fields}
        vals.update(fs=fsv, zfs_raid=zr)
        bad = self.stage.validate(w.ctx, vals) if fsv == "xfs-luks" else None
        if bad:
            return bad
        tmp = dict(w.c, fs=fsv, zfs_raid=zr)
        w2 = type("X", (), {"c": tmp, "disks": w.ctx.disks})
        err, warn = self.stage.check_selection(type("C", (), {"c": tmp, "disks": w.ctx.disks})(), self.sel)
        if err:
            return err
        if fsv == "zfs" and not w.dry and S.sh("modprobe zfs && test -e /dev/zfs")[0] != 0:
            return "The ZFS kernel module could not be loaded on this machine. Choose XFS."
        if warn:
            self.warn = warn
        return None

    def commit(self):
        vals = {"fs": self.win.c["fs"], "zfs_raid": self.win.c["zfs_raid"]}     # a product with one file system has no 'fs' field
        vals.update({f.key: f.value for f in self.fields})
        self.stage.commit(self.win.ctx, vals)
        self.win.c["disks"] = list(self.sel)


class NetPage(Page):
    id = "netconfig"

    def __init__(self, win, stage):
        super().__init__(win, stage)
        self.title, self.intro = stage.title, stage.intro
        self.timer = None

    def build(self):
        w = self.win
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(10))
        outer.append(lbl("Network interfaces", "flabel"))
        self.nbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        self.nbox.add_css_class("card")
        outer.append(self.nbox)
        c = w.c
        self.role = {}
        self.wanlan = w.ctx.wanlan
        if self.wanlan:
            if not c["wan"]["iface"] and w.nics:        # like the text installer: first wired NIC = WAN, second = LAN
                wired = [x for x in w.nics if not x["wifi"]]
                c["wan"]["iface"] = (wired[0] if wired else w.nics[0])["name"]
                if not c["lan"]["iface"] and len(wired) > 1:
                    c["lan"]["iface"] = wired[1]["name"]
            if c["wan"]["iface"]:
                self.role[c["wan"]["iface"]] = "wan"
            if c["lan"]["iface"]:
                self.role[c["lan"]["iface"]] = "lan"
        else:
            if not c["mgmt"]["iface"] and w.nics:
                wired = [x for x in w.nics if not x["wifi"]]
                up = [x for x in wired if S.carrier(x["name"])]
                c["mgmt"]["iface"] = (up or wired or w.nics)[0]["name"]
            self.role[c["mgmt"]["iface"]] = "mgmt"
        self.rows = {}
        self.build_rows()
        self.form_holder = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        outer.append(self.form_holder)
        self.wifi_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        self.wifi_box.add_css_class("card")
        outer.append(self.wifi_box)
        self.build_form()
        self.wifi_box.set_visible(False)
        return outer

    def build_rows(self):
        w = self.win
        if not w.nics:
            self.nbox.append(lbl("No network interface was found. You can still install; the web UI becomes reachable once you add a "
                                 "NIC (USB Ethernet works) or Wi-Fi.", "warn"))
            return
        grp = None
        for n in w.nics:
            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(10))
            dot = lbl("\u25cf", "dot", wrap=False)
            state = lbl("", None, wrap=False)
            if self.wanlan:
                dd = Gtk.DropDown.new_from_strings(["unused", "WAN", "LAN"])
                dd.set_selected({"wan": 1, "lan": 2}.get(self.role.get(n["name"]), 0))
                dd.connect("notify::selected", lambda d, _p, nm=n["name"]: self.set_role(nm, ["", "wan", "lan"][d.get_selected()]))
                a11y(dd, f"Role of {n['name']}")
                row.append(dd)
                sel = dd
            else:
                rb = Gtk.CheckButton()
                if grp is None:
                    grp = rb
                else:
                    rb.set_group(grp)
                rb.set_active(self.role.get(n["name"]) == "mgmt")
                rb.connect("toggled", lambda b, nm=n["name"]: b.get_active() and self.set_role(nm, "mgmt"))
                a11y(rb, f"Use {n['name']} as management interface")
                row.append(rb)
                sel = rb
            row.append(dot)
            nl = lbl(f"{n['name']:<10} {n['mac']}   {n['driver']} {n['model'][:26]}", "mono", wrap=False)
            nl.set_ellipsize(Pango.EllipsizeMode.END)       # a long NIC description must never make the window wider than the screen (large text)
            nl.set_hexpand(True)
            row.append(nl)
            row.append(state)
            ipl = lbl("", "hint", wrap=False)
            row.append(ipl)
            self.nbox.append(row)
            self.rows[n["name"]] = (dot, state, ipl)
        self.poll()

    def poll(self):
        for n in self.win.nics:
            dot, state, ipl = self.rows.get(n["name"], (None,) * 3)
            if dot is None:
                continue
            for k in ("up", "down", "wifi"):
                dot.remove_css_class(k)
            if n["wifi"]:
                dot.add_css_class("wifi")
                state.set_text("Wi-Fi")
            elif S.carrier(n["name"]):
                dot.add_css_class("up")
                state.set_text("link UP")
            else:
                dot.add_css_class("down")
                state.set_text("no link")
            ip = S.nic_ip(n["name"])
            ipl.set_text(("  " + ip) if ip else "")
        return True

    def start_poll(self):
        if self.timer is None:
            self.timer = GLib.timeout_add(1500, self.poll)

    def stop_poll(self):
        if self.timer is not None:
            GLib.source_remove(self.timer)
            self.timer = None

    def set_role(self, name, role):
        for k in [k for k, v in self.role.items() if v == role and role in ("wan", "lan", "mgmt") and k != name]:
            self.role[k] = None
        self.role[name] = role or None
        self.win.update_nav()
        self.build_form()

    def selected_iface(self):
        key = "mgmt" if not self.wanlan else "wan"
        for k, v in self.role.items():
            if v == key:
                return k
        return ""

    def build_form(self):
        w = self.win
        c = w.c
        while (ch := self.form_holder.get_first_child()) is not None:
            self.form_holder.remove(ch)
        if self.wanlan:
            wf = c["wan"]
            f = [Field("wmode", "WAN addressing", "choice", wf["mode"], [("dhcp", "DHCP (from the ISP / upstream)"), ("static", "Static address")]),
                 Field("waddr", "WAN address/prefix", "text", wf["address"], validate=lambda v, a: S.cidr_problem(v), visible=lambda v: v["wmode"] == "static", advanced=True),
                 Field("wgw", "WAN gateway", "text", wf["gateway"], validate=lambda v, a: S.ip_problem(v, a.get("waddr")), visible=lambda v: v["wmode"] == "static", advanced=True),
                 Field("wdns", "DNS server(s)", "text", wf["dns"], validate=lambda v, a: S.dns_problem(v), visible=lambda v: v["wmode"] == "static", advanced=True),
                 Field("laddr", "LAN address/prefix", "text", c["lan"]["address"], validate=lambda v, a: S.cidr_problem(v),
                       hint="The firewall's own LAN address; also the web UI address (a DHCP server for LAN clients can be enabled later in the web UI).")]
        else:
            n = c["mgmt"]
            f = [Field("mode", "IP configuration", "choice", n["mode"], [("dhcp", "DHCP (automatic)"), ("static", "Static address")]),
                 Field("address", "IP address / prefix", "text", n["address"] or S.nic_ip(self.selected_iface()), validate=lambda v, a: S.cidr_problem(v),
                       visible=lambda v: v["mode"] == "static", hint="e.g. 192.168.1.50/24", advanced=True),
                 Field("gateway", "Gateway", "text", n["gateway"] or APP.default_gateway(), validate=lambda v, a: S.ip_problem(v, a.get("address")),
                       visible=lambda v: v["mode"] == "static", advanced=True),
                 Field("dns", "DNS server(s)", "text", n["dns"] or APP.default_dns(), validate=lambda v, a: S.dns_problem(v),
                       visible=lambda v: v["mode"] == "static", hint="space separated", advanced=True)]
        self.fields = f
        self.view = FormView(w, f)
        self.form_holder.append(self.view)
        wifi = [x for x in w.nics if x["wifi"] and self.role.get(x["name"]) in ("mgmt", "wan")]
        self.build_wifi(wifi[0]["name"] if wifi else None)

    def build_wifi(self, iface):
        w = self.win
        while (ch := self.wifi_box.get_first_child()) is not None:
            self.wifi_box.remove(ch)
        self.wifi_iface = iface
        self.wifi_box.set_visible(bool(iface))
        if not iface:
            return
        self.wifi_box.append(lbl(f"Wi-Fi on {iface}", "flabel"))
        self.ssid = Gtk.Entry()
        self.ssid.set_placeholder_text("Network name (SSID)")
        self.psk = Gtk.PasswordEntry(show_peek_icon=True)
        self.psk.set_property("placeholder-text", "Passphrase (empty for an open network)")
        self.wifi_status = lbl("", "hint")
        btn = Gtk.Button(label="Scan and connect")
        btn.connect("clicked", lambda b: self.wifi_go())
        for x in (self.ssid, self.psk, btn, self.wifi_status):
            self.wifi_box.append(x)
        self.scan_list = Gtk.DropDown.new_from_strings(["(press Scan to list networks)"])
        self.wifi_box.append(self.scan_list)

    def wifi_go(self):
        ssid, psk, iface = self.ssid.get_text().strip(), self.psk.get_text(), self.wifi_iface
        if not ssid:
            self.wifi_status.set_text("Enter the network name first.")
            return
        self.wifi_status.set_text(f"Connecting to {ssid} ...")

        def work():
            ok, info = (True, "10.0.0.2") if self.win.dry else S.wifi_connect(iface, ssid, psk)
            GLib.idle_add(self.wifi_done, ok, info, ssid, psk)
        threading.Thread(target=work, daemon=True).start()

    def wifi_done(self, ok, info, ssid, psk):
        if ok:
            self.win.c["wifi"] = {"ssid": ssid, "psk": psk, "iface": self.wifi_iface}
            self.wifi_status.set_text(f"Connected, address {info}.")
        else:
            self.wifi_status.set_text(f"Could not connect: {info}")

    def on_show(self):
        self.start_poll()

    def problem(self):
        w, c = self.win, self.win.c
        if w.nics:
            if not self.selected_iface():
                return "Choose the " + ("WAN" if self.wanlan else "management") + " interface."
        elif self.wanlan:
            return "A firewall needs at least one network interface. Plug in a NIC (USB Ethernet works) and restart the installer."
        r = ST.first_problem(self.stage, w.ctx, self.fields)
        if r:
            return r[1]
        if self.wifi_iface and not c.get("wifi"):
            return "Connect to the Wi-Fi network first (Scan and connect)."
        return None

    def commit(self):
        c, v = self.win.c, ST.form_values(self.fields)
        if self.wanlan:
            lan_if = next((k for k, r in self.role.items() if r == "lan"), "")
            c["wan"].update({"iface": self.selected_iface(), "mode": "dhcp" if c.get("wifi") else v["wmode"],
                             "address": v.get("waddr", ""), "gateway": v.get("wgw", ""), "dns": v.get("wdns", "")})
            c["lan"].update({"iface": lan_if, "address": v["laddr"].strip()})
        else:
            c["mgmt"].update({"iface": self.selected_iface(), "mode": "dhcp" if c.get("wifi") else v["mode"],
                              "address": v.get("address", ""), "gateway": v.get("gateway", ""), "dns": v.get("dns", "")})
        wn = next((x for x in self.win.nics if x["name"] == self.selected_iface()), None)
        if not wn or not wn["wifi"]:
            c["wifi"] = None
        self.stop_poll()


class UsePage(Page):
    id = "use_flags"

    def __init__(self, win, stage):
        super().__init__(win, stage)
        self.title, self.intro = stage.title, stage.intro
        self.meta = UF.load_meta()
        win.ctx.use_meta = self.meta
        self.issues = []
        self._block = False

    def build(self):
        w = self.win
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(10))
        if not self.meta:
            outer.append(lbl("This image carries no Portage metadata, so USE flags cannot be edited. The tested image defaults are used.", "warn"))
            return outer
        # presets
        pr = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(8))
        self.pbtn = {}
        first = None
        for k, label, _d in UF.PRESETS:
            b = Gtk.ToggleButton(label=label)
            if first is None:
                first = b
            else:
                b.set_group(first)
            b.connect("toggled", lambda b, k=k: b.get_active() and self.preset(k))
            a11y(b, f"Preset {label}")
            pr.append(b)
            self.pbtn[k] = b
        outer.append(pr)
        self.pdesc = lbl("", "hint")
        outer.append(self.pdesc)
        # search + group
        sr = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(8))
        self.search = Gtk.SearchEntry()
        self.search.set_placeholder_text("Search flags, descriptions, packages")
        self.search.set_hexpand(True)
        self.search.connect("search-changed", lambda e: self.lb.invalidate_filter())
        a11y(self.search, "Search USE flags")
        sr.append(self.search)
        self.group_dd = Gtk.DropDown.new_from_strings(["All groups"] + UF.GROUPS)
        self.group_dd.connect("notify::selected", lambda d, _p: self.lb.invalidate_filter())
        a11y(self.group_dd, "Flag group")
        sr.append(self.group_dd)
        self.chg_only = Gtk.CheckButton(label="Changed only")
        self.chg_only.connect("toggled", lambda b: self.lb.invalidate_filter())
        sr.append(self.chg_only)
        outer.append(sr)
        # list + issues
        body = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(12))
        body.set_vexpand(True)
        self.lb = Gtk.ListBox()
        self.lb.set_selection_mode(Gtk.SelectionMode.NONE)
        self.lb.set_filter_func(self.filter_row)
        self.lb.set_header_func(self.header_row)
        self.rows = []
        grp_i = {g: i for i, g in enumerate(UF.GROUPS)}
        for r in sorted(self.meta.flag_rows(), key=lambda r: (grp_i.get(r["group"], 99), r["flag"])):
            self.rows.append(self.make_row(r))
        sc = Gtk.ScrolledWindow()
        sc.set_child(self.lb)
        sc.set_hexpand(True)
        sc.set_vexpand(True)
        sc.set_min_content_height(w.px(300))
        sc.set_min_content_width(w.px(520))
        body.append(sc)
        right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(8))
        right.set_size_request(w.px(380), -1)
        right.append(lbl("Portage sanity check", "flabel"))
        self.ibox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        isc = Gtk.ScrolledWindow()
        isc.set_child(self.ibox)
        isc.set_vexpand(True)
        isc.set_min_content_height(w.px(160))
        right.append(isc)
        right.append(lbl("Binary packages or source build?", "flabel"))
        self.impact = lbl("", None)
        right.append(self.impact)
        body.append(right)
        outer.append(body)
        self.sync_from_cfg()
        return outer

    # -- rows ----------------------------------------------------------------------
    def make_row(self, r):
        w = self.win
        row = Gtk.ListBoxRow()
        box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=w.px(10))
        cb = Gtk.CheckButton()
        a11y(cb, f"USE flag {r['flag']}: {r['desc']}")
        cb.connect("toggled", lambda b, f=r["flag"]: self.toggled(f, b))
        box.append(cb)
        txt = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        txt.append(lbl(r["flag"] + ("" if r["used"] else "   (not used by installed packages)"), "mono", wrap=False))
        d = lbl(r["desc"], "hint")
        txt.append(d)
        box.append(txt)
        row.set_child(box)
        row._r, row._cb = r, cb
        self.lb.append(row)
        return row

    def header_row(self, row, before):
        if before is None or before._r["group"] != row._r["group"]:
            row.set_header(lbl(row._r["group"], "grp"))
        else:
            row.set_header(None)

    def filter_row(self, row):
        r = row._r
        gi = self.group_dd.get_selected()
        if gi > 0 and r["group"] != UF.GROUPS[gi - 1]:
            return False
        if self.chg_only.get_active() and r["flag"] not in self.choices():
            return False
        q = self.search.get_text().lower().strip()
        if q and q not in r["flag"].lower() and q not in r["desc"].lower() and not any(q in p.lower() for p in r["packages"]):
            return False
        return True

    # -- state ------------------------------------------------------------------------
    def use(self):
        return self.win.c["use"]

    def choices(self):
        return UF.effective_choices(self.meta, self.win.p, self.use())

    def sync_from_cfg(self):
        self._block = True
        ch = self.choices()
        for row in self.rows:
            f, st = row._r["flag"], row._r["state"]
            v = ch.get(f, st)
            row._cb.set_inconsistent(v is None)
            row._cb.set_active(bool(v))
            if v is None:
                row._cb.set_inconsistent(True)
        name = self.use()["preset"]
        for k, b in self.pbtn.items():
            b.set_active(k == name)
        self.pdesc.set_text("  " + next((d for kk, _l, d in UF.PRESETS if kk == name), "Custom selection."))
        self._block = False
        self.recheck()

    def preset(self, k):
        if self._block:
            return
        u = self.use()
        u["preset"], u["flags"] = k, {}
        self.sync_from_cfg()
        self.lb.invalidate_filter()

    def toggled(self, flag, cb):
        if self._block:
            return
        u = self.use()
        flags = dict(self.choices())
        row_state = self.meta.state.get(flag)
        nv = cb.get_active()
        cb.set_inconsistent(False)
        if nv == row_state:
            flags.pop(flag, None)
        else:
            flags[flag] = nv
        u["flags"], u["preset"] = flags, ("custom" if flags else "recommended")
        self._block = True
        for k, b in self.pbtn.items():
            b.set_active(False)
        self.pdesc.set_text("  Custom selection.")
        self._block = False
        self.recheck()

    def recheck(self):
        w = self.win
        ch = self.choices()
        self.issues = UF.check(self.meta, w.p, ch)
        while (c := self.ibox.get_first_child()) is not None:
            self.ibox.remove(c)
        if not self.issues:
            self.ibox.append(lbl("\u2714 No problems found.", "ok"))
        order = {"error": 0, "warning": 1, "info": 2}
        for i in sorted(self.issues, key=lambda i: order[i.level]):
            tag = {"error": "\u2716 Error", "warning": "\u26a0 Warning", "info": "\u2139 Note"}[i.level]
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
            box.add_css_class("issue")
            box.add_css_class(i.level)
            box.append(lbl(tag, "err" if i.level == "error" else "flabel"))
            box.append(lbl(i.text, None))
            self.ibox.append(box)
        imp = UF.impact(self.meta, ch)
        txt = UF.summary_line(self.meta, ch)
        if imp["list"]:
            txt += "\n\nRebuild list: " + ", ".join(f"{p} (~{m} min)" for p, m in imp["list"][:8]) + (" ..." if len(imp["list"]) > 8 else "")
            txt += "\n\nThe installed system boots from binaries now; the rebuild happens later (emerge) on your command."
        self.impact.set_text(txt)
        w.update_nav()
        w.write_status()

    def problem(self):
        if self.meta and UF.has_errors(self.issues):
            n = sum(1 for i in self.issues if i.level == "error")
            return f"Fix {n} Portage error{'s' if n > 1 else ''} first (see the sanity check on the right)."
        return None

    def status(self):
        return {"issues": [[i.level, i.code, i.text] for i in self.issues]}


class ReviewPage(Page):
    id = "review"
    nav_next = "Install"

    def __init__(self, win):
        super().__init__(win)
        self.sidebar = "Review"
        self.title = "Review and install"
        self.intro = "Nothing has been written yet. Review the plan; the disk is erased when you press Install."

    def build(self):
        self.box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.win.px(8))
        return self.box

    def on_show(self):
        w = self.win
        while (c := self.box.get_first_child()) is not None:
            self.box.remove(c)
        w.ctx.disks = w.ctx.disks
        self.box.append(lbl(f"All data on {', '.join(w.c['disks'])} will be DESTROYED.", "err"))
        self.ack = Gtk.CheckButton(label=f"I understand that everything on {', '.join(w.c['disks'])} will be erased.")
        self.ack.connect("toggled", lambda b: w.update_nav())
        a11y(self.ack, "Confirm erasing the disk")
        self.box.append(self.ack)
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(4))
        card.add_css_class("card")
        for sid, lines in ST.full_plan(w.ctx):
            card.append(lbl(ST.BY_ID[sid].title, "flabel"))
            for l in lines:
                card.append(lbl("  \u2022 " + l, "hint" if False else None))
        self.box.append(card)

    def problem(self):
        return None if self.ack.get_active() else "Tick the box to confirm that the disk may be erased."


class ProgressPage(Page):
    id = "install"
    can_back = False

    def __init__(self, win):
        super().__init__(win)
        self.sidebar = "Install"
        self.title = "Installing"
        self.intro = "Do not power off. This takes a few minutes."
        self.state = {"pct": 0, "note": "Starting", "lines": [], "err": None, "done": False}
        self.started = False

    def build(self):
        w = self.win
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(10))
        self.bar = Gtk.ProgressBar()
        self.bar.set_show_text(True)
        self.note = lbl("Starting", "flabel")
        box.append(self.note)
        box.append(self.bar)
        exp = Gtk.Expander(label="Show details")
        self.tv = Gtk.TextView()
        self.tv.set_editable(False)
        self.tv.set_cursor_visible(False)
        sc = Gtk.ScrolledWindow()
        sc.set_child(self.tv)
        sc.set_min_content_height(w.px(240))
        exp.set_child(sc)
        self.sc = sc
        box.append(exp)
        return box

    def on_show(self):
        if self.started:
            return
        self.started = True
        w = self.win
        st = self.state
        inst = S.Installer(w.p, w.c, lambda l: st["lines"].append(l), lambda pct, note: st.update(pct=pct, note=note))
        inst._ctx = w.ctx

        def work():
            try:
                if w.dry:
                    for i in range(0, 101, 5):
                        st.update(pct=i, note="dry run")
                        st["lines"].append(f"fake step {i}")
                        time.sleep(float(os.environ.get("LIVEINST_DRY_DELAY", "0.05")))
                else:
                    inst.go()
            except Exception as e:  # noqa
                st["err"] = e
                st["lines"].append("ERROR: " + str(e))
            st["done"] = True
        threading.Thread(target=work, daemon=True).start()
        self.nlines = 0
        GLib.timeout_add(250, self.tick)

    def tick(self):
        st = self.state
        self.bar.set_fraction(st["pct"] / 100.0)
        self.bar.set_text(f"{st['pct']}%")
        self.note.set_text(st["note"])
        buf = self.tv.get_buffer()
        if len(st["lines"]) != self.nlines:
            buf.set_text("\n".join(st["lines"][-400:]))
            self.nlines = len(st["lines"])
            adj = self.sc.get_vadjustment()
            GLib.idle_add(lambda: adj.set_value(adj.get_upper()))
        self.win.write_status({"pct": st["pct"], "note": st["note"]})
        if st["done"]:
            if st["err"]:
                self.win.modal("Installation FAILED", f"{st['err']}\n\nThe log is /var/log/live-installer.log. You can drop to a shell.",
                               [("Shell", lambda: self.win.finish("shell"))], kind="err")
            else:
                self.win.advance()
            return False
        return True


class DonePage(Page):
    id = "finish"
    can_back = False

    def __init__(self, win):
        super().__init__(win)
        self.sidebar = "Finish"
        self.title = "Installation complete"

    def build(self):
        self.box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.win.px(8))
        return self.box

    def on_show(self):
        """built when the page is shown: the address, key and options are the ones chosen on the earlier screens"""
        w = self.win
        c, p = w.c, w.p
        while (ch := self.box.get_first_child()) is not None:
            self.box.remove(ch)
        url = APP.web_url(w.state)
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=w.px(6))
        card.add_css_class("card")
        ssh = "off" if not c["ssh_enabled"] else "root with " + ("your key" if c["ssh_keys"] else "NO key (console only)") + (" or password" if c["ssh_password_auth"] else "")
        for k, v in (("Web UI", url), ("Login", "admin  /  the password you just set"), ("SSH", ssh), ("Console", "root  /  the same password")):
            r = Gtk.Box(spacing=w.px(10))
            kl = lbl(k, "flabel", wrap=False)
            kl.set_size_request(w.px(110), -1)
            r.append(kl)
            r.append(lbl(v, "mono" if k == "Web UI" else None, wrap=False))
            card.append(r)
        self.box.append(card)
        self.box.append(lbl(f"{p['title']} has been installed. Remove the installation medium, then reboot.", "ok"))
        if "<" in url:
            self.box.append(lbl("The address comes from DHCP: it is shown on the console after boot and in your DHCP server's lease list.", "hint"))


# ==========================================================================
# window
# ==========================================================================
class Win(Gtk.ApplicationWindow):
    def __init__(self, app, profile, dry):
        super().__init__(application=app)
        self.p, self.dry = profile, dry
        self.state = APP.State(profile)
        self.c = self.state.c
        self.ctx = self.state.ctx
        self.ctx.dry = dry
        self.result = "abort"
        self.idx = 0
        self.nics = []
        self.hc = os.environ.get("LIVEINST_HC") == "1"
        self.large = os.environ.get("LIVEINST_LARGE") == "1"
        self.scale = monitor_scale()
        self.accent = profile.get("brand_color", "#5b3fa0")
        self.set_title(f"{profile['title']} installer")
        self.set_decorated(False)             # kiosk: no title bar / window buttons
        self.provider = Gtk.CssProvider()
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), self.provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.restyle()
        self.probe()
        self.build_ui()
        if os.environ.get("LIVEINST_SIZE"):          # dev/test without a window manager
            w_, h_ = os.environ["LIVEINST_SIZE"].split("x")
            self.set_default_size(int(w_), int(h_))
        # no fullscreen()/maximize() request: cage makes the window fullscreen itself, and cage 0.2.1 + wlroots 0.19
        # aborts ("xdg_surface->initialized") when a client asks for fullscreen at startup
        sc = Gtk.ShortcutController()
        sc.set_scope(Gtk.ShortcutScope.GLOBAL)
        for trig, fn in (("<Alt>Right", self.go_next), ("<Alt>Left", self.go_back)):
            sc.add_shortcut(Gtk.Shortcut.new(Gtk.ShortcutTrigger.parse_string(trig), Gtk.CallbackAction.new(lambda *a, fn=fn: fn() or True)))
        self.add_controller(sc)
        sc2 = Gtk.ShortcutController()
        sc2.add_shortcut(Gtk.Shortcut.new(Gtk.ShortcutTrigger.parse_string("Escape"), Gtk.CallbackAction.new(lambda *a: self.ask_abort() or True)))
        self.add_controller(sc2)
        self.idx = 0
        resume = self.load_resume()
        self.show_page(resume if resume is not None else 0)

    def px(self, n):
        return int(round(n * self.scale * (1.2 if self.large else 1.0)))

    def restyle(self):
        (self.add_css_class if self.hc else self.remove_css_class)("hc")
        text = css(self.scale, self.accent, self.hc, self.large)
        try:
            self.provider.load_from_string(text)
        except AttributeError:
            self.provider.load_from_data(text.encode())
        Gtk.Settings.get_default().set_property("gtk-cursor-theme-size", 24 if self.scale < 2 else 48)

    def probe(self):
        if self.dry:
            self.ctx.disks = [{"path": "/dev/vda", "name": "vda", "size": 64 * 10**9, "model": "QEMU HARDDISK", "kind": "SSD", "removable": False,
                               "parts": [{"path": "/dev/vda1", "size": 512 * 10**6, "fstype": "vfat", "label": "", "partlabel": ""},
                                         {"path": "/dev/vda2", "size": 40 * 10**9, "fstype": "ntfs", "label": "Windows", "partlabel": ""}]},
                              {"path": "/dev/vdb", "name": "vdb", "size": 128 * 10**9, "model": "QEMU HARDDISK", "kind": "SSD", "removable": False, "parts": []}]
            self.nics = [{"name": "eth0", "mac": "52:54:00:12:34:56", "wifi": False, "driver": "virtio_net", "model": "Virtio network"},
                         {"name": "eth1", "mac": "52:54:00:12:34:57", "wifi": False, "driver": "virtio_net", "model": "Virtio network"}]
            self.ctx.efi = True
            self.ctx.ram_mb = 8192
        else:
            self.nics = S.list_nics()
            S.bring_up(self.nics)
            self.ctx.disks = S.list_disks()
        self.state.nics = self.nics
        self.state.sync()

    # -- layout --------------------------------------------------------------------------
    def build_ui(self):
        p = self.p
        root = Gtk.Overlay()
        self.set_child(root)
        self.root = root
        main = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        root.set_child(main)
        # sidebar
        side = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        side.add_css_class("side")
        side.set_size_request(self.px(250), -1)
        mark = p.get("logo_mark", "")
        if mark and os.path.exists(mark):               # the official product mark (branding/), crop/scale only
            logo = Gtk.Image.new_from_file(mark)
            logo.set_pixel_size(self.px(56))
        else:
            logo = lbl(p["title"][:1].upper(), "logo", wrap=False, xalign=0.5)
        logo.set_halign(Gtk.Align.START)
        logo.set_margin_start(self.px(8))
        a11y(logo, p["title"] + " logo")
        side.append(logo)
        side.append(lbl(p["title"], "brand", wrap=False))
        side.append(lbl("Installer", "sub", wrap=False))
        self.step_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        side.append(self.step_box)
        filler = Gtk.Box()
        filler.set_vexpand(True)
        side.append(filler)
        main.append(side)
        # right
        right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        right.set_hexpand(True)
        main.append(right)
        top = Gtk.Box(spacing=self.px(10))
        top.add_css_class("top")
        top.append(lbl(self.state.p["title"] + " " + ("(UEFI)" if self.ctx.efi else "(BIOS)"), "hint", wrap=False))
        sp = Gtk.Box()
        sp.set_hexpand(True)
        top.append(sp)
        self.hc_btn = Gtk.ToggleButton(label="High contrast")
        self.hc_btn.set_active(self.hc)
        self.hc_btn.connect("toggled", lambda b: self.set_a11y(hc=b.get_active()))
        self.lg_btn = Gtk.ToggleButton(label="Larger text")
        self.lg_btn.set_active(self.large)
        self.lg_btn.connect("toggled", lambda b: self.set_a11y(large=b.get_active()))
        self.kb_btn = Gtk.ToggleButton(label="On-screen keyboard")
        self.kb_btn.connect("toggled", lambda b: self.osk_rev.set_reveal_child(b.get_active()))
        for b in (self.hc_btn, self.lg_btn, self.kb_btn):
            b.set_focus_on_click(False)
            top.append(b)
        right.append(top)
        self.title_l = lbl("", "title")
        self.intro_l = lbl("", "intro")
        head = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.px(4))
        head.set_margin_start(self.px(28))
        head.set_margin_end(self.px(28))
        head.set_margin_top(self.px(18))
        head.append(self.title_l)
        head.append(self.intro_l)
        right.append(head)
        self.stack = Gtk.Stack()
        self.stack.set_vexpand(True)
        self.stack.add_css_class("page")
        sc = Gtk.ScrolledWindow()
        sc.set_child(self.stack)
        sc.set_vexpand(True)
        sc.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        sc.set_margin_start(self.px(28))
        sc.set_margin_end(self.px(28))
        sc.set_margin_top(self.px(10))
        right.append(sc)
        self.err_l = lbl("", "err")
        self.err_l.set_margin_start(self.px(28))
        right.append(self.err_l)
        # on-screen keyboard
        self.osk_rev = Gtk.Revealer()
        self.osk_rev.set_child(self.make_osk())
        right.append(self.osk_rev)
        # bottom bar
        bar = Gtk.Box(spacing=self.px(10))
        bar.add_css_class("bar")
        self.abort_btn = Gtk.Button(label="Abort")
        self.abort_btn.connect("clicked", lambda b: self.ask_abort())
        bar.append(self.abort_btn)
        sp2 = Gtk.Box()
        sp2.set_hexpand(True)
        bar.append(sp2)
        self.back_btn = Gtk.Button(label="Back")
        self.back_btn.connect("clicked", lambda b: self.go_back())
        self.next_btn = Gtk.Button(label="Next")
        self.next_btn.add_css_class("suggested-action")
        self.next_btn.connect("clicked", lambda b: self.go_next())
        self.reboot_btn = Gtk.Button(label="Reboot")
        self.reboot_btn.add_css_class("suggested-action")
        self.reboot_btn.connect("clicked", lambda b: self.finish("reboot"))
        self.shell_btn = Gtk.Button(label="Shell")
        self.shell_btn.connect("clicked", lambda b: self.finish("shell"))
        for b in (self.back_btn, self.next_btn, self.shell_btn, self.reboot_btn):
            bar.append(b)
        right.append(bar)
        # pages
        self.pages = self.make_pages()
        for pg in self.pages:
            pg.widget = pg.build()
            self.stack.add_named(pg.widget, pg.id)
        for i, pg in enumerate(self.pages):
            l = lbl(f"{i + 1}  {pg.sidebar}", "step", wrap=False)
            pg.side = l
            self.step_box.append(l)

    def make_pages(self):
        pages = []
        for s in ST.stage_list(self.ctx, ui_only=True):
            if s.id == "keymap":
                pages.append(KeymapPage(self, s))
            elif s.id == "disk":
                pages.append(DiskPage(self, s))
            elif s.id == "netconfig":
                pages.append(NetPage(self, s))
            elif s.id == "use_flags":
                pages.append(UsePage(self, s))
            elif s.ui == "form":
                pages.append(FormPage(self, s))
            if s.id == "use_flags":
                pages += [ReviewPage(self), ProgressPage(self), DonePage(self)]
        return pages

    # -- OSK ------------------------------------------------------------------------------
    def make_osk(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.px(4))
        box.add_css_class("osk")
        box.set_margin_start(self.px(20))
        self.shift = False
        rows = ["1234567890-=", "qwertyuiop[]", "asdfghjkl;'", "zxcvbnm,./"]
        sh = {"1": "!", "2": "@", "3": "#", "4": "$", "5": "%", "6": "^", "7": "&", "8": "*", "9": "(", "0": ")", "-": "_", "=": "+",
              "[": "{", "]": "}", ";": ":", "'": '"', ",": "<", ".": ">", "/": "?"}
        self.keys = []
        for r in rows:
            hb = Gtk.Box(spacing=self.px(4))
            for ch in r:
                b = Gtk.Button(label=ch)
                b.set_focusable(False)
                b.set_focus_on_click(False)
                b.connect("clicked", lambda b, ch=ch: self.osk_type(sh.get(ch, ch.upper()) if self.shift else ch))
                self.keys.append((b, ch, sh.get(ch, ch.upper())))
                hb.append(b)
            box.append(hb)
        hb = Gtk.Box(spacing=self.px(4))
        for label, fn in (("Shift", self.osk_shift), ("Space", lambda: self.osk_type(" ")), ("\u232b", self.osk_back)):
            b = Gtk.Button(label=label)
            b.set_focusable(False)
            b.set_focus_on_click(False)
            b.connect("clicked", lambda b, fn=fn: fn())
            if label == "Space":
                b.set_hexpand(True)
            hb.append(b)
        box.append(hb)
        return box

    def osk_target(self):
        f = self.get_focus()
        while f is not None and not isinstance(f, Gtk.Editable):
            f = f.get_parent()
        return f

    def osk_type(self, ch):
        t = self.osk_target()
        if t is not None:
            pos = t.get_position()
            t.insert_text(ch, -1) if False else (t.insert_text(ch, pos) and t.set_position(pos + len(ch)))
        if self.shift:
            self.osk_shift()

    def osk_back(self):
        t = self.osk_target()
        if t is not None:
            pos = t.get_position()
            if pos > 0:
                t.delete_text(pos - 1, pos)

    def osk_shift(self):
        self.shift = not self.shift
        for b, ch, up in self.keys:
            b.set_label(up if self.shift else ch)

    # -- a11y --------------------------------------------------------------------------------
    def set_a11y(self, hc=None, large=None):
        if hc is not None:
            self.hc = hc
        if large is not None:
            self.large = large
        self.restyle()

    # -- navigation -----------------------------------------------------------------------------
    def show_page(self, i):
        self.idx = max(0, min(i, len(self.pages) - 1))
        pg = self.pages[self.idx]
        self.stack.set_visible_child_name(pg.id)
        self.title_l.set_text(pg.title)
        self.intro_l.set_text(pg.intro)
        for j, q in enumerate(self.pages):
            for k in ("current", "done"):
                q.side.remove_css_class(k)
            if j == self.idx:
                q.side.add_css_class("current")
            elif j < self.idx:
                q.side.add_css_class("done")
        self.err_l.set_text("")
        is_done = pg.id == "finish"
        self.back_btn.set_visible(pg.can_back and self.idx > 0)
        self.next_btn.set_visible(not is_done and pg.id != "install")
        self.reboot_btn.set_visible(is_done)
        self.shell_btn.set_visible(is_done)
        self.abort_btn.set_visible(pg.id not in ("install", "finish"))
        self.next_btn.set_label(pg.nav_next)
        self.next_btn.remove_css_class("destructive-action")
        self.next_btn.remove_css_class("suggested-action")
        self.next_btn.add_css_class("destructive-action" if pg.id == "review" else "suggested-action")
        pg.on_show()
        autoname(self)
        self.update_nav()
        self.write_status()

    def update_nav(self):
        pg = self.pages[self.idx]
        if pg.id in ("review", "use_flags", "disk"):
            self.next_btn.set_sensitive(pg.problem() is None)
            if pg.id == "use_flags" and pg.problem():
                self.err_l.set_text(pg.problem())
            elif pg.id == "use_flags":
                self.err_l.set_text("")
        else:
            self.next_btn.set_sensitive(True)

    def go_next(self):
        pg = self.pages[self.idx]
        if pg.id in ("finish",) or not self.next_btn.get_visible():
            return
        err = pg.problem()
        if err:
            self.err_l.set_text(err)
            self.write_status({"error": err})
            return
        try:
            pg.commit()
        except Exception as e:  # noqa
            import traceback
            traceback.print_exc()
            self.err_l.set_text(f"Internal error on this screen: {e}")
            return
        if pg.id == "keymap" and self.keymap_needs_restart():
            return
        self.advance()

    def go_back(self):
        pg = self.pages[self.idx]
        if pg.can_back and self.idx > 0:
            if pg.id == "netconfig":
                pg.stop_poll()
            self.show_page(self.idx - 1)

    def advance(self):
        self.show_page(self.idx + 1)

    def keymap_needs_restart(self):
        layout, variant = xkb_for(self.c["keymap"])
        if os.environ.get("XKB_DEFAULT_LAYOUT", "us") == layout and os.environ.get("XKB_DEFAULT_VARIANT", "") == variant:
            return False
        if self.dry or not os.environ.get("WAYLAND_DISPLAY"):
            return False
        try:
            os.makedirs(os.path.dirname(RESUME), exist_ok=True)
            with open(RESUME, "w") as f:
                json.dump({"cfg": self.c, "next": self.idx + 1, "xkb": [layout, variant]}, f)
        except OSError:
            return False
        self.result = "xkb"
        self.get_application().quit()
        return True

    def load_resume(self):
        try:
            with open(RESUME) as f:
                d = json.load(f)
            os.unlink(RESUME)
            self.c.update(d["cfg"])
            for pg in self.pages:
                pass
            return d["next"]
        except (OSError, ValueError, KeyError):
            return None

    # -- modal (in-window: cage shows a single toplevel) ------------------------------------------------
    def modal(self, title, text, buttons, kind="warn"):
        dim = Gtk.Box()
        dim.add_css_class("overlay-dim")
        dim.set_halign(Gtk.Align.FILL)
        dim.set_valign(Gtk.Align.FILL)
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.px(12))
        card.add_css_class("modal")
        card.set_halign(Gtk.Align.CENTER)
        card.set_valign(Gtk.Align.CENTER)
        card.set_size_request(self.px(520), -1)
        card.append(lbl(title, "title err" if kind == "err" else "title"))
        card.append(lbl(text, None))
        hb = Gtk.Box(spacing=self.px(10))
        hb.set_halign(Gtk.Align.END)
        group = Gtk.Overlay()
        group.set_child(dim)
        group.add_overlay(card)
        self.root.add_overlay(group)

        def close(fn):
            self.root.remove_overlay(group)
            if fn:
                fn()
        first = None
        for label, fn in buttons:
            b = Gtk.Button(label=label)
            b.connect("clicked", lambda b, fn=fn: close(fn))
            hb.append(b)
            first = first or b
        card.append(hb)
        autoname(card)
        if first:
            first.grab_focus()
        self.write_status({"modal": title})

    def picker(self, title, options, current, cb):
        """In-window searchable list: type to filter, Up/Down + Enter (or click/tap) to choose, Esc to cancel."""
        dim = Gtk.Box()
        dim.add_css_class("overlay-dim")
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=self.px(8))
        card.add_css_class("modal")
        card.set_halign(Gtk.Align.CENTER)
        card.set_valign(Gtk.Align.CENTER)
        card.set_size_request(self.px(560), self.px(520))
        card.append(lbl(title, "title"))
        search = Gtk.SearchEntry()
        search.set_placeholder_text("Type to filter")
        a11y(search, f"Filter {title}")
        card.append(search)
        lb = Gtk.ListBox()
        lb.set_selection_mode(Gtk.SelectionMode.SINGLE)
        lb.set_activate_on_single_click(True)
        rows = []
        for v, t in options:
            r = Gtk.ListBoxRow()
            r._v, r._t = v, t
            l = lbl(t, None, wrap=False)
            l.set_margin_top(self.px(6))
            l.set_margin_bottom(self.px(6))
            r.set_child(l)
            lb.append(r)
            rows.append(r)
        sc = Gtk.ScrolledWindow()
        sc.set_child(lb)
        sc.set_vexpand(True)
        card.append(sc)
        hb = Gtk.Box(spacing=self.px(10))
        hb.set_halign(Gtk.Align.END)
        cancel = Gtk.Button(label="Cancel")
        hb.append(cancel)
        card.append(hb)
        group = Gtk.Overlay()
        group.set_child(dim)
        group.add_overlay(card)
        self.root.add_overlay(group)
        prev_focus = self.get_focus()

        def close(v=None):
            self.root.remove_overlay(group)
            if prev_focus is not None:
                prev_focus.grab_focus()
            if v is not None:
                cb(v)

        def visible_rows():
            return [r for r in rows if r.get_child_visible() and r.get_mapped() or r.get_visible()]

        def flt(row):
            q = search.get_text().lower().strip()
            return not q or q in row._t.lower()
        lb.set_filter_func(flt)

        def first_match():
            for r in rows:
                if flt(r):
                    return r
            return None

        def on_changed(_e):
            lb.invalidate_filter()
            fm = first_match()
            if fm is not None:
                lb.select_row(fm)
        search.connect("search-changed", on_changed)
        search.connect("activate", lambda e: first_match() and close(lb.get_selected_row()._v if lb.get_selected_row() and flt(lb.get_selected_row()) else first_match()._v))
        lb.connect("row-activated", lambda l, r: close(r._v))
        cancel.connect("clicked", lambda b: close())
        kc = Gtk.EventControllerKey()

        def key(ctrl, keyval, keycode, state):
            if keyval == Gdk.KEY_Escape:
                close()
                return True
            if keyval in (Gdk.KEY_Down, Gdk.KEY_Up):
                vis = [r for r in rows if flt(r)]
                cur = lb.get_selected_row()
                i = vis.index(cur) if cur in vis else -1
                i = max(0, min(len(vis) - 1, i + (1 if keyval == Gdk.KEY_Down else -1)))
                if vis:
                    lb.select_row(vis[i])
                    GLib.idle_add(lambda: vis[i].grab_focus() if False else False)
                return True
            return False
        kc.connect("key-pressed", key)
        card.add_controller(kc)
        for r in rows:
            if r._v == current:
                lb.select_row(r)
                GLib.idle_add(lambda r=r: (r.grab_focus(), search.grab_focus(), False)[2])
        autoname(card)
        search.grab_focus()
        self.write_status({"modal": "picker " + title})

    def ask_abort(self):
        pg = self.pages[self.idx]
        if pg.id in ("install", "finish"):
            return
        self.modal("Abort installation?", "Leave the installer? Nothing has been changed on disk yet. You will get a live shell.",
                   [("Continue", None), ("Abort", lambda: self.finish("abort"))])

    def finish(self, how):
        self.result = how
        self.get_application().quit()

    def write_status(self, extra=None):
        try:
            pg = self.pages[self.idx]
            d = {"page": pg.id, "title": pg.title, "index": self.idx, "pages": [q.id for q in self.pages], "t": time.time(),
                 "error": self.err_l.get_text(), "next_enabled": self.next_btn.get_sensitive()}
            d.update(pg.status())
            d.update(extra or {})
            os.makedirs(os.path.dirname(STATUS), exist_ok=True)
            with open(STATUS, "w") as f:
                json.dump(d, f)
        except Exception:  # noqa
            pass


class App(Gtk.Application):
    def __init__(self, profile, dry):
        super().__init__(application_id="org.errant.LiveInstaller")
        self.profile, self.dry = profile, dry
        self.result = "fallback"
        self.win = None

    def do_activate(self):
        try:
            self.win = Win(self, self.profile, self.dry)
        except Exception:  # noqa
            import traceback
            traceback.print_exc()
            self.result = "fallback"
            self.quit()
            return
        self.win.present()


def run(profile, dry=False):
    if not Gtk.init_check():
        return "fallback"
    if Gdk.Display.get_default() is None:
        return "fallback"
    app = App(profile, dry)
    app.run([])
    if app.win is None:
        return "fallback"
    return app.win.result
