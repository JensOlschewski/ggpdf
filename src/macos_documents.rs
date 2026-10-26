//! Attach document callbacks to winit's *existing* NSApplicationDelegate.
//! Replacing it crashes winit when it processes its next event.
use objc2::ffi;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, MainThreadMarker, NSString, NSURL};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

type Delivery = (Sender<Vec<PathBuf>>, Arc<Mutex<Option<eframe::egui::Context>>>);
static DELIVERY: OnceLock<Delivery> = OnceLock::new();

fn enqueue(paths: Vec<PathBuf>) {
    if paths.is_empty() { return; }
    if let Some((sender, repaint)) = DELIVERY.get() {
        let _ = sender.send(paths);
        if let Some(ctx) = repaint.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }
}

unsafe extern "C" fn open_file(
    _delegate: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, filename: *mut NSString,
) -> bool {
    // AppKit guarantees the filename lives throughout this delegate call.
    if let Some(filename) = unsafe { filename.as_ref() } {
        enqueue(vec![PathBuf::from(filename.to_string())]);
        true
    } else { false }
}

unsafe extern "C" fn open_files(
    _delegate: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, filenames: *mut NSArray<NSString>,
) {
    if let Some(filenames) = unsafe { filenames.as_ref() } {
        enqueue(filenames.iter().map(|s| PathBuf::from(s.to_string())).collect());
    }
}

unsafe extern "C" fn open_urls(
    _delegate: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut NSArray<NSURL>,
) {
    if let Some(urls) = unsafe { urls.as_ref() } {
        enqueue(urls.iter().filter_map(|url| {
            unsafe { url.isFileURL().then(|| url.path()) }
                .flatten().map(|path| PathBuf::from(path.to_string()))
        }).collect());
    }
}

/// Winit 0.30 installs and checks its own delegate. Add missing selectors to
/// that class; do not call setDelegate. Must run after EventLoop::build.
pub fn install(sender: Sender<Vec<PathBuf>>, repaint: Arc<Mutex<Option<eframe::egui::Context>>>) {
    let mtm = MainThreadMarker::new().expect("macOS UI must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = unsafe { app.delegate() }.expect("winit delegate not installed");
    let class: &objc2::runtime::AnyClass = unsafe { msg_send![&*delegate, class] };
    let class = class as *const _ as *mut ffi::objc_class;
    // Objective-C method encodings: BOOL/void, self, selector, app, argument.
    // Winit's class is registered, but these selectors are not implemented.
    //
    // `class_addMethod` returns objc2's `Bool` typedef, which is a plain
    // `bool` on arm64 but a C `signed char`-sized integer on x86_64 (the
    // Objective-C runtime's own ABI split) — comparing it against the
    // integer literal `0` only type-checked on whichever architecture was
    // tried first. `i32::from` has a `std`-provided conversion for both,
    // so the comparison compiles on either.
    unsafe {
        assert_ne!(i32::from(ffi::class_addMethod(class, sel!(application:openFile:).as_ptr(),
            Some(std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSString) -> bool, unsafe extern "C" fn()>(open_file)),
            c"c@:@@".as_ptr())), 0);
        assert_ne!(i32::from(ffi::class_addMethod(class, sel!(application:openFiles:).as_ptr(),
            Some(std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSString>), unsafe extern "C" fn()>(open_files)),
            c"v@:@@".as_ptr())), 0);
        assert_ne!(i32::from(ffi::class_addMethod(class, sel!(application:openURLs:).as_ptr(),
            Some(std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>), unsafe extern "C" fn()>(open_urls)),
            c"v@:@@".as_ptr())), 0);
    }
    DELIVERY.set((sender, repaint)).expect("document handler registered twice");
}
