// macOS `aether://` URL scheme receiver. The bundle's Info.plist registers the
// scheme (CFBundleURLTypes); the OS delivers opens to the running app as the
// kAEGetURL Apple Event. We install a handler object on NSAppleEventManager and
// forward each URL to the App as WorkerMsg::OpenUrl — which relays it to the
// extension host (window.registerUriHandler), completing OAuth deep links like
// Cline's `aether://saoudrizwan.claude-dev/auth?...`.

#![cfg(target_os = "macos")]

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use winit::event_loop::EventLoopProxy;

use crate::marketplace::WorkerMsg;

static SINK: OnceLock<Mutex<(Sender<WorkerMsg>, Option<EventLoopProxy<()>>)>> = OnceLock::new();

fn deliver(url: String) {
    if let Some(sink) = SINK.get() {
        let guard = sink.lock().unwrap();
        let _ = guard.0.send(WorkerMsg::OpenUrl(url));
        if let Some(p) = guard.1.as_ref() {
            let _ = p.send_event(());
        }
    }
}

mod handler {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel, ClassType, DefinedClass};

    // Four-char codes for the "open URL" Apple Event.
    const K_INTERNET_EVENT_CLASS: u32 = u32::from_be_bytes(*b"GURL");
    const K_AE_GET_URL: u32 = u32::from_be_bytes(*b"GURL");
    const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    objc2::define_class!(
        #[unsafe(super(objc2_foundation::NSObject))]
        #[name = "AetherURLHandler"]
        pub struct UrlHandler;

        impl UrlHandler {
            #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
            fn handle_get_url(&self, event: *mut AnyObject, _reply: *mut AnyObject) {
                unsafe {
                    if event.is_null() {
                        return;
                    }
                    // -[NSAppleEventDescriptor paramDescriptorForKeyword:] → stringValue
                    let param: *mut AnyObject =
                        msg_send![&*event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT];
                    if param.is_null() {
                        return;
                    }
                    let s: *mut objc2_foundation::NSString = msg_send![&*param, stringValue];
                    if s.is_null() {
                        return;
                    }
                    super::deliver((*s).to_string());
                }
            }
        }
    );

    pub fn install() {
        unsafe {
            let mgr: *mut AnyObject =
                msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
            let h: Retained<UrlHandler> = msg_send![UrlHandler::class(), new];
            let _: () = msg_send![
                &*mgr,
                setEventHandler: &*h,
                andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: K_INTERNET_EVENT_CLASS,
                andEventID: K_AE_GET_URL
            ];
            // The manager keeps only a weak-ish reference historically — leak the
            // handler so it lives for the process (one tiny object).
            std::mem::forget(h);
        }
    }
}

/// Install the aether:// Apple Event handler. Call once at startup, after the
/// worker channel exists. No-op if called twice.
pub fn install(tx: Sender<WorkerMsg>, proxy: Option<EventLoopProxy<()>>) {
    if SINK.set(Mutex::new((tx, proxy))).is_ok() {
        handler::install();
    }
}
