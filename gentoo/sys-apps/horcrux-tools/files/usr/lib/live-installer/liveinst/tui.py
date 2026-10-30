"""Tiny curses toolkit for the guided installer.

Works on a VGA/framebuffer console and on a dumb serial line (80x24, vt100):
keys are read raw (no terminfo keypad translation) and escape sequences are
parsed here, so arrow keys behave the same for TERM=linux, vt100 and xterm.

Keys: Up/Down/Tab/Shift-Tab move, Enter = next field / press button,
Left/Right = cycle a choice or pick a button, Space = toggle a check box,
Esc = abort (asks first).  Printable characters edit the focused text field.
"""
import curses
import os
import time

os.environ.setdefault("ESCDELAY", "25")

UP, DOWN, LEFT, RIGHT, TAB, BTAB, ENTER, ESC, BACKSPACE, DELETE, HOME, END, PGUP, PGDN = (
    "UP", "DOWN", "LEFT", "RIGHT", "TAB", "BTAB", "ENTER", "ESC", "BS", "DEL", "HOME", "END", "PGUP", "PGDN")


class Abort(Exception):
    pass


class Term:
    def __init__(self, scr, title="", subtitle=""):
        self.scr = scr
        self.title = title
        self.subtitle = subtitle
        try:
            curses.curs_set(1)
        except curses.error:
            pass
        scr.idlok(False)
        scr.scrollok(False)
        scr.keypad(False)          # raw bytes: we parse escape sequences ourselves
        scr.timeout(-1)
        self.colors = False
        try:
            if curses.has_colors():
                curses.start_color()
                try:
                    curses.use_default_colors()
                    bg = -1
                except curses.error:
                    bg = curses.COLOR_BLACK
                curses.init_pair(1, curses.COLOR_CYAN, bg)      # title
                curses.init_pair(2, curses.COLOR_RED, bg)       # error
                curses.init_pair(3, curses.COLOR_GREEN, bg)     # ok
                curses.init_pair(4, curses.COLOR_YELLOW, bg)    # warning
                self.colors = True
        except curses.error:
            pass

    # ---- attributes -------------------------------------------------
    def a(self, kind):
        if kind == "title":
            return curses.A_BOLD | (curses.color_pair(1) if self.colors else 0)
        if kind == "err":
            return curses.A_BOLD | (curses.color_pair(2) if self.colors else 0)
        if kind == "ok":
            return curses.A_BOLD | (curses.color_pair(3) if self.colors else 0)
        if kind == "warn":
            return curses.A_BOLD | (curses.color_pair(4) if self.colors else 0)
        if kind == "sel":
            return curses.A_REVERSE
        if kind == "bold":
            return curses.A_BOLD
        return curses.A_NORMAL

    def size(self):
        h, w = self.scr.getmaxyx()
        return max(h, 10), max(w, 40)

    def put(self, y, x, text, attr=0):
        h, w = self.scr.getmaxyx()
        if y < 0 or y >= h or x >= w:
            return
        try:
            self.scr.addstr(y, x, text[: max(0, w - x - (1 if y == h - 1 else 0))], attr)
        except curses.error:
            pass

    # ---- input --------------------------------------------------------
    def key(self):
        """Return a key name (constants above) or a printable character."""
        while True:
            try:
                ch = self.scr.get_wch()
            except KeyboardInterrupt:
                raise Abort()
            except curses.error:
                continue
            if ch == curses.KEY_RESIZE:
                return "RESIZE"
            if ch == "\x0c":                  # Ctrl-L: repaint (serial lines can garble)
                self.scr.clearok(True)
                return "RESIZE"
            if isinstance(ch, int):
                mapped = {curses.KEY_UP: UP, curses.KEY_DOWN: DOWN, curses.KEY_LEFT: LEFT, curses.KEY_RIGHT: RIGHT,
                          curses.KEY_BACKSPACE: BACKSPACE, curses.KEY_DC: DELETE, curses.KEY_HOME: HOME,
                          curses.KEY_END: END, curses.KEY_PPAGE: PGUP, curses.KEY_NPAGE: PGDN,
                          curses.KEY_BTAB: BTAB, curses.KEY_ENTER: ENTER}.get(ch)
                if mapped:
                    return mapped
                continue
            if ch == "\x1b":
                return self._escape()
            if ch in ("\r", "\n"):
                return ENTER
            if ch == "\t":
                return TAB
            if ch in ("\x7f", "\x08"):
                return BACKSPACE
            if ch == "\x03":
                raise Abort()
            if ch == "\x15":
                return "CLEAR"
            if ch >= " ":
                return ch

    def _escape(self):
        self.scr.timeout(60)
        try:
            try:
                c1 = self.scr.get_wch()
            except curses.error:
                return ESC
            if c1 not in ("[", "O"):
                return ESC
            seq = ""
            for _ in range(6):
                try:
                    c = self.scr.get_wch()
                except curses.error:
                    break
                seq += c if isinstance(c, str) else ""
                if c1 == "O" or (seq and (seq[-1].isalpha() or seq[-1] == "~")):
                    break
            table = {"A": UP, "B": DOWN, "C": RIGHT, "D": LEFT, "H": HOME, "F": END, "Z": BTAB,
                     "1~": HOME, "7~": HOME, "4~": END, "8~": END, "3~": DELETE, "5~": PGUP, "6~": PGDN}
            return table.get(seq, "UNKNOWN")
        finally:
            self.scr.timeout(-1)

    # ---- frames ---------------------------------------------------------
    def frame(self, step, heading, full=True):
        # full=True repaints everything (robust on any terminal, ~1.5 KB); progress frames use diffs
        if full:
            self.scr.clear()
        else:
            self.scr.erase()
        h, w = self.size()
        self.put(0, 1, self.title, self.a("title"))
        if self.subtitle:
            self.put(0, max(2 + len(self.title), w - len(self.subtitle) - 2), self.subtitle)
        self.put(1, 0, "=" * (w - 1))
        self.put(2, 1, (f"[{step}] " if step else "") + heading, self.a("bold"))
        return h, w

    def footer(self, text, attr=0):
        h, w = self.size()
        self.put(h - 1, 0, " " * (w - 1))
        self.put(h - 1, 1, text, attr)

    def wrap(self, text, width):
        out = []
        for para in text.split("\n"):
            if not para:
                out.append("")
                continue
            line = ""
            for word in para.split(" "):
                if len(line) + len(word) + 1 > width and line:
                    out.append(line)
                    line = word
                else:
                    line = (line + " " + word) if line else word
            out.append(line)
        return out


