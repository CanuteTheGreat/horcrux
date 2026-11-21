//! macOS: `horcrux-console://` links.
//!
//! Launch Services does not put a clicked link on the command line: it
//! starts (or activates) the app and sends it an Apple Event of class
//! `kInternetEventClass`, id `kAEGetURL`, whose direct object is the URL.
//! winit installs no handler for it, so this registers one with
//! `NSAppleEventManager` before the event loop runs and hands the URL to
//! the app as `UserEvent::Link` (parsed and checked there with
//! `config::parse_link`, like a link on the command line).
//!
//! Compile-checked only (clippy for aarch64/x86_64-apple-darwin in
//! console-desktop.yml): no macOS runner exists for this repo.
use crate::UserEvent;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, sel, AllocAnyThread};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use std::sync::{Mutex, OnceLock};
use winit::event_loop::EventLoopProxy;

/// `kInternetEventClass` and `kAEGetURL` ('GURL').
const GURL: u32 = u32::from_be_bytes(*b"GURL");
/// `keyDirectObject` ('----').
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class adds one
    // method and no instance variables.
    #[unsafe(super(NSObject))]
    #[name = "HorcruxConsoleLinkHandler"]
    struct LinkHandler;

    impl LinkHandler {
        /// `-handleGetURLEvent:withReplyEvent:` (the selector registered
        /// with the Apple Event manager below).
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            if let Some(url) = url_of(event) {
                deliver(url);
            }
        }
    }
);

impl LinkHandler {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: plain NSObject init of a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

fn url_of(event: &NSAppleEventDescriptor) -> Option<String> {
    // -paramDescriptorForKeyword: (AEKeyword = FourCharCode); sent directly
    // so the CoreServices bindings are not needed for one constant type.
    // SAFETY: the selector takes a u32 keyword and returns a descriptor or nil.
    let d: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    Some(d?.stringValue()?.to_string())
}

fn deliver(url: String) {
    if let Some(p) = PROXY.get() {
        if let Ok(p) = p.lock() {
            let _ = p.send_event(UserEvent::Link(url));
        }
    }
}

/// Register the `kAEGetURL` handler. Call once, before `run_app`.
pub fn install(proxy: EventLoopProxy<UserEvent>) {
    if PROXY.set(Mutex::new(proxy)).is_err() {
        return;
    }
    let handler = LinkHandler::new();
    let mgr = NSAppleEventManager::sharedAppleEventManager();
    // SAFETY: the handler object implements the selector with the
    // signature the Apple Event manager calls; it is kept alive for the
    // life of the process below (the manager does not retain it).
    // AEEventClass / AEEventID are FourCharCodes (u32).
    unsafe {
        let _: () = msg_send![
            &*mgr,
            setEventHandler: &*handler,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: GURL,
            andEventID: GURL
        ];
    }
    std::mem::forget(handler);
}
