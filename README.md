# ggPdf

Native macOS PDF viewer with keyboard-driven navigation, Vim-style text selection,
tabs, search, zoom, and dark mode. Built in Rust with egui and PDFium.

## Install

Requires macOS, Rust/Cargo, and a PDFium library for your Mac's architecture at
`pdfium/libpdfium.dylib` (not included in this repository; see
`pdfium/LICENSE.txt` for its license). Run from the repository root:

```sh
./install-macos.sh
```

The installer builds and signs `/Applications/ggPdf.app`. It replaces only a
matching native bundle (`local.jens.ggpdf.native`), not an unrelated app.
It does not change your default PDF handler. Choose ggPdf in Finder’s **Get Info
→ Open with → Change All…** to make double-clicking PDFs open in ggPdf.

Open a PDF through Finder, drag it into the window, use Ctrl+O, or run:

```sh
open -a /Applications/ggPdf.app /path/to/document.pdf
```

For development:

```sh
cargo test
cargo run -- assets/test-pdfs/letter.pdf
```

Finder passes PDFs as macOS document-open events rather than command-line
arguments. Cold launches and an already-running app both open them as tabs.
The bundle ID remains `local.jens.ggpdf.native`.

## License

ggPdf code: MIT (`LICENSE`). PDFium: `pdfium/LICENSE.txt`.
