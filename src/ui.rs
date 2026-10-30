//! The frame-by-frame UI: `impl eframe::App for SpikeApp`. Reads input
//! (chrome shortcuts, the modal dispatcher, search typing), then paints
//! the tab bar, the scrolling page list with its overlays, and the
//! transient notice box. State mutation lives in `app_state.rs`; this
//! file is the egui glue around it.

use crate::app_state::SpikeApp;
use crate::color::parse_hex_color;
use crate::search::SearchDirection;
use crate::viewport::{to_egui_rect, zoom_step, HALF_PAGE_SCROLL, SCALE};
use crate::{config, input, selection};
use eframe::egui;

/// Asks for another frame, same as `ctx.request_repaint()`, except
/// while this window isn't focused: then it asks again only after a
/// short delay instead of immediately, so background work (filling in
/// pages, a notice-box countdown) still completes without this window
/// continuously demanding frames from the OS compositor while some
/// *other* app has focus.
fn request_repaint(ctx: &egui::Context) {
    if ctx.input(|i| i.focused) {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

impl eframe::App for SpikeApp {
    #[cfg(target_os = "macos")]
    fn on_exit(&mut self) {
        // State is saved by now. Returning to winit's run-on-demand loop lets
        // AppKit's Touch Bar finder unregister KVO observers from deallocated
        // views and abort on Touch Bar Macs linked against older SDKs. Exit
        // like eframe's `run_and_return: false` mode does.
        std::process::exit(0);
    }

    // Only restore the outer window. Persisting egui's ScrollArea memory
    // across launches would reuse tab indices for potentially different
    // documents; per-tab scroll positions still work within one session.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_os = "macos")]
        if let Some(receiver) = &self.document_events {
            let batches: Vec<_> = receiver.try_iter().collect();
            for paths in batches {
                self.open_document_event(paths, &ctx);
            }
        }
        let mut resolved_actions: Vec<(config::Action, usize)> = Vec::new();
        // `copy_selection`/`confirm_search` call `ctx.copy_text`/
        // `self.notify` (which itself calls `ctx.input`) — real `egui::
        // Context` methods, not just `self` field writes. Calling either
        // directly from inside the `ctx.input(|i| {...})` closure below
        // would re-enter `Context`'s single internal write lock on the
        // same thread: confirmed by a standalone reproduction to
        // deadlock and (in a debug build) panic after egui's own 10s
        // deadlock-detection timeout, not just a theoretical risk. Every
        // other chrome shortcut here only touches plain `self` fields,
        // which is why this is the only spot that needs deferring —
        // collected into flags during the closure, same as
        // `resolved_actions` above, and acted on once it's closed.
        let mut copy_requested = false;
        let mut confirm_search: Option<SearchDirection> = None;
        let mut open_dialog_requested = false;
        let mut dropped_paths = Vec::new();
        ctx.input(|i| {
            dropped_paths.extend(i.raw.dropped_files.iter().map(|file| file.path().to_path_buf()));
            for event in &i.events {
                // While typing a `/`/`?` query, every other key below
                // (chrome shortcuts, the modal `Dispatcher`) is
                // swallowed — matches Vim's command-line mode, where
                // e.g. Ctrl+K zooming or entering Cursor mode mid-query
                // would make no sense.
                if let Some(direction) = self.search_editing {
                    match event {
                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => {
                            self.search_editing = None;
                            confirm_search = Some(direction);
                        }
                        egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                            self.search_editing = None;
                        }
                        egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                            self.search_draft.pop();
                        }
                        egui::Event::Text(text) if !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt => self.search_draft.push_str(text),
                        _ => {}
                    }
                    continue;
                }
                // `/` starts a forward search, `?` a backward one —
                // checked before the modal dispatcher further down so
                // they don't just dead-end as unbound keys (neither is
                // in `config.rs`'s token vocabulary). Shift is allowed
                // through (`?` is Shift+/ on a US keyboard), matching
                // every other plain-key check in this file.
                if let egui::Event::Text(text) = event {
                    if !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt && (text == "/" || text == "?") {
                        self.search_editing = Some(if text == "/" { SearchDirection::Forward } else { SearchDirection::Backward });
                        self.search_draft.clear();
                        self.dispatcher.reset();
                        continue;
                    }
                }

                // App-chrome shortcuts (tab/zoom/debug toggles) are kept
                // modifier-gated or on keys outside config.rs's token
                // vocabulary, so they never collide with a vim-style
                // single-letter binding — e.g. plain "b" is word_backward
                // in Cursor mode by default, so the debug box toggle lives
                // on Cmd+B instead of plain "b".
                if let egui::Event::Key { key, pressed: true, .. } = event {
                    match key {
                        egui::Key::Num1 => {
                            self.app.jump_to_tab(0);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num2 => {
                            self.app.jump_to_tab(1);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num3 => {
                            self.app.jump_to_tab(2);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num4 => {
                            self.app.jump_to_tab(3);
                            self.dispatcher.reset();
                        }
                        egui::Key::Tab if i.modifiers.shift => {
                            self.app.prev_tab();
                            self.dispatcher.reset();
                        }
                        egui::Key::Tab => {
                            self.app.next_tab();
                            self.dispatcher.reset();
                        }
                        // Matches keys.js's plainCtrl table exactly:
                        // Ctrl+K/J zoom in/out, Ctrl+0 resets to 100%,
                        // Ctrl+U/D half-page scroll. keys.js checks
                        // ctrlKey && !metaKey && !altKey && !shiftKey;
                        // egui doesn't expose a held-modifiers check per
                        // match arm directly, so the `if` guards below do
                        // the equivalent (shift is allowed through, same
                        // as keys.js's plainCtrl which also permits it —
                        // that was a latent inconsistency there too, kept
                        // for fidelity rather than "fixed" here).
                        egui::Key::K if i.modifiers.ctrl => {
                            self.zoom_to(zoom_step(self.zoom, 1), i.pointer.hover_pos());
                        }
                        egui::Key::J if i.modifiers.ctrl => self.zoom_to(zoom_step(self.zoom, -1), i.pointer.hover_pos()),
                        egui::Key::Num0 if i.modifiers.ctrl => self.zoom_to(SCALE, i.pointer.hover_pos()),
                        egui::Key::U if i.modifiers.ctrl => self.pending_scroll = Some(egui::vec2(0.0, HALF_PAGE_SCROLL)),
                        egui::Key::D if i.modifiers.ctrl => self.pending_scroll = Some(egui::vec2(0.0, -HALF_PAGE_SCROLL)),
                        // "i" continues text objects ("y i s") when a sequence is
                        // pending and in cursor/visual modes; otherwise only
                        // Normal mode toggles dark.
                        egui::Key::I if input::dark_mode_shortcut(self.dispatcher.mode(), i.modifiers) && !self.dispatcher.has_pending() => self.dark = !self.dark,
                        egui::Key::B if i.modifiers.command => self.show_boxes = !self.show_boxes,
                        egui::Key::C if i.modifiers.command => copy_requested = true,
                        egui::Key::O if i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt => open_dialog_requested = true,
                        egui::Key::Escape => {
                            // Unconditional mouse-selection clear, on top
                            // of whatever config.rs's "Esc" binding (if
                            // any) resolves to via the dispatcher below.
                            self.cursor_idx = None;
                            self.anchor_idx = None;
                        }
                        _ => {}
                    }
                }

                // Modal key-sequence dispatch (config.rs-driven). Skip
                // while any chrome modifier is held so e.g. Cmd+C's text
                // artifact (if any) can't also feed the dispatcher a "c".
                if i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt {
                    continue;
                }
                if let Some(token) = input::token_for_event(event)
                    && let Some(resolved) = self.dispatcher.feed(&self.cfg, &token)
                {
                    resolved_actions.push(resolved);
                }
            }
        });
        if copy_requested {
            self.copy_selection(&ctx);
        }
        if let Some(direction) = confirm_search {
            self.confirm_search(direction, &ctx);
        }
        // Native file pickers and PDFium both interact with the OS; never
        // run them while holding egui's input lock.
        if open_dialog_requested
            && let Some(paths) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_files()
        {
            self.open_pdf_paths(paths, &ctx);
        }
        self.open_pdf_paths(dropped_paths, &ctx);
        for (action, count) in resolved_actions {
            self.apply_action(action, count, &ctx);
        }
        if self.close_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // shell.css's ".pdf-shell-tabs" has `background: transparent`,
        // but that's transparent onto `body`'s own `--shell-bg` (the
        // canvas color) — not onto whatever sits further behind it. Here
        // the tab bar is painted before `CentralPanel`, straight onto
        // the window's default background, so an actually-transparent
        // fill left a near-black strip behind it that didn't match the
        // canvas color on every other edge. Computed early so both this
        // panel and `CentralPanel` below paint the same color.
        let ap = &self.cfg.appearance;
        let themed = |light: &str, dark: &str| parse_hex_color(if self.dark { dark } else { light }, egui::Color32::MAGENTA);
        let canvas_color = themed(&ap.canvas_light, &ap.canvas_dark);
        let tab_bar_color = themed(&ap.tab_bar_light, &ap.tab_bar_dark);
        let tab_active_color = themed(&ap.tab_active_light, &ap.tab_active_dark);
        let tab_inactive_color = themed(&ap.tab_inactive_light, &ap.tab_inactive_dark);
        let gutter_rel_color = themed(&ap.gutter_rel_light, &ap.gutter_rel_dark);
        let gutter_current_color = themed(&ap.gutter_current_light, &ap.gutter_current_dark);
        let font_size = ap.font_size;

        // Matches shell.css's ".pdf-shell-tabs": a centered, numbered tab
        // row, canvas-colored background, small monospace text, current
        // tab in plain red/bold (no button chrome) — hidden entirely
        // with one document open ("no pane-bar height" for the
        // single-pane case).
        if self.app.tab_count() > 1 {
            egui::Panel::top("tabs")
                // `show_separator_line` is on by default and paints a
                // dim 1px line at the panel's bottom edge regardless of
                // `frame`'s fill.
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(tab_bar_color).inner_margin(egui::Margin::symmetric(6, 3)))
                .show(ui, |ui| {
                    let labels: Vec<(String, bool)> = (0..self.app.tab_count())
                        .map(|index| {
                            let label = self.app.tab_label(index).unwrap_or("?");
                            (format!("{}: {}", index + 1, label), index == self.app.active_index())
                        })
                        .collect();
                    // `ui.horizontal` always claims the panel's full
                    // width (so `with_layout(.., Align::Center)` around
                    // it has nothing to center). Center the row by hand
                    // instead: measure every label at the same font used
                    // to paint it, then pad the left edge by the leftover
                    // space.
                    let font_id = egui::FontId::monospace(font_size);
                    let spacing = ui.spacing().item_spacing.x;
                    let content_width: f32 = labels
                        .iter()
                        .map(|(text, _)| ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.clone(), font_id.clone(), tab_inactive_color).size().x))
                        .sum::<f32>()
                        + spacing * labels.len().saturating_sub(1) as f32;
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - content_width) / 2.0).max(0.0));
                        for (index, (text, is_current)) in labels.into_iter().enumerate() {
                            let rich = egui::RichText::new(text).monospace().size(font_size);
                            let rich = if is_current { rich.color(tab_active_color).strong() } else { rich.color(tab_inactive_color) };
                            if ui.add(egui::Label::new(rich).sense(egui::Sense::click())).clicked() {
                                self.app.jump_to_tab(index);
                                self.dispatcher.reset();
                            }
                        }
                    });
                });
        }

        // Matches style.css's ".pdf-visual-status.is-notice": a small
        // floating box, not a permanent toolbar — shown only briefly
        // after something notable happens (see `notify()`), not for
        // every cursor motion (those update `self.status` for parity/
        // future accessibility use, but aren't painted at all, matching
        // "Keep mode/cursor announcements accessible without an
        // on-screen mode bar.").
        if self.search_editing.is_some() || ctx.input(|i| i.time) < self.notice_until {
            let (bg, fg) = if self.dark {
                (egui::Color32::from_rgb(0x33, 0x33, 0x33), egui::Color32::from_rgb(0xee, 0xee, 0xee))
            } else {
                (egui::Color32::WHITE, egui::Color32::from_rgb(0x33, 0x33, 0x33))
            };
            let prompt = self.search_editing.map(|direction| format!("{}{}▏", if direction == SearchDirection::Forward { "/" } else { "?" }, self.search_draft));
            egui::Area::new(egui::Id::unique("notice"))
                .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(bg)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .shadow(egui::epaint::Shadow { offset: [0, 1], blur: 6, spread: 0, color: egui::Color32::from_black_alpha(50) })
                        .show(ui, |ui| {
                            // Area reuses its previous frame's width. A wrapping
                            // label fits that width instead of growing with the
                            // query, stacking characters vertically. Extend
                            // measures one line and lets the area resize.
                            ui.add(egui::Label::new(egui::RichText::new(prompt.as_deref().unwrap_or(&self.notice)).monospace().size(12.0).color(fg)).wrap_mode(egui::TextWrapMode::Extend));
                        });
                });
            request_repaint(&ctx);
        }

        // zoom is intentionally not part of this key: it's a pure
        // display-space scale (see `viewport::target_render_scale`),
        // never triggers a reload.
        self.switch_to_active_tab(&ctx);
        self.collect_render_results(&ctx);

        let ap = &self.cfg.appearance;
        let pick = |light: &str, dark: &str| parse_hex_color(if self.dark { dark } else { light }, egui::Color32::MAGENTA);
        let selection_color = pick(&ap.selection_light, &ap.selection_dark);
        let cursor_color = pick(&ap.cursor_light, &ap.cursor_dark);
        let search_color = pick(&ap.search_light, &ap.search_dark);
        let search_current_color = pick(&ap.search_current_light, &ap.search_current_dark);
        let yank_color = pick(&ap.yank_highlight_light, &ap.yank_highlight_dark);
        let now = ctx.input(|i| i.time);
        if self.yank_flash.is_some_and(|f| now >= f.until) {
            self.yank_flash = None;
        }
        if self.yank_flash.is_some() {
            request_repaint(&ctx);
        }
        let page_gap = self.cfg.appearance.page_gap as f32;

        let mut zoom_correction_pending = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(canvas_color))
            .show(ui, |ui| {
                // Captured before entering the ScrollArea: inside it,
                // ui.available_width() is effectively unbounded (that's
                // what lets horizontal scrolling work for a page wider
                // than the window), so it can't be used for centering
                // math — this is the actual visible viewport size.
                let viewport_size = ui.available_size();

                self.viewport_rect = ui.clip_rect();
                let mut page_rects = Vec::new();
                let mut corrected_zoom = false;

                egui::ScrollArea::both().id_salt(("pdf-scroll", self.app.active_index())).animated(false).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    if let Some(delta) = self.pending_scroll.take() {
                        ui.scroll_with_delta(delta);
                    }

                    let Some(viewer) = &self.viewer else {
                        ui.label("no document open");
                        return;
                    };

                    let gutter_width = self.cfg.appearance.gutter_width as f32;
                    for page_idx in 0..viewer.pages.len() {
                        let page_view = &viewer.pages[page_idx];
                        // Display size from the page's point size directly
                        // (works whether or not it's rendered yet: for a
                        // rendered page this equals texture_px * display_scale,
                        // since the texture was rasterized at its own
                        // `render_scale`, computed from the live zoom —
                        // see `viewport::target_render_scale`).
                        let size = egui::vec2(page_view.size_pt.width_pt, page_view.size_pt.height_pt) * self.zoom;
                        // Centers the page horizontally when it's narrower
                        // than the viewport; 0 (flush left, scrollable)
                        // once zoomed past the window width.
                        let pad = ((viewport_size.x - size.x) / 2.0).max(0.0);

                        let Some(page) = &page_view.rendered else {
                            // Not rendered yet (progressive_render hasn't
                            // reached it) — reserve its layout space with
                            // a plain placeholder so scrolling/positions
                            // stay correct, and don't do any interaction/
                            // overlay work for it this frame.
                            ui.horizontal(|ui| {
                                ui.add_space(pad);
                                let (placeholder_rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                page_rects.push(placeholder_rect);
                                ui.painter_at(placeholder_rect).rect_filled(placeholder_rect, 0.0, canvas_color.linear_multiply(0.85));
                            });
                            ui.add_space(page_gap);
                            continue;
                        };
                        // This page's own `render_scale` (the scale
                        // `target_render_scale` computed when it was last
                        // rasterized, possibly clamped lower for an
                        // unusually large page — see `RenderedPage`), not
                        // a single scale shared by every page.
                        let display_scale = self.zoom / page.render_scale;
                        // Gutter line numbers (relative to the cursor's
                        // line, absolute on the cursor's own line — same
                        // convention as visual.js's lineNumber()) only on
                        // the page the keyboard cursor is actually on,
                        // only while Cursor/Visual/VisualLine is active.
                        // Simplification versus the original: that app
                        // computes relative numbers *across* pages too;
                        // this only does same-page relative numbering,
                        // to keep the gutter correct without needing a
                        // cross-page line-count pass every frame.
                        let show_gutter = matches!(self.dispatcher.mode(), config::Mode::Cursor | config::Mode::Visual | config::Mode::VisualLine)
                            && self.keyboard_page() == Some(page_idx);

                        // Recompute centering pad including the gutter's
                        // width when shown, so the page+gutter together
                        // stay centered rather than just the page.
                        let pad = ((viewport_size.x - size.x - if show_gutter { gutter_width } else { 0.0 }) / 2.0).max(0.0);

                        let (response, gutter_rect) = ui
                            .horizontal(|ui| {
                                ui.add_space(pad);
                                let gutter_rect = show_gutter.then(|| ui.allocate_exact_size(egui::vec2(gutter_width, size.y), egui::Sense::hover()).0);
                                let texture = if self.dark { &page.dark_texture } else { &page.texture };
                                let response = ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                                (response, gutter_rect)
                            })
                            .inner;
                        let rect = response.rect;
                        page_rects.push(rect);

                        // Sharp viewport tile over the stretched page texture.
                        // Skipped while its theme differs from the current one.
                        if let Some(tile) = self.tiles.get(&(self.app.active_index(), page_idx))
                            && tile.dark == self.dark
                        {
                            let f = self.zoom / tile.scale;
                            let tile_rect = egui::Rect::from_min_size(rect.min + egui::vec2(tile.x as f32, tile.y as f32) * f, egui::vec2(tile.w as f32, tile.h as f32) * f);
                            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                            ui.painter_at(rect).image(tile.texture.id(), tile_rect, uv, egui::Color32::WHITE);
                        }

                        if let Some(gutter_rect) = gutter_rect {
                            let cursor_line = self.page_cursor.filter(|_| self.active_page == Some(page_idx)).map(|c| c.line);
                            let gutter_painter = ui.painter_at(gutter_rect);
                            for (i, line) in page.lines.iter().enumerate() {
                                let Some(lr) = line.rect() else { continue };
                                let (label, color) = match cursor_line {
                                    Some(cl) if cl == i => ((i + 1).to_string(), gutter_current_color),
                                    Some(cl) => ((i as isize - cl as isize).unsigned_abs().to_string(), gutter_rel_color),
                                    None => ((i + 1).to_string(), gutter_rel_color),
                                };
                                let y = gutter_rect.top() + (lr.top + lr.bottom) * 0.5 * display_scale;
                                gutter_painter.text(
                                    egui::pos2(gutter_rect.right() - 4.0, y),
                                    egui::Align2::RIGHT_CENTER,
                                    label,
                                    egui::FontId::monospace(font_size - 1.0),
                                    color,
                                );
                            }
                        }

                        if let Some(pos) = response.hover_pos() {
                            let local = pos - rect.min.to_vec2();
                            if response.drag_started() {
                                self.active_page = Some(page_idx);
                                self.anchor_idx = self.char_at_pos(page_idx, local);
                                self.cursor_idx = self.anchor_idx;
                            } else if response.dragged() && self.active_page == Some(page_idx) {
                                self.cursor_idx = self.char_at_pos(page_idx, local);
                            } else if response.clicked() {
                                self.active_page = Some(page_idx);
                                self.anchor_idx = self.char_at_pos(page_idx, local);
                                self.cursor_idx = self.anchor_idx;
                            }
                        }

                        let painter = ui.painter_at(rect);
                        for (match_idx, m) in self.search_matches.iter().enumerate().filter(|(_, m)| m.page == page_idx) {
                            let current = self.search_current == Some(match_idx);
                            let color = if current { search_current_color } else { search_color };
                            for span in &m.spans {
                                let r = to_egui_rect(*span, self.zoom).translate(rect.min.to_vec2());
                                painter.rect_filled(r, 0.0, color);
                                if current {
                                    painter.rect_stroke(r, 0.0, (1.5, search_current_color), egui::StrokeKind::Outside);
                                }
                            }
                        }
                        if self.show_boxes {
                            for c in &page.chars {
                                let r = to_egui_rect(c.rect, display_scale).translate(rect.min.to_vec2());
                                painter.rect_stroke(
                                    r,
                                    0.0,
                                    (0.5, egui::Color32::from_rgb(0, 150, 255)),
                                    egui::StrokeKind::Outside,
                                );
                            }
                        }

                        if self.active_page == Some(page_idx) {
                            if let Some((a, b)) = self.display_selected_range(page_idx) {
                                // One merged rect per covered line, not
                                // one rect per character — filling each
                                // character's own (sometimes narrow or
                                // degenerate, especially for spaces) rect
                                // individually left visible gaps in the
                                // highlight, most visibly between words.
                                // The cursor's own filled cell is drawn on
                                // top of this afterward, so it still
                                // stands out as a distinct marker within
                                // the bar.
                                for span in selection::selection_span_rects(&page.lines, a, b) {
                                    let r = to_egui_rect(span, display_scale).translate(rect.min.to_vec2());
                                    painter.rect_filled(r, 0.0, selection_color);
                                }
                            }
                            if let Some(idx) = self.cursor_idx
                                && let Some(c) = page.chars.get(idx)
                            {
                                // A thin outline on one character was
                                // easy to miss entirely — a filled
                                // block cursor plus a thicker outline
                                // is much harder not to notice.
                                let r = to_egui_rect(c.rect, display_scale).translate(rect.min.to_vec2());
                                painter.rect_filled(r, 0.0, cursor_color);
                                painter.rect_stroke(r, 0.0, (3.0, cursor_color), egui::StrokeKind::Outside);
                            }
                        }

                        if let Some(flash) = self.yank_flash.filter(|f| f.page == page_idx) {
                            for span in selection::selection_span_rects(&page.lines, flash.start, flash.end) {
                                let r = to_egui_rect(span, display_scale).translate(rect.min.to_vec2());
                                painter.rect_filled(r, 0.0, yank_color);
                            }
                        }

                        ui.add_space(page_gap);
                    }
                    if let Some(anchor) = self.pending_zoom_anchor.take()
                        && let Some(rect) = page_rects.get(anchor.page)
                    {
                        ui.scroll_with_delta(anchor.correction(*rect, self.zoom));
                        corrected_zoom = true;
                    }
                });
                self.page_rects = page_rects;
                // egui applies scroll_with_delta after laying out and painting
                // the content. Do not show that uncorrected pass: run one more
                // pass using the updated scroll offset, like Sioyek's atomic
                // zoom-and-offset update before its next paint.
                if corrected_zoom {
                    zoom_correction_pending = true;
                    ctx.request_discard("zoom anchor scroll correction");
                }
            });
        if !zoom_correction_pending {
            self.progressive_render(&ctx);
            self.update_tiles(&ctx);
        }
    }
}

/// Puts the configured font file, if any, ahead of the built-in
/// monospace font. A missing or unreadable file is reported and ignored.
pub fn install_font(ctx: &egui::Context, font_file: &str) {
    if font_file.is_empty() {
        return;
    }
    let bytes = match std::fs::read(font_file) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("ggpdf: could not read font_file {font_file}: {e}");
            return;
        }
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("user".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
        fonts.families.entry(family).or_default().insert(0, "user".to_owned());
    }
    ctx.set_fonts(fonts);
}
