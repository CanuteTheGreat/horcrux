//! Real-QEMU test of the Horcrux Console frame source (QEMU's D-Bus display):
//! frames arrive, keyboard input typed through the D-Bus keyboard logs in
//! and runs a command, and pointer moves and clicks sent through the D-Bus
//! mouse reach the guest kernel (`od` on `/dev/input/mice` prints them, so
//! the picture changes). Needs QEMU with the dbus display module and the
//! Alpine "virt" ISO (any x86_64 Linux live ISO with a root console works):
//!
//!     HXC_TEST_ISO=/path/alpine-virt.iso \
//!       cargo test -p horcrux-vm --features qemu --test hxc_dbus_e2e -- --ignored --nocapture
//!
//! `HXC_TEST_TABLET=1` adds a USB tablet (absolute pointer) to the guest;
//! `HXC_TEST_BOOT_SECS` (default 35) is the wait for the login prompt.
//!
//! Why not a desktop guest: Tiny Core's Xvesa ignores both PS/2 and tablet
//! input under QEMU (QMP input-send-event does not move its pointer either),
//! so its desktop picture proves nothing about the input path.
#![cfg(feature = "qemu")]

use horcrux_vm::console::hxc::keymap;
use horcrux_vm::console::hxc::qemu_dbus::{
    qmp_add_dbus_client, DbusDisplay, DisplayEvent, GuestInput,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

async fn quiet(updates: &AtomicU64, quiet: Duration, max: Duration) -> bool {
    let end = Instant::now() + max;
    let mut last = updates.load(Ordering::SeqCst);
    let mut since = Instant::now();
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = updates.load(Ordering::SeqCst);
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return true;
        }
    }
    false
}

/// Updates seen within `wait` after `action`.
async fn changes_after<F, Fut>(updates: &AtomicU64, wait: Duration, action: F) -> u64
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let before = updates.load(Ordering::SeqCst);
    action().await;
    tokio::time::sleep(wait).await;
    updates.load(Ordering::SeqCst) - before
}

async fn click(d: &DbusDisplay) {
    d.input(GuestInput::ButtonPress(0)).await.expect("press");
    d.input(GuestInput::ButtonRelease(0))
        .await
        .expect("release");
}

