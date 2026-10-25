# Native ggpdf (macOS)

Build and install the Rust/PDFium viewer as a macOS app:

```sh
cd native
./install-macos.sh
```

Requires Rust/Cargo and `pdfium/libpdfium.dylib` for your Mac's architecture.
The installer builds a release binary and creates `/Applications/ggPdf.app`
with a stable bundle ID (`local.jens.ggpdf.native`), PDFium, an icon, and
assets. It also finds any earlier install of this same app under an older
name or location (`/Applications/ggpdf Native.app`,
`~/Applications/ggpdf Native.app`, `~/Applications/ggPdf.app`) and removes
it as part of the rename, after checking its bundle ID matches — it never
touches an app it didn't install itself. It does **not** replace the old
PDF.js `~/Applications/ggpdf.app`, change your default PDF handler, or edit
AeroSpace. The installer needs write access to `/Applications` and never
runs Cargo as root. Open the new app directly:

```sh
open -a "/Applications/ggPdf.app"
open -a "/Applications/ggPdf.app" --args "/path/to/book.pdf"
```

Finder double-click and “Open with” deliver PDFs as macOS document-open events,
not command-line arguments. The app handles these on cold launch and in an
existing window, opening each file as a tab. The macOS bridge adds those
callbacks to winit's existing application delegate; replacing that delegate
breaks winit's event loop.

To route new windows to your PDF workspace, put this rule before your
catch-all `on-window-detected` rule in `~/.config/aerospace/aerospace.toml`:

```toml
[[on-window-detected]]
if.app-id = 'local.jens.ggpdf.native'
run = ['move-node-to-workspace D', 'layout tiling', 'layout h_tiles']
```

Then run `aerospace reload-config` and reopen the app. AeroSpace still decides
the final tile; a bundle ID makes matching reliable but cannot guarantee that
the initial resize is invisible. Note that this bundle ID also matches the
file-open panel's own window (it's the same process) — a tiling rule for this
ID will also tile that panel instead of leaving it floating, unlike apps
whose panels AeroSpace's own dialog heuristic recognizes and floats
automatically (that heuristic mainly keys off the window having no
fullscreen/maximize button, which a plain `NSOpenPanel` usually lacks; if
AeroSpace still tiles it for you, add a second, narrower rule matching this
app's window title to float only the panel).

The window title is `ggPdf`. eframe saves the last normal window size and
position to `~/Library/Application Support/local.jens.ggpdf.native/app.ron`
and restores them on the next launch. The first launch uses 1200×800. AeroSpace
may still reposition or resize the window when it applies its workspace rule;
saved geometry is not a substitute for tiling. If the app closes while its
workspace is hidden, AeroSpace may have parked its window offscreen. eframe
discards that offscreen saved position on the next launch, but keeps its size.

## Native panel language

The bundle declares `CFBundleLocalizations = [en, de]` (no `.lproj` folders
needed) so macOS's built-in Open/Save panel chrome follows the system
language instead of staying in English — a bundle that only declares (or
defaults to) `en` keeps AppKit's own panels in English regardless of the
system language; see
[storytold/pdfcraft#2668](https://github.com/storytold/pdfcraft/issues/2668)
for the same root cause in another app. ggpdf's own UI text is unaffected
either way — it draws everything itself via egui, none of it through AppKit.

## Keeping the file-open panel floating

If AeroSpace's own dialog heuristic doesn't float the Open panel on its own
(it's the same app/bundle ID as the main window, so a tiling rule for ggpdf
tiles the panel too), add a rule matching the panel's own title before the
main ggpdf rule in `~/.config/aerospace/aerospace.toml`:

```toml
[[on-window-detected]]
if.app-id = 'local.jens.ggpdf.native'
if.window-title-regex-substring = '^(Open|Öffnen)$'
run = 'layout floating'
```
