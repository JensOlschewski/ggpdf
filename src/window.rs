//! Native window setup: the OS-level title/app-id, the installed-app
//! resource directory, and the window icon. Isolated from `main.rs`
//! because all three are startup-only, platform-flavored details that
//! don't change once the window exists.

use eframe::egui;
use std::env;
use std::path::PathBuf;

pub const WINDOW_TITLE: &str = "ggPdf";
pub const WINDOW_APP_ID: &str = "local.jens.ggpdf.native";

/// Resources shipped with an installed app. Development builds keep using
/// paths relative to the current working directory.
pub fn bundled_resources() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    #[cfg(target_os = "windows")]
    {
        let directory = executable.parent()?;
        return (directory.join("pdfium/pdfium.dll").is_file()
            && directory.join("assets/test-pdfs/letter.pdf").is_file())
            .then(|| directory.to_path_buf());
    }
    #[cfg(target_os = "macos")]
    {
        let macos = executable.parent()?;
        let contents = macos.parent()?;
        (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents")
            .then(|| contents.join("Resources"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Decodes `icon.png` into the raw RGBA egui/winit wants for a window/app
/// icon. Checked first in the installed resources (`bundled_resources()`),
/// then relative to the dev checkout — so `cargo
/// run` picks up the same icon without needing it installed anywhere.
///
/// Without this, `eframe::NativeOptions::viewport` has no icon set, and
/// `AppTitleIconSetter` (see eframe's `native/app_icon.rs`) falls back to
/// egui's own generic icon — then calls macOS's `NSApplication::
/// setApplicationIconImage` with *that* fallback, overwriting the Dock
/// icon the `.app` bundle's own `CFBundleIconFile` would otherwise show.
/// Found live: the Dock/app-switcher icon after launching the installed
/// app was egui's generic icon, not `ggPdf.icns`.
pub fn load_app_icon() -> Option<egui::IconData> {
    let candidates = [bundled_resources().map(|r| r.join("icon.png")), Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icon.png"))];
    for path in candidates.into_iter().flatten() {
        if let Ok(image) = image::open(&path) {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            return Some(egui::IconData { rgba: rgba.into_raw(), width, height });
        }
    }
    None
}

pub fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_app_id(WINDOW_APP_ID).with_inner_size([1200.0, 800.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions {
        // eframe persists the outer window geometry under this stable ID.
        // This initial size matters only before any window state is saved.
        viewport,
        persist_window: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod window_options_tests {
    use super::*;

    #[test]
    fn window_geometry_is_saved_under_stable_id_without_forcing_maximize() {
        let options = native_options();
        assert_eq!(options.viewport.app_id.as_deref(), Some(WINDOW_APP_ID));
        assert_eq!(options.viewport.inner_size, Some(egui::vec2(1200.0, 800.0)));
        assert_ne!(options.viewport.maximized, Some(true));
        assert!(options.persist_window);
    }

    #[test]
    fn app_icon_loads_from_the_dev_checkout_so_cargo_run_also_gets_it() {
        let icon = load_app_icon().expect("icon.png should decode from the repo root");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
