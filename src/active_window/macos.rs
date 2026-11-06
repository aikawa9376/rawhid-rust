//! Workspace notifications avoid window enumeration and focus polling.
use std::{ptr::NonNull, sync::mpsc::Sender};

use block2::RcBlock;
use objc2::{rc::autoreleasepool, MainThreadMarker};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSNotification, NSRunLoop};

fn send_app_name(tx: &Sender<String>, app: &NSRunningApplication) {
    if let Some(name) = app.localizedName() {
        let name = name.to_string();
        if !name.is_empty() {
            let _ = tx.send(name);
        }
    }
}

pub fn watch_app_names(tx: Sender<String>) {
    let _main_thread = MainThreadMarker::new().expect("workspace watcher requires the main thread");
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();
        let events = tx.clone();
        let callback = RcBlock::new(move |notification: NonNull<NSNotification>| {
            autoreleasepool(|_| {
                // SAFETY: The notification center supplies a live notification
                // for the duration of this callback. The key is an AppKit constant.
                let notification = unsafe { notification.as_ref() };
                let Some(info) = notification.userInfo() else {
                    return;
                };
                let Some(app) = info.objectForKey(unsafe { NSWorkspaceApplicationKey }) else {
                    return;
                };
                if let Some(app) = app.downcast_ref::<NSRunningApplication>() {
                    send_app_name(&events, app);
                }
            });
        });
        // Subscribe before reading initial focus so startup cannot miss a switch.
        // SAFETY: No object filter or operation queue is used. The callback only
        // captures a Send + Sync channel sender and borrows notification objects.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &callback,
            )
        };
        if let Some(app) = workspace.frontmostApplication() {
            send_app_name(&tx, &app);
        }

        let run_loop = NSRunLoop::mainRunLoop();
        loop {
            // Drain temporary Cocoa objects after each event. With no timer the
            // main thread sleeps until a run-loop source delivers an event.
            let handled = autoreleasepool(|_| unsafe {
                run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &NSDate::distantFuture())
            });
            if !handled {
                break;
            }
        }
        // SAFETY: This is the observer registered above, with no callbacks left
        // running on this thread after the run loop stops.
        unsafe { center.removeObserver((*observer).as_ref()) };
    });
}
