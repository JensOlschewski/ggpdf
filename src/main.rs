//! Entry point: loads config and PDFium, then either runs a headless dev
//! tool (`--dump`/`--profile <path>`) or launches the native window. The
//! live app itself is `app_state::SpikeApp` (state) + `ui` (the egui
//! frame loop) — see those modules for how a document actually gets on
//! screen.

// Installed release builds are GUI apps: opening a PDF must not spawn a console.
// Keep a console in debug builds for cargo run diagnostics.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod app_state;
mod color;
mod config;
mod dev_tools;
mod document;
mod input;
#[cfg(target_os = "macos")]
mod macos_documents;
mod render;
mod render_worker;
mod search;
mod selection;
mod text;
mod ui;
mod viewer;
mod viewport;
mod window;

use app_state::SpikeApp;
use document::PdfLibrary;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use window::{bundled_resources, native_options, WINDOW_TITLE};

fn main() -> eframe::Result<()> {
    let startup = std::time::Instant::now();
    let cfg = config::load_or_default();
    let config_elapsed = startup.elapsed();
    let total_bindings: usize = cfg.keys.values().map(|m| m.len()).sum();
    println!(
        "ggpdf: loaded config — {} modes, {total_bindings} key bindings, page_gap={}",
        cfg.keys.len(),
        cfg.appearance.page_gap
    );

    let lib_dir = env::var_os("PDFIUM_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| bundled_resources().map_or_else(|| PathBuf::from("pdfium"), |resources| resources.join("pdfium")));
    let library = PdfLibrary::load(&lib_dir)
        .unwrap_or_else(|e| panic!("failed to load PDFium from {lib_dir:?}: {e}. Set PDFIUM_LIB_DIR."));
    let library_elapsed = startup.elapsed();
    let library: &'static PdfLibrary = Box::leak(Box::new(library));

    let args: Vec<String> = env::args().skip(1).collect();
    let dump = args.iter().any(|a| a == "--dump");
    let profile_flag_index = args.iter().position(|a| a == "--profile");
    let profile_path = profile_flag_index.and_then(|i| args.get(i + 1)).cloned();
    let file_args: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| a.as_str() != "--dump" && *i != profile_flag_index.unwrap_or(usize::MAX) && *i != profile_flag_index.map_or(usize::MAX, |fi| fi + 1))
        .map(|(_, a)| a.clone())
        .collect();

    if dump {
        dev_tools::dump_fixtures(library).expect("dump failed");
        return Ok(());
    }
    if let Some(path) = profile_path {
        println!("startup before document: config {:.1?}, PDFium {:.1?}", config_elapsed, library_elapsed - config_elapsed);
        dev_tools::profile_document(library, &path).unwrap_or_else(|e| println!("profile failed: {e}"));
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
        let mut event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = Arc::new(std::sync::Mutex::new(None));
        macos_documents::install(sender, Arc::clone(&repaint));
        let mut app = eframe::create_native(
            WINDOW_TITLE,
            native_options(),
            Box::new(move |cc| {
                *repaint.lock().unwrap() = Some(cc.egui_ctx.clone());
                // Finder can send the cold-launch event before eframe creates
                // its UI. Consume it now, before deciding on a sample tab.
                let initial_events: Vec<_> = receiver.try_iter().flatten().collect();
                let mut paths = file_args.clone();
                paths.extend(initial_events.iter().map(|p| p.to_string_lossy().into_owned()));
                let mut viewer = SpikeApp::new(library, cfg, &paths);
                viewer.document_events = Some(receiver);
                Ok(Box::new(viewer))
            }),
            &event_loop,
        );
        let result = event_loop.run_app_on_demand(&mut app);
        result?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    eframe::run_native(
        WINDOW_TITLE,
        native_options(),
        Box::new(move |_cc| Ok(Box::new(SpikeApp::new(library, cfg, &file_args)))),
    )
}