class Field:
    """kind: text | password | choice | list | check | label | info | multi"""

    def __init__(self, key, label, kind="text", value="", options=None, validate=None, hint="", width=38,
                 visible=None, optional=False):
        self.key, self.label, self.kind = key, label, kind
        self.value = value
        self.options = options or []      # choice/list: [(value, text)]
        self.validate = validate
        self.hint = hint
        self.width = width
        self.visible = visible            # callable(values)->bool
        self.optional = optional

    def text_of(self):
        if self.kind == "password":
            return "*" * len(self.value)
        if self.kind == "check":
            return "[x]" if self.value else "[ ]"
        if self.kind in ("choice", "list"):
            for v, t in self.options:
                if v == self.value:
                    return t
            return str(self.value)
        if self.kind == "multi":
            return ", ".join(t for v, t in self.options if v in self.value) or "(none)"
        return str(self.value)


def popup_list(term, title, options, current=None, filterable=True):
    """Pick one of [(value, text)]; type to filter. Returns value or None."""
    flt = ""
    sel = 0
    for i, (v, _t) in enumerate(options):
        if v == current:
            sel = i
    top = 0
    while True:
        shown = [(v, t) for v, t in options if flt.lower() in t.lower()] if flt else list(options)
        sel = min(sel, max(0, len(shown) - 1))
        h, w = term.size()
        rows = min(len(shown), h - 8)
        bw = min(w - 4, max([len(t) for _v, t in shown] + [len(title) + 4, 30]) + 6)
        bh = rows + 4
        y0, x0 = max(1, (h - bh) // 2), max(1, (w - bw) // 2)
        if sel < top:
            top = sel
        if sel >= top + rows:
            top = sel - rows + 1
        for r in range(bh):
            term.put(y0 + r, x0, " " * bw, curses.A_REVERSE if r == 0 else 0)
        term.put(y0, x0 + 1, f" {title} ", curses.A_REVERSE | curses.A_BOLD)
        term.put(y0 + 1, x0 + 1, ("Filter: " + flt + "_") if filterable else "Up/Down, Enter = select, Esc = cancel")
        for r in range(rows):
            i = top + r
            if i < len(shown):
                text = shown[i][1][: bw - 4]
                term.put(y0 + 2 + r, x0 + 2, text.ljust(bw - 4), term.a("sel") if i == sel else 0)
        term.put(y0 + bh - 1, x0 + 1, "Enter=select Esc=cancel" + ("  (type to filter)" if filterable else ""))
        term.scr.refresh()
        k = term.key()
        if k == UP:
            sel = max(0, sel - 1)
        elif k == DOWN:
            sel = min(len(shown) - 1, sel + 1)
        elif k == PGUP:
            sel = max(0, sel - rows)
        elif k == PGDN:
            sel = min(len(shown) - 1, sel + rows)
        elif k == ENTER:
            return shown[sel][0] if shown else None
        elif k == ESC:
            return None
        elif k == BACKSPACE:
            flt = flt[:-1]
        elif k == "CLEAR":
            flt = ""
        elif filterable and len(k) == 1:
            flt += k
            sel = 0
            top = 0


def message(term, title, text, kind="warn", buttons=("OK",)):
    """Modal message; returns the chosen button text."""
    idx = 0
    while True:
        h, w = term.size()
        lines = term.wrap(text, min(w - 10, 70))
        bh = len(lines) + 5
        bw = min(w - 2, max(max(len(l) for l in lines), len(title), 24) + 6)
        y0, x0 = max(1, (h - bh) // 2), max(0, (w - bw) // 2)
        for r in range(bh):
            term.put(y0 + r, x0, " " * bw, curses.A_REVERSE)
        term.put(y0, x0 + 2, f" {title} ", term.a(kind) | curses.A_REVERSE)
        for i, l in enumerate(lines):
            term.put(y0 + 2 + i, x0 + 2, l, curses.A_REVERSE)
        bx = x0 + 2
        for i, b in enumerate(buttons):
            label = f"< {b} >"
            term.put(y0 + bh - 2, bx, label, curses.A_NORMAL if i == idx else curses.A_REVERSE)
            bx += len(label) + 2
        term.scr.move(y0 + bh - 2, x0 + 2)
        term.scr.refresh()
        k = term.key()
        if k in (LEFT, UP, BTAB):
            idx = (idx - 1) % len(buttons)
        elif k in (RIGHT, DOWN, TAB):
            idx = (idx + 1) % len(buttons)
        elif k == ENTER:
            return buttons[idx]
        elif k == ESC and len(buttons) > 1:
            return buttons[-1]


def run_form(term, step, heading, intro, fields, buttons=("Back", "Next"), validate_all=None, status=""):
    """Run a form. Returns (button_text, {key: value}).  Validation runs on 'Next'."""
    pos = 0
    err = ""
    bsel = len(buttons) - 1
    def vis():
        vals = {f.key: f.value for f in fields}
        return [f for f in fields if f.visible is None or f.visible(vals)]
    while True:
        vf = vis()
        focusable = [f for f in vf if f.kind not in ("label", "info")]
        total = len(focusable) + 1               # + the buttons row
        pos = min(pos, total - 1)
        h, w = term.frame(step, heading)
        y = 4
        for l in term.wrap(intro, w - 4):
            term.put(y, 2, l)
            y += 1
        y += 1
        lw = max([len(f.label) for f in vf if f.kind not in ("label",)] + [8]) + 2
        cur = None
        for f in vf:
            if f.kind == "label":
                for l in term.wrap(f.label, w - 6):
                    term.put(y, 2, l, term.a("warn") if f.hint == "warn" else 0)
                    y += 1
                continue
            if f.kind == "info":
                val = f.value({x.key: x.value for x in fields}) if callable(f.value) else f.value
                term.put(y, 2, f.label.ljust(lw) + str(val)[: w - lw - 4])
                y += 1
                continue
            focused = pos < len(focusable) and focusable[pos] is f
            term.put(y, 2, f.label.ljust(lw))
            txt = f.text_of()
            if f.kind in ("choice", "list"):
                disp = f"< {txt} >"
            else:
                disp = txt
            width = min(f.width, w - lw - 6)
            if f.kind in ("text", "password"):
                shown = disp[-width:]
                term.put(y, 2 + lw, "[" + shown.ljust(width) + "]", term.a("sel") if focused else 0)
                if focused:
                    cur = (y, 2 + lw + 1 + min(len(shown), width))
            else:
                term.put(y, 2 + lw, disp[: w - lw - 4], term.a("sel") if focused else 0)
                if focused:
                    cur = (y, 2 + lw)
            y += 1
            if focused and f.hint and f.hint != "warn":
                term.put(h - 3, 2, f.hint[: w - 4])
        # buttons
        by = min(y + 1, h - 4)
        bx = 2
        for i, b in enumerate(buttons):
            lab = f"< {b} >"
            on = pos == total - 1 and i == bsel
            term.put(by, bx, lab, term.a("sel") if on else 0)
            if on:
                cur = (by, bx + 2)
            bx += len(lab) + 2
        if err:
            term.footer(err, term.a("err"))
        else:
            term.footer(status or "Tab/Enter: next field   Space/Left/Right: change   Enter on < Next >: continue   Esc: abort")
        if cur:
            try:
                term.scr.move(*cur)
            except curses.error:
                pass
        term.scr.refresh()
        k = term.key()
        err = ""
        if k == "RESIZE":
            continue
        if k == ESC:
            if message(term, "Abort installation?", "Leave the installer? Nothing has been changed on disk yet.",
                       "warn", ("Continue", "Abort")) == "Abort":
                raise Abort()
            continue
        on_buttons = pos == total - 1
        f = None if on_buttons else focusable[pos]
        if k in (DOWN, TAB):
            pos = (pos + 1) % total
        elif k in (UP, BTAB):
            pos = (pos - 1) % total
        elif on_buttons and k in (LEFT, RIGHT):
            bsel = (bsel + (1 if k == RIGHT else -1)) % len(buttons)
        elif on_buttons and k == ENTER:
            b = buttons[bsel]
            if b == buttons[-1] and b in ("Next", "Install", "Reboot", "Continue", "Erase and install"):
                bad = None
                for ff in vis():
                    if ff.kind in ("label", "info"):
                        continue
                    if ff.validate and not (ff.optional and not ff.value):
                        e = ff.validate(ff.value, {x.key: x.value for x in vis()})
                        if e:
                            bad = (ff, e)
                            break
                if not bad and validate_all:
                    e = validate_all({x.key: x.value for x in vis()})
                    if e:
                        bad = (None, e)
                if bad:
                    err = bad[1]
                    if bad[0] is not None:
                        pos = focusable.index(bad[0])
                    continue
            return b, {x.key: x.value for x in fields}
        elif f is not None and f.kind in ("text", "password"):
            if k == ENTER:
                pos += 1
            elif k == BACKSPACE:
                f.value = f.value[:-1]
            elif k == "CLEAR":
                f.value = ""
            elif len(k) == 1:
                f.value += k
        elif f is not None and f.kind in ("choice", "list"):
            vals = [v for v, _t in f.options]
            i = vals.index(f.value) if f.value in vals else 0
            if k == LEFT and vals:
                f.value = vals[(i - 1) % len(vals)]
            elif k in (RIGHT, " ") and vals:
                f.value = vals[(i + 1) % len(vals)]
            elif k == ENTER:
                if f.kind == "list" or len(vals) > 6:
                    r = popup_list(term, f.label.strip(), f.options, f.value)
                    if r is not None:
                        f.value = r
                pos += 1
        elif f is not None and f.kind == "check":
            if k in (" ", LEFT, RIGHT):
                f.value = not f.value
            elif k == ENTER:
                pos += 1
        elif f is not None and f.kind == "multi":
            # Space toggles the option under the 'cursor' (cycled with Left/Right)
            ids = [v for v, _t in f.options]
            if k in (LEFT, RIGHT) and ids:
                f._cur = (getattr(f, "_cur", 0) + (1 if k == RIGHT else -1)) % len(ids)
            elif k == " " and ids:
                v = ids[getattr(f, "_cur", 0)]
                f.value = [x for x in f.value if x != v] if v in f.value else f.value + [v]
            elif k == ENTER:
                pos += 1


def run_disk_picker(term, step, heading, intro, disks, selected, multi, fmt_size):
    """Disk list with a detail pane. Returns (button, [paths])."""
    cur = 0
    sel = list(selected)
    err = ""
    on_buttons = False
    bsel = 1
    buttons = ("Back", "Next")
    while True:
        h, w = term.frame(step, heading)
        y = 4
        for l in term.wrap(intro, w - 4):
            term.put(y, 2, l)
            y += 1
        y += 1
        term.put(y, 2, "    DEVICE        SIZE      TYPE   MODEL", term.a("bold"))
        y += 1
        max_rows = max(3, min(len(disks), h - y - 12))
        top = max(0, cur - max_rows + 1)
        for i in range(top, min(len(disks), top + max_rows)):
            d = disks[i]
            mark = "[x]" if d["path"] in sel else "[ ]"
            line = f"{mark} {d['path']:<12} {fmt_size(d['size']):>8}  {d['kind']:<6} {d['model']}"
            term.put(y, 2, line[: w - 4], term.a("sel") if (i == cur and not on_buttons) else 0)
            y += 1
        y += 1
        d = disks[cur] if disks else None
        if d:
            term.put(y, 2, f"Existing content of {d['path']} (will be DESTROYED if selected):", term.a("bold"))
            y += 1
            room = max(1, h - y - 4)
            if not d["parts"]:
                term.put(y, 4, "(no partitions - looks empty)")
            for p in d["parts"][:room]:
                desc = f"{p['path']:<14} {fmt_size(p['size']):>8}  {p['fstype'] or '-':<8} {p['label'] or p['partlabel'] or ''}"
                term.put(y, 4, desc[: w - 6])
                y += 1
        by = h - 3
        bx = 2
        for i, b in enumerate(buttons):
            lab = f"< {b} >"
            term.put(by, bx, lab, term.a("sel") if (on_buttons and i == bsel) else 0)
            bx += len(lab) + 2
        if err:
            term.footer(err, term.a("err"))
        else:
            term.footer(("Space: select/unselect" if multi else "Space: select") + "   Up/Down: move   Tab: buttons   Esc: abort")
        term.scr.refresh()
        k = term.key()
        err = ""
        if k == ESC:
            if message(term, "Abort installation?", "Leave the installer? Nothing has been changed on disk yet.",
                       "warn", ("Continue", "Abort")) == "Abort":
                raise Abort()
        elif k == TAB or (k == DOWN and not on_buttons and cur == len(disks) - 1):
            on_buttons = not on_buttons
        elif k == BTAB or (k == UP and on_buttons):
            on_buttons = not on_buttons
        elif on_buttons and k in (LEFT, RIGHT):
            bsel = 1 - bsel
        elif on_buttons and k == ENTER:
            if buttons[bsel] == "Back":
                return "Back", sel
            if not sel:
                err = "Select a disk first (Space)."
                on_buttons = False
                continue
            return "Next", sel
        elif not on_buttons and k == UP:
            cur = max(0, cur - 1)
        elif not on_buttons and k == DOWN:
            cur = min(len(disks) - 1, cur + 1)
        elif not on_buttons and k in (" ", ENTER):
            p = disks[cur]["path"]
            if multi:
                sel = [x for x in sel if x != p] if p in sel else sel + [p]
            else:
                sel = [p]
            if k == ENTER and not multi:
                on_buttons = True
                bsel = 1


def progress_screen(term, step, heading, pct, lines, note=""):
    h, w = term.frame(step, heading, full=False)
    barw = max(10, w - 14)
    filled = int(barw * pct / 100)
    term.put(4, 2, "[" + "#" * filled + "." * (barw - filled) + f"] {pct:3d}%")
    term.put(5, 2, note[: w - 4], term.a("bold"))
    avail = h - 9
    for i, l in enumerate(lines[-avail:]):
        term.put(7 + i, 2, l[: w - 4])
    term.footer("Installation in progress - do not power off. Full log: /var/log/live-installer.log")
    term.scr.refresh()
