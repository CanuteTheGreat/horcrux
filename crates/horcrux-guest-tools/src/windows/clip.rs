//! Clipboard text (CF_UNICODETEXT) get/set and change watching.
//!
//! One thread owns a message-only window (a clipboard owner is needed for
//! SetClipboardData) and polls GetClipboardSequenceNumber.
use crate::agent::Out;
use horcrux_guest_proto::{Clipboard, Msg};
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CF_UNICODETEXT: u32 = 13;

pub enum Cmd {
    Set(String),
    Watch(Out, usize),
}

/// Start the clipboard thread; it ends when the sender is dropped.
pub fn spawn(poll_ms: u64) -> mpsc::Sender<Cmd> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    std::thread::spawn(move || run(rx, poll_ms));
    tx
}

fn run(rx: mpsc::Receiver<Cmd>, poll_ms: u64) {
    // SAFETY: STATIC is a system class; HWND_MESSAGE makes a message-only window.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            windows::core::w!("horcrux-clipboard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        )
    }
    .unwrap_or_default();
    let mut watch: Option<(Out, usize)> = None;
    // Text this node put on / read from the clipboard last: never sent back.
    let mut last = String::new();
    // A guest clipboard change the port queue had no room for yet.
    let mut pending: Option<String> = None;
    // SAFETY: no arguments.
    let mut seq = unsafe { GetClipboardSequenceNumber() };
    loop {
        match rx.recv_timeout(Duration::from_millis(poll_ms)) {
            Ok(Cmd::Set(t)) => {
                if set_text(hwnd, &t) {
                    // the node's text replaces an older guest copy not sent yet
                    pending = None;
                    last = normalize(&t);
                    // SAFETY: no arguments.
                    seq = unsafe { GetClipboardSequenceNumber() };
                }
            }
            Ok(Cmd::Watch(o, max)) => {
                watch = Some((o, max));
                pending = None;
                // Only changes from now on are sent.
                // SAFETY: no arguments.
                seq = unsafe { GetClipboardSequenceNumber() };
                last = get_text(hwnd).map(|t| normalize(&t)).unwrap_or_default();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // SAFETY: no arguments.
        let now = unsafe { GetClipboardSequenceNumber() };
        if now != seq {
            seq = now;
            if let (Some((_, max)), Some(t)) = (&watch, get_text(hwnd)) {
                let t = normalize(&t);
                if !t.is_empty() && t != last && t.len() <= *max {
                    last = t.clone();
                    // a newer copy replaces one not sent yet
                    pending = Some(t);
                }
            }
        }
        // The port queue is shared with video and audio, which keep it full
        // while the node reads slower than the guest encodes. Those drop
        // packets; a clipboard change must not be lost, so it stays pending
        // and is retried every poll until the writer takes it (without
        // blocking: clipboard writes from the node keep being served).
        if let (Some((out, _)), Some(t)) = (&watch, pending.take()) {
            match out.try_send(Msg::Clipboard(Clipboard {
                mime: "text/plain".into(),
                text: t,
            })) {
                Ok(()) => {}
                Err(TrySendError::Full(Msg::Clipboard(c))) => pending = Some(c.text),
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => watch = None,
            }
        }
    }
    if !hwnd.is_invalid() {
        // SAFETY: the window was created by this thread.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Wire form uses `\n`; Windows applications want `\r\n`.
fn normalize(t: &str) -> String {
    t.replace("\r\n", "\n")
}

fn open(hwnd: HWND) -> bool {
    for _ in 0..20 {
        // SAFETY: hwnd is a window of this thread or null.
        if unsafe { OpenClipboard(hwnd) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn get_text(hwnd: HWND) -> Option<String> {
    if !open(hwnd) {
        return None;
    }
    let mut out = None;
    // SAFETY: the clipboard is open; the global handle is locked while read.
    unsafe {
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if !p.is_null() {
                let max = GlobalSize(g) / 2;
                let mut n = 0;
                while n < max && *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(g);
            }
        }
        let _ = CloseClipboard();
    }
    out
}

fn set_text(hwnd: HWND, text: &str) -> bool {
    let t = normalize(text).replace('\n', "\r\n");
    let w: Vec<u16> = t.encode_utf16().chain(std::iter::once(0)).collect();
    if !open(hwnd) {
        return false;
    }
    let mut ok = false;
    // SAFETY: the clipboard is open; on success the system owns the block.
    unsafe {
        let _ = EmptyClipboard();
        if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(g) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(g);
                ok = SetClipboardData(CF_UNICODETEXT, HANDLE(g.0)).is_ok();
            }
            if !ok {
                let _ = GlobalFree(g);
            }
        }
        let _ = CloseClipboard();
    }
    let _ = PCWSTR::null();
    ok
}