/// DOM key code for one character of a shell command (lower case only).
fn code_of(c: char) -> String {
    match c {
        'a'..='z' => format!("Key{}", c.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{c}"),
        ' ' => "Space".into(),
        '-' => "Minus".into(),
        '/' => "Slash".into(),
        '.' => "Period".into(),
        '\n' => "Enter".into(),
        other => panic!("no key for {other:?}"),
    }
}

async fn type_text(d: &DbusDisplay, text: &str) {
    let shift = keymap::qnum("ShiftLeft").expect("qnum");
    for c in text.chars() {
        // '>' and '_' are shifted keys on a US layout
        let (code, shifted) = match c {
            '>' => ("Period".to_string(), true),
            '_' => ("Minus".to_string(), true),
            c => (code_of(c), false),
        };
        let q = keymap::qnum(&code).expect("qnum");
        if shifted {
            d.input(GuestInput::KeyPress(shift)).await.unwrap();
        }
        d.input(GuestInput::KeyPress(q)).await.unwrap();
        d.input(GuestInput::KeyRelease(q)).await.unwrap();
        if shifted {
            d.input(GuestInput::KeyRelease(shift)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs QEMU with the D-Bus display and HXC_TEST_ISO"]
async fn dbus_display_frames_keyboard_and_pointer() {
    let iso = std::env::var("HXC_TEST_ISO").expect("HXC_TEST_ISO");
    let tablet = std::env::var("HXC_TEST_TABLET")
        .map(|v| v == "1")
        .unwrap_or(false);
    let boot: u64 = std::env::var("HXC_TEST_BOOT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35);
    let dir = tempfile::tempdir().unwrap();
    let qmp = dir.path().join("qmp.sock");
    let mut args: Vec<String> = [
        "-machine",
        "accel=kvm:tcg",
        "-m",
        "512",
        "-cdrom",
        &iso,
        "-vga",
        "std",
        "-display",
        "dbus,p2p=yes",
        "-qmp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("unix:{},server=on,wait=off", qmp.display()));
    if tablet {
        args.extend(["-device", "qemu-xhci", "-device", "usb-tablet"].map(String::from));
    }
    let mut qemu = tokio::process::Command::new("qemu-system-x86_64")
        .args(&args)
        .kill_on_drop(true)
        .spawn()
        .expect("qemu-system-x86_64");
    for _ in 0..100 {
        if qmp.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The desktop profile's display poll rate (QEMU builds without the
    // listener refresh-rate support ignore it).
    let poll_mhz = horcrux_common::console::ProfileSettings::default().display_poll_mhz();
    let (tx, mut rx) = mpsc::channel::<DisplayEvent>(64);
    let display = DbusDisplay::connect(&qmp, "t", tx, false, poll_mhz)
        .await
        .expect("connect");
    let updates = Arc::new(AtomicU64::new(0));
    let scanouts = Arc::new(AtomicU64::new(0));
    let (u, s) = (updates.clone(), scanouts.clone());
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DisplayEvent::Scanout { .. } => {
                    s.fetch_add(1, Ordering::SeqCst);
                    u.fetch_add(1, Ordering::SeqCst);
                }
                DisplayEvent::Update { .. } => {
                    u.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        scanouts.load(Ordering::SeqCst) >= 1,
        "a first frame (Scanout) arrives"
    );
    println!("ok - first frame arrived");

    // Boot to the login prompt and log in. The framebuffer console blinks
    // its cursor (a picture update every ~200 ms), so the first command
    // stops the blinking; only then is "no update" meaningful.
    tokio::time::sleep(Duration::from_secs(boot)).await;
    type_text(&display, "root\n").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    type_text(&display, "echo 0 >/sys/class/graphics/fbcon/cursor_blink\n").await;
    assert!(
        quiet(&updates, Duration::from_secs(2), Duration::from_secs(30)).await,
        "the console settles once the cursor stops blinking (the login worked)"
    );
    let mut echoed = 0;
    for c in "echo".chars() {
        let n = changes_after(&updates, Duration::from_millis(700), || async {
            type_text(&display, &c.to_string()).await;
        })
        .await;
        if n > 0 {
            echoed += 1;
        }
    }
    assert!(
        echoed >= 3,
        "typed characters echo at the shell ({echoed}/4)"
    );
    println!("ok - keyboard input reaches the guest ({echoed}/4 keystrokes echoed)");
    type_text(&display, "\nod -tx1 -w3 -v /dev/input/mice\n").await;
    assert!(
        quiet(&updates, Duration::from_secs(2), Duration::from_secs(20)).await,
        "the console settles after starting od"
    );

    let mut moved = 0;
    for (i, (x, y)) in [(200u32, 150u32), (800, 200), (700, 600), (250, 550)]
        .iter()
        .enumerate()
    {
        let d = &display;
        let n = changes_after(&updates, Duration::from_millis(1500), || async move {
            let abs = d.input(GuestInput::Abs(*x, *y)).await;
            let r = match &abs {
                Ok(()) => Ok(()),
                Err(_) => {
                    d.input(GuestInput::Rel(if i % 2 == 0 { 40 } else { -40 }, 20))
                        .await
                }
            };
            println!("move {i}: absolute={} result={r:?}", abs.is_ok());
        })
        .await;
        println!("move {i}: {n} display updates");
        if n > 0 {
            moved += 1;
        }
        quiet(
            &updates,
            Duration::from_millis(800),
            Duration::from_secs(10),
        )
        .await;
    }
    assert!(moved >= 3, "pointer moves reach the guest ({moved}/4)");
    println!("ok - pointer motion reaches the guest through D-Bus ({moved}/4)");

    let d = &display;
    let clicks = changes_after(&updates, Duration::from_millis(1500), || async move {
        d.input(GuestInput::ButtonPress(0)).await.unwrap();
        d.input(GuestInput::ButtonRelease(0)).await.unwrap();
    })
    .await;
    assert!(clicks > 0, "a click reaches the guest");
    println!("ok - button press/release reaches the guest ({clicks} updates)");

    // A second viewer of the same VM. QEMU's p2p D-Bus display serves its
    // objects on only the newest client connection, so a second connection
    // per viewer would cut the first viewer's keyboard and mouse off.
    let (tx2, mut rx2) = mpsc::channel::<DisplayEvent>(64);
    let second = DbusDisplay::connect(&qmp, "t2", tx2, false, poll_mhz)
        .await
        .expect("second viewer connects");
    let updates2 = Arc::new(AtomicU64::new(0));
    let u2 = updates2.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx2.recv().await {
            if matches!(
                ev,
                DisplayEvent::Scanout { .. } | DisplayEvent::Update { .. }
            ) {
                u2.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    quiet(
        &updates,
        Duration::from_millis(800),
        Duration::from_secs(10),
    )
    .await;
    let n = changes_after(&updates, Duration::from_millis(1500), || click(&display)).await;
    assert!(
        n > 0,
        "the first viewer's input still reaches the guest after a second viewer connected"
    );
    let n2 = updates2.load(Ordering::SeqCst);
    assert!(
        n2 > 0,
        "the second viewer sees the guest's picture too ({n2} updates)"
    );
    println!(
        "ok - two viewers: the first one's input still works ({n} updates; second viewer saw {n2})"
    );
    quiet(
        &updates,
        Duration::from_millis(800),
        Duration::from_secs(10),
    )
    .await;
    let n = changes_after(&updates, Duration::from_millis(1500), || click(&second)).await;
    assert!(n > 0, "the second viewer's input reaches the guest");
    drop(second);
    quiet(
        &updates,
        Duration::from_millis(800),
        Duration::from_secs(10),
    )
    .await;
    let n = changes_after(&updates, Duration::from_millis(1500), || click(&display)).await;
    assert!(
        n > 0,
        "the first viewer's input works after the second viewer left"
    );
    println!("ok - second viewer's input works; the first still works after it left");

    // Another D-Bus client outside Horcrux takes QEMU's display objects:
    // the viewer reconnects on its next input instead of going deaf.
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("pair");
    let theirs = std::os::fd::OwnedFd::from(theirs);
    let q = qmp.clone();
    tokio::task::spawn_blocking(move || qmp_add_dbus_client(&q, &theirs, "outsider"))
        .await
        .expect("join")
        .expect("QEMU accepts another D-Bus client");
    ours.set_nonblocking(true).expect("nonblocking");
    // the outsider completes the D-Bus handshake, so QEMU moves its objects
    let _outsider = zbus::connection::Builder::unix_stream(
        tokio::net::UnixStream::from_std(ours).expect("tokio stream"),
    )
    .p2p()
    .build()
    .await
    .expect("outsider handshake");
    tokio::time::sleep(Duration::from_millis(500)).await;
    quiet(
        &updates,
        Duration::from_millis(800),
        Duration::from_secs(10),
    )
    .await;
    let n = changes_after(&updates, Duration::from_millis(1500), || click(&display)).await;
    assert!(
        n > 0,
        "input works again after another client took the D-Bus display"
    );
    println!("ok - input reconnects after another D-Bus client took the display ({n} updates)");
    drop(display);
    let _ = qemu.kill().await;
}
