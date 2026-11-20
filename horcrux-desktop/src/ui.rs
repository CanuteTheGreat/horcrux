//! The connection window: node URL, user and password, then the node's
//! VMs that the Horcrux Console can show, a profile, and Open. Drawn into
//! the same software surface as the console picture (an 8x8 bitmap font,
//! scaled), so it needs nothing beyond the window itself.
//!
//! Everything here is plain state and pixels; `main.rs` feeds it key and
//! mouse events and acts on the returned [`Action`].

/// Profiles offered for a console (empty: the VM's own).
pub const PROFILES: [(&str, &str); 4] = [
    ("", "VM default"),
    ("desktop", "Desktop"),
    ("gaming", "Gaming"),
    ("console", "Console"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct VmEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Node,
    User,
    Password,
    Connect,
    Trust,
    List,
    Profile,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Tab,
    BackTab,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Escape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Connect,
    Trust,
    Open,
    Quit,
}

#[derive(Debug, Default)]
pub struct Form {
    pub node: String,
    pub user: String,
    pub password: String,
    pub focus: Option<Field>,
    pub vms: Vec<VmEntry>,
    /// VMs on the node the console cannot show (stopped, other hypervisor,
    /// console off), counted for the hint line.
    pub hidden: usize,
    pub selected: usize,
    pub profile: usize,
    pub status: String,
    /// A second, quieter line under the status (e.g. a fingerprint).
    pub detail: String,
    pub error: bool,
    pub busy: bool,
    pub logged_in: bool,
    /// Fingerprint of an untrusted node certificate the user may trust.
    pub untrusted: Option<String>,
    /// VM to select (and open) after login (from a link).
    pub want_vm: Option<String>,
}

const FIELD_MAX: usize = 256;

impl Form {
    pub fn new(node: &str, user: &str, profile: &str) -> Form {
        Form {
            node: node.into(),
            user: user.into(),
            profile: PROFILES.iter().position(|p| p.0 == profile).unwrap_or(0),
            focus: Some(if node.is_empty() {
                Field::Node
            } else if user.is_empty() {
                Field::User
            } else {
                Field::Password
            }),
            ..Default::default()
        }
    }

    pub fn profile_name(&self) -> &'static str {
        PROFILES[self.profile].0
    }

    pub fn selected_vm(&self) -> Option<&VmEntry> {
        self.vms.get(self.selected)
    }

    /// Fields in Tab order for the current state.
    fn order(&self) -> Vec<Field> {
        let mut v = vec![Field::Node, Field::User, Field::Password, Field::Connect];
        if self.untrusted.is_some() {
            v.push(Field::Trust);
        }
        if self.logged_in && !self.vms.is_empty() {
            v.extend([Field::List, Field::Profile, Field::Open]);
        }
        v
    }

    pub fn set_vms(&mut self, vms: Vec<VmEntry>, hidden: usize) {
        self.vms = vms;
        self.hidden = hidden;
        self.logged_in = true;
        self.selected = self
            .want_vm
            .as_ref()
            .and_then(|w| self.vms.iter().position(|v| &v.id == w))
            .unwrap_or(0);
        self.focus = Some(if self.vms.is_empty() {
            Field::Connect
        } else {
            Field::List
        });
    }

    /// The node, user or password changed: the VM list belongs to the old
    /// login.
    fn edited(&mut self) {
        if self.logged_in {
            self.logged_in = false;
            self.vms.clear();
        }
        self.untrusted = None;
    }

    pub fn key(&mut self, k: Key) -> Action {
        if k == Key::Escape {
            return Action::Quit;
        }
        if self.busy {
            return Action::None;
        }
        let order = self.order();
        let focus = self.focus.unwrap_or(Field::Node);
        let pos = order.iter().position(|f| *f == focus).unwrap_or(0);
        match k {
            Key::Tab => self.focus = Some(order[(pos + 1) % order.len()]),
            Key::BackTab => self.focus = Some(order[(pos + order.len() - 1) % order.len()]),
            Key::Enter => {
                return match focus {
                    Field::Node | Field::User | Field::Password | Field::Connect => Action::Connect,
                    Field::Trust => Action::Trust,
                    Field::List | Field::Profile | Field::Open => Action::Open,
                }
            }
            Key::Up if focus == Field::List => self.selected = self.selected.saturating_sub(1),
            Key::Down if focus == Field::List => {
                self.selected = (self.selected + 1).min(self.vms.len().saturating_sub(1))
            }
            Key::Left if focus == Field::Profile => {
                self.profile = (self.profile + PROFILES.len() - 1) % PROFILES.len()
            }
            Key::Right if focus == Field::Profile => {
                self.profile = (self.profile + 1) % PROFILES.len()
            }
            Key::Char(c) if !c.is_control() => {
                let t = match focus {
                    Field::Node => &mut self.node,
                    Field::User => &mut self.user,
                    Field::Password => &mut self.password,
                    _ => return Action::None,
                };
                if t.chars().count() < FIELD_MAX {
                    t.push(c);
                }
                self.edited();
            }
            Key::Backspace => {
                let t = match focus {
                    Field::Node => &mut self.node,
                    Field::User => &mut self.user,
                    Field::Password => &mut self.password,
                    _ => return Action::None,
                };
                t.pop();
                self.edited();
            }
            _ => {}
        }
        Action::None
    }

    pub fn click(&mut self, x: i32, y: i32, l: &Layout) -> Action {
        if self.busy {
            return Action::None;
        }
        let Some((f, row)) = l.hit(x, y) else {
            return Action::None;
        };
        if !self.order().contains(&f) {
            return Action::None;
        }
        self.focus = Some(f);
        match f {
            Field::Connect => Action::Connect,
            Field::Trust => Action::Trust,
            Field::Open => Action::Open,
            Field::Profile => {
                self.profile = (self.profile + 1) % PROFILES.len();
                Action::None
            }
            Field::List => {
                if let Some(r) = row {
                    let i = l.list_first + r;
                    if i < self.vms.len() {
                        let again = self.selected == i;
                        self.selected = i;
                        // a second click on the selected VM opens it
                        if again {
                            return Action::Open;
                        }
                    }
                }
                Action::None
            }
            _ => Action::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// Where everything is for a window size and text scale.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub scale: i32,
    pub fields: Vec<(Field, Rect)>,
    pub list_rows: usize,
    pub list_first: usize,
    pub row_h: i32,
    pub labels: Vec<(String, i32, i32)>,
    pub status_y: i32,
    /// status and detail text, wrapped to the window width
    pub status_lines: Vec<String>,
    pub detail_lines: Vec<String>,
    pub hint_y: i32,
}

impl Layout {
    pub fn new(f: &Form, w: i32, h: i32, scale: i32) -> Layout {
        let s = scale.max(1);
        let ch = 8 * s; // character cell
        let m = 2 * ch;
        let fw = (w - 2 * m).clamp(10 * ch, 80 * ch);
        let fh = ch + ch / 2 + 4;
        let mut l = Layout {
            scale: s,
            row_h: ch + ch / 2,
            ..Default::default()
        };
        let mut y = m + 2 * ch; // below the title
        for (label, field) in [
            ("Node URL", Field::Node),
            ("User", Field::User),
            ("Password", Field::Password),
        ] {
            l.labels.push((label.into(), m, y));
            y += ch + ch / 2;
            l.fields.push((
                field,
                Rect {
                    x: m,
                    y,
                    w: fw,
                    h: fh,
                },
            ));
            y += fh + ch;
        }
        let bw = 14 * ch;
        l.fields.push((
            Field::Connect,
            Rect {
                x: m,
                y,
                w: bw,
                h: fh,
            },
        ));
        if f.untrusted.is_some() {
            l.fields.push((
                Field::Trust,
                Rect {
                    x: m + bw + ch,
                    y,
                    w: 22 * ch,
                    h: fh,
                },
            ));
        }
        y += fh + ch;
        l.status_y = y;
        let cols = ((w - 2 * m) / ch).max(10) as usize;
        l.status_lines = wrap(&f.status, cols, 3);
        l.detail_lines = wrap(&f.detail, cols, 2);
        let lines = (l.status_lines.len() + l.detail_lines.len()).max(2) as i32;
        y += lines * (ch + ch / 2) + ch / 2;
        if f.logged_in && !f.vms.is_empty() {
            l.labels.push(("Virtual machines".into(), m, y));
            y += ch + ch / 2;
            let room = (h - y - 3 * fh - 4 * ch).max(l.row_h);
            l.list_rows = ((room / l.row_h) as usize).clamp(1, f.vms.len().max(1));
            l.list_first = f.selected.saturating_sub(l.list_rows - 1);
            let lh = l.list_rows as i32 * l.row_h + 4;
            l.fields.push((
                Field::List,
                Rect {
                    x: m,
                    y,
                    w: fw,
                    h: lh,
                },
            ));
            y += lh + ch / 2;
            l.hint_y = y;
            y += ch + ch / 2;
            l.labels.push(("Profile".into(), m, y + (fh - ch) / 2));
            l.fields.push((
                Field::Profile,
                Rect {
                    x: m + 9 * ch,
                    y,
                    w: 16 * ch,
                    h: fh,
                },
            ));
            y += fh + ch;
            l.fields.push((
                Field::Open,
                Rect {
                    x: m,
                    y,
                    w: bw,
                    h: fh,
                },
            ));
        } else {
            l.hint_y = y;
        }
        l
    }

    /// The field under a point, and the list row when it is the list.
    pub fn hit(&self, x: i32, y: i32) -> Option<(Field, Option<usize>)> {
        let (f, r) = self.fields.iter().find(|(_, r)| r.contains(x, y))?;
        let row = (*f == Field::List).then(|| ((y - r.y - 2).max(0) / self.row_h) as usize);
        Some((*f, row))
    }

    #[cfg(test)]
    pub fn rect(&self, f: Field) -> Option<Rect> {
        self.fields.iter().find(|(g, _)| *g == f).map(|(_, r)| *r)
    }
}

pub const BG: u32 = 0x1e1e2e;
const FIELD: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;
const DIM: u32 = 0x9399b2;
const ACCENT: u32 = 0x89b4fa;
const ERROR: u32 = 0xf38ba8;
const OK: u32 = 0xa6e3a1;
const DARK: u32 = 0x11111b;

/// A software framebuffer (0RGB pixels).
pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
}

impl Canvas<'_> {
    pub fn fill(&mut self, r: Rect, c: u32) {
        let x0 = r.x.clamp(0, self.w);
        let x1 = (r.x + r.w).clamp(0, self.w);
        if x1 <= x0 {
            return;
        }
        for y in r.y.clamp(0, self.h)..(r.y + r.h).clamp(0, self.h) {
            let row = (y * self.w) as usize;
            self.px[row + x0 as usize..row + x1 as usize].fill(c);
        }
    }

    fn frame(&mut self, r: Rect, c: u32, t: i32) {
        self.fill(Rect { h: t, ..r }, c);
        self.fill(
            Rect {
                y: r.y + r.h - t,
                h: t,
                ..r
            },
            c,
        );
        self.fill(Rect { w: t, ..r }, c);
        self.fill(
            Rect {
                x: r.x + r.w - t,
                w: t,
                ..r
            },
            c,
        );
    }

    /// Text in the 8x8 font at `scale`; clipped to `max_w` pixels.
    pub fn text(&mut self, x: i32, y: i32, s: &str, c: u32, scale: i32, max_w: i32) {
        let mut cx = x;
        for ch in s.chars() {
            if cx + 8 * scale > x + max_w {
                break;
            }
            let g = font8x8::legacy::BASIC_LEGACY
                .get(ch as usize)
                .copied()
                .unwrap_or(font8x8::legacy::BASIC_LEGACY['?' as usize]);
            for (gy, bits) in g.iter().enumerate() {
                for gx in 0..8 {
                    if bits & (1 << gx) != 0 {
                        self.fill(
                            Rect {
                                x: cx + gx * scale,
                                y: y + gy as i32 * scale,
                                w: scale,
                                h: scale,
                            },
                            c,
                        );
                    }
                }
            }
            cx += 8 * scale;
        }
    }
}

