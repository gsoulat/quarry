//! Icône de l'application, générée par `examples/gen_icon.rs`.

use iced::window;

const ICON_256: &[u8] = include_bytes!("../assets/icon-256.png");

/// Icône de fenêtre (barre des tâches sous Windows et Linux).
pub fn window_icon() -> Option<window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_256));
    let mut reader = decoder.read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    window::icon::from_rgba(rgba, info.width, info.height).ok()
}

/// Icône du Dock sur macOS. Hors d'un bundle `.app`, macOS affiche sinon
/// l'icône générique d'exécutable ; on la remplace au lancement.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const ICON_1024: &[u8] = include_bytes!("../assets/icon.png");

    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(ICON_1024);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else { return };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY : appelé sur le thread principal avec une image valide.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}
