# Native ggpdf (macOS)

Build and install the Rust/PDFium viewer as a macOS app:

```sh
cd native
./install-macos.sh
```

Requires Rust/Cargo and `pdfium/libpdfium.dylib` for your Mac's architecture.
The installer builds a release binary and creates `/Applications/ggpdf Native.app`
with a stable bundle ID (`local.jens.ggpdf.native`), PDFium, an icon, and assets.
It moves a previously installed `~/Applications/ggpdf Native.app` out of the
way after checking its bundle ID. It does **not** replace the old PDF.js
`~/Applications/ggpdf.app`, change your default PDF handler, or edit AeroSpace.
The installer needs write access to `/Applications` and never runs Cargo as root.
Open the new app directly:

```sh
open -a "/Applications/ggpdf Native.app"
open -a "/Applications/ggpdf Native.app" --args "/path/to/book.pdf"
```

To route new windows to your PDF workspace, put this rule before your
catch-all `on-window-detected` rule in `~/.config/aerospace/aerospace.toml`:

```toml
[[on-window-detected]]
if.app-id = 'local.jens.ggpdf.native'
run = ['move-node-to-workspace D', 'layout tiling', 'layout h_tiles']
```

Then run `aerospace reload-config` and reopen the app. AeroSpace still decides
the final tile; a bundle ID makes matching reliable but cannot guarantee that
the initial resize is invisible.

The window title is `ggPDF`. eframe saves the last normal window size and
position to `~/Library/Application Support/local.jens.ggpdf.native/app.ron`
and restores them on the next launch. The first launch uses 1200×800. AeroSpace
may still reposition or resize the window when it applies its workspace rule;
saved geometry is not a substitute for tiling. If the app closes while its
workspace is hidden, AeroSpace may have parked its window offscreen. eframe
discards that offscreen saved position on the next launch, but keeps its size.