/// Word-wrap `s` to `cols` characters, at most `max` lines (the last one
/// ends in "..." when text was cut). Long words (fingerprints) are split.
fn wrap(s: &str, cols: usize, max: usize) -> Vec<String> {
    let cols = cols.max(4);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = cur.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= cols {
                if gap == 1 {
                    cur.push(' ');
                }
                cur.extend(word.iter());
                break;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut cur));
                continue;
            }
            // a word longer than a line
            let rest = word.split_off(cols);
            lines.push(word.iter().collect());
            word = rest;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max {
        lines.truncate(max);
        let last = &mut lines[max - 1];
        let keep = last.chars().count().min(cols - 3);
        *last = last.chars().take(keep).collect::<String>() + "...";
    }
    lines
}

/// Keep the end of a long value visible (the caret is there).
fn tail(s: &str, chars: usize) -> String {
    let n = s.chars().count();
    if n <= chars {
        s.to_string()
    } else {
        s.chars().skip(n - chars).collect()
    }
}

pub fn draw(f: &Form, l: &Layout, c: &mut Canvas) {
    let s = l.scale;
    let ch = 8 * s;
    let m = 2 * ch;
    c.fill(
        Rect {
            x: 0,
            y: 0,
            w: c.w,
            h: c.h,
        },
        BG,
    );
    c.text(m, m, "Horcrux Console", ACCENT, s, c.w - 2 * m);
    for (t, x, y) in &l.labels {
        c.text(*x, *y, t, DIM, s, c.w);
    }
    for (field, r) in &l.fields {
        let focused = f.focus == Some(*field);
        let pad = (r.h - ch) / 2;
        let inner = r.w - 2 * pad;
        match field {
            Field::Node | Field::User | Field::Password => {
                c.fill(*r, FIELD);
                let v = match field {
                    Field::Node => f.node.clone(),
                    Field::User => f.user.clone(),
                    _ => "*".repeat(f.password.chars().count()),
                };
                let fit = (inner / ch - 1).max(1) as usize;
                let mut v = tail(&v, fit);
                if focused {
                    v.push('_');
                }
                c.text(r.x + pad, r.y + pad, &v, TEXT, s, inner);
            }
            Field::Connect | Field::Trust | Field::Open => {
                c.fill(*r, if f.busy { DIM } else { ACCENT });
                let t = match field {
                    Field::Connect if f.logged_in => "Reconnect",
                    Field::Connect => "Connect",
                    Field::Trust => "Trust certificate",
                    _ => "Open console",
                };
                c.text(r.x + pad, r.y + pad, t, DARK, s, inner);
            }
            Field::Profile => {
                c.fill(*r, FIELD);
                let t = format!("< {} >", PROFILES[f.profile].1);
                c.text(r.x + pad, r.y + pad, &t, TEXT, s, inner);
            }
            Field::List => {
                c.fill(*r, FIELD);
                for i in 0..l.list_rows {
                    let k = l.list_first + i;
                    let Some(vm) = f.vms.get(k) else { break };
                    let ry = r.y + 2 + i as i32 * l.row_h;
                    if k == f.selected {
                        c.fill(
                            Rect {
                                x: r.x + 2,
                                y: ry,
                                w: r.w - 4,
                                h: l.row_h,
                            },
                            if focused { ACCENT } else { DIM },
                        );
                    }
                    let col = if k == f.selected { DARK } else { TEXT };
                    let t = if vm.name.is_empty() || vm.name == vm.id {
                        vm.id.clone()
                    } else {
                        format!("{} ({})", vm.name, vm.id)
                    };
                    c.text(r.x + pad, ry + (l.row_h - ch) / 2, &t, col, s, inner);
                }
            }
        }
        if focused {
            // buttons are filled with the accent colour: outline them in
            // the text colour so the keyboard focus shows there too
            let col = match field {
                Field::Connect | Field::Trust | Field::Open => TEXT,
                _ => ACCENT,
            };
            c.frame(*r, col, s.max(1));
        }
    }
    let mut y = l.status_y;
    for t in &l.status_lines {
        c.text(m, y, t, if f.error { ERROR } else { OK }, s, c.w - 2 * m);
        y += ch + ch / 2;
    }
    for t in &l.detail_lines {
        c.text(m, y, t, DIM, s, c.w - 2 * m);
        y += ch + ch / 2;
    }
    if f.logged_in {
        let hint = if f.vms.is_empty() {
            format!(
                "No VM on this node can be shown in the Horcrux Console right now ({} VMs: stopped, or console off).",
                f.hidden
            )
        } else if f.hidden > 0 {
            format!(
                "{} other VMs are stopped or have no Horcrux Console.",
                f.hidden
            )
        } else {
            String::new()
        };
        c.text(m, l.hint_y, &hint, DIM, s, c.w - 2 * m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vms(n: usize) -> Vec<VmEntry> {
        (0..n)
            .map(|i| VmEntry {
                id: format!("vm-{i}"),
                name: format!("Machine {i}"),
            })
            .collect()
    }

    #[test]
    fn status_text_wraps() {
        assert_eq!(wrap("", 10, 3), Vec::<String>::new());
        assert_eq!(wrap("one two three four", 9, 3), vec!["one two", "three", "four"]);
        assert_eq!(wrap(&"ab".repeat(8), 6, 3), vec!["ababab", "ababab", "abab"]);
        assert_eq!(wrap("aaaa bbbb cccc dddd", 4, 2), vec!["aaaa", "b..."]);
        // a narrow window still shows the whole fingerprint
        let mut f = Form::new("https://n", "u", "");
        f.status = "The node's certificate is not trusted yet. Compare it with the node.".into();
        f.detail = "ab".repeat(32);
        let l = Layout::new(&f, 640, 480, 2);
        assert_eq!(l.detail_lines.concat(), "ab".repeat(32));
        assert!(l.status_lines.len() >= 2);
    }

    #[test]
    fn typing_tabbing_and_actions() {
        let mut f = Form::new("", "root", "");
        assert_eq!(f.focus, Some(Field::Node));
        for c in "https://n:8006".chars() {
            assert_eq!(f.key(Key::Char(c)), Action::None);
        }
        assert_eq!(f.node, "https://n:8006");
        f.key(Key::Backspace);
        f.key(Key::Char('6'));
        f.key(Key::Tab);
        f.key(Key::Tab);
        assert_eq!(f.focus, Some(Field::Password));
        f.key(Key::Char('s'));
        f.key(Key::Char('\u{8}')); // control characters are not text
        assert_eq!(f.password, "s");
        assert_eq!(f.key(Key::Enter), Action::Connect);
        f.set_vms(vms(3), 2);
        assert_eq!(f.focus, Some(Field::List));
        f.key(Key::Down);
        f.key(Key::Down);
        f.key(Key::Down);
        assert_eq!(f.selected_vm().unwrap().id, "vm-2");
        f.key(Key::Tab);
        assert_eq!(f.focus, Some(Field::Profile));
        f.key(Key::Right);
        f.key(Key::Right);
        assert_eq!(f.profile_name(), "gaming");
        f.key(Key::Left);
        assert_eq!(f.profile_name(), "desktop");
        assert_eq!(f.key(Key::Enter), Action::Open);
        // editing the login drops the old list
        f.focus = Some(Field::User);
        f.key(Key::Char('x'));
        assert!(!f.logged_in && f.vms.is_empty());
        assert_eq!(f.key(Key::Escape), Action::Quit);
        f.busy = true;
        assert_eq!(f.key(Key::Enter), Action::None);
    }

    #[test]
    fn link_preselects_and_starts_at_password() {
        let mut f = Form::new("https://n", "alice", "gaming");
        assert_eq!(f.focus, Some(Field::Password));
        assert_eq!(f.profile_name(), "gaming");
        f.want_vm = Some("vm-1".into());
        f.set_vms(vms(2), 0);
        assert_eq!(f.selected, 1);
    }

    #[test]
    fn layout_hits_and_draws() {
        let mut f = Form::new("https://n", "root", "");
        f.untrusted = Some("ab".repeat(32));
        f.set_vms(vms(40), 1);
        let (w, h) = (1280, 800);
        let l = Layout::new(&f, w, h, 2);
        let node = l.rect(Field::Node).unwrap();
        assert_eq!(l.hit(node.x + 3, node.y + 3), Some((Field::Node, None)));
        assert!(l.rect(Field::Trust).is_some());
        let list = l.rect(Field::List).unwrap();
        assert!(list.y + list.h < h, "list fits");
        let open = l.rect(Field::Open).unwrap();
        assert!(open.y + open.h <= h, "open button fits: {open:?}");
        // clicking a row selects it, a second click opens it
        let y2 = list.y + 2 + l.row_h + 1;
        assert_eq!(f.click(list.x + 5, y2, &l), Action::None);
        assert_eq!(f.selected, 1);
        assert_eq!(f.click(list.x + 5, y2, &l), Action::Open);
        assert_eq!(f.click(open.x + 1, open.y + 1, &l), Action::Open);
        let mut px = vec![0u32; (w * h) as usize];
        f.status = "connected".into();
        draw(&f, &l, &mut Canvas { px: &mut px, w, h });
        assert!(px.contains(&TEXT), "text drawn");
        assert!(px.contains(&ACCENT), "buttons drawn");
        // a tiny window still lays out and draws without panicking
        let l = Layout::new(&f, 100, 80, 1);
        let mut px = vec![0u32; 100 * 80];
        draw(
            &f,
            &l,
            &mut Canvas {
                px: &mut px,
                w: 100,
                h: 80,
            },
        );
    }
}
