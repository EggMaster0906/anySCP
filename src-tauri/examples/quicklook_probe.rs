//! Native smoke probe, never registered as app IPC. Run on a logged-in Mac:
//! cargo run --example quicklook_probe
#[cfg(target_os = "macos")]
#[path = "../src/quicklook/macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{MainThreadMarker, NSPoint, NSString};
    use objc2_quick_look_ui::QLPreviewPanel;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("Quick Look 測試.txt");
    std::fs::write(&path, "anySCP native Quick Look responder probe\n").unwrap();
    let closed = Arc::new(AtomicUsize::new(0));
    tauri::Builder::default().setup(move |app| {
        let window = WebviewWindowBuilder::new(app, "probe", WebviewUrl::App("index.html".into()))
            .title("anySCP Quick Look native probe").build()?;
        let handle = app.handle().clone();
        let closed = closed.clone();
        std::thread::spawn(move || {
            // Keep fixture alive for every preview.
            let _fixture = fixture;
            for cycle in 0..20 {
                std::thread::sleep(Duration::from_millis(350));
                let w = window.clone();
                let p = path.clone();
                let counter = closed.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                handle.run_on_main_thread(move || {
                    let result = macos::show(&w, cycle.to_string(), &p, Box::new(move || {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }));
                    tx.send(result).unwrap();
                }).unwrap();
                rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
                std::thread::sleep(Duration::from_millis(400));
                let (tx, rx) = std::sync::mpsc::channel();
                handle.run_on_main_thread(move || {
                    let mtm = MainThreadMarker::new().unwrap();
                    let panel = unsafe { QLPreviewPanel::sharedPreviewPanel(mtm).unwrap() };
                    assert!(panel.isVisible(), "panel not visible in cycle {cycle}");
                    assert!(unsafe { panel.currentController() }.is_some());
                    assert!(unsafe { panel.currentPreviewItem() }.is_some());
                    if cycle % 3 == 2 {
                        panel.performClose(None);
                    } else {
                        let key = if cycle % 3 == 0 { " " } else { "\u{1b}" };
                        let text = NSString::from_str(key);
                        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                            NSEventType::KeyDown, NSPoint::ZERO, NSEventModifierFlags::empty(), 0.0,
                            panel.windowNumber(), None, &text, &text, false,
                            if cycle % 3 == 0 { 49 } else { 53 },
                        ).unwrap();
                        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
                    }
                    tx.send(()).unwrap();
                }).unwrap();
                rx.recv_timeout(Duration::from_secs(10)).unwrap();
                std::thread::sleep(Duration::from_millis(250));
                assert_eq!(closed.load(Ordering::SeqCst), cycle + 1, "close callback missing");
                let (tx, rx) = std::sync::mpsc::channel();
                handle.run_on_main_thread(move || {
                    macos::close(Some(&cycle.to_string()));
                    tx.send(()).unwrap();
                }).unwrap();
                rx.recv_timeout(Duration::from_secs(10)).unwrap();
                eprintln!("Quick Look native cycle {}/20 passed", cycle + 1);
            }
            eprintln!("PASS: 20 native show/control/Space/Esc/close/cleanup cycles");
            handle.exit(0);
        });
        Ok(())
    }).build(tauri::generate_context!()).unwrap().run(|_, _| {});
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Native Quick Look probe requires macOS");
}
