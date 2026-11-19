//! Real-QEMU test of the Horcrux Console frame source (QEMU's D-Bus display):
//! frames arrive, and keyboard and pointer input through the D-Bus
//! interfaces change the guest's picture. Needs QEMU with the dbus display
//! module and a bootable ISO with a graphical desktop that draws its own
//! pointer (Tiny Core Linux in CI):
//!
//!     HXC_TEST_ISO=/path/TinyCore-current.iso \
//!       cargo test -p horcrux-vm --features qemu --test hxc_dbus_e2e -- --ignored --nocapture
//!
//! `HXC_TEST_TABLET=1` adds a USB tablet (absolute pointer) to the guest.
#![cfg(feature = "qemu")]

use horcrux_vm::console::hxc::qemu_dbus::{DbusDisplay, DisplayEvent};
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs QEMU with the D-Bus display and HXC_TEST_ISO"]
async fn dbus_display_frames_keyboard_and_pointer() {
    let iso = std::env::var("HXC_TEST_ISO").expect("HXC_TEST_ISO");
    let tablet = std::env::var("HXC_TEST_TABLET").map(|v| v == "1").unwrap_or(false);
    let dir = tempfile::tempdir().unwrap();
    let qmp = dir.path().join("qmp.sock");
    let mut args: Vec<String> = [
        "-machine", "accel=kvm:tcg", "-m", "768", "-cdrom", &iso, "-vga", "std",
        "-display", "dbus,p2p=yes", "-qmp",
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

    let (tx, mut rx) = mpsc::channel::<DisplayEvent>(64);
    let display = DbusDisplay::connect(&qmp, "t", tx, false).await.expect("connect");
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
    assert!(scanouts.load(Ordering::SeqCst) >= 1, "a first frame (Scanout) arrives");
    println!("ok - first frame arrived");

    // boot to the desktop: Enter at the boot prompt, then wait until the
    // picture stays still for 8 s
    display.keyboard.press(28).await.unwrap();
    display.keyboard.release(28).await.unwrap();
    tokio::time::sleep(Duration::from_secs(25)).await;
    assert!(
        quiet(&updates, Duration::from_secs(8), Duration::from_secs(240)).await,
        "the desktop settles"
    );
    println!("ok - desktop settled (absolute pointer: {})", display.absolute);

    let mut moved = 0;
    for (i, (x, y)) in [(200u32, 150u32), (800, 200), (700, 600), (250, 550)].iter().enumerate() {
        let d = &display;
        let n = changes_after(&updates, Duration::from_millis(1500), || async move {
            let abs = d.mouse.set_abs_position(*x, *y).await;
            let r = match abs {
                Ok(()) => Ok(()),
                Err(_) => d.mouse.rel_motion(if i % 2 == 0 { 120 } else { -120 }, 80).await,
            };
            println!("move {i}: abs={:?} result={r:?}", abs.is_ok());
        })
        .await;
        println!("move {i}: {n} display updates");
        if n > 0 {
            moved += 1;
        }
        quiet(&updates, Duration::from_millis(800), Duration::from_secs(10)).await;
    }
    assert!(moved >= 3, "pointer moves change the picture ({moved}/4)");
    println!("ok - pointer input reaches the guest through D-Bus ({moved}/4)");
    drop(display);
    let _ = qemu.kill().await;
}
