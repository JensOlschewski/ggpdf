# ggPdf

Heavily vibecoded minimal keyboard-driven PDF viewer with Vim-style motions and
text selection, based on PDFium engine.

<p align="center">
  <img src="icon.svg" alt="ggPdf icon" width="96">
</p>

| Cursormode                                                  | Darkmode                                                  |
| ----------------------------------------------------------- | --------------------------------------------------------- |
| <img src="docs/assets/ggpdf-linenumber.png" width="450em"/> | <img src="docs/assets/ggpdf-darkmode.png" width="450em"/> |

| Sentence selection in vim-like visual mode                          | Paragraph selection with vip                                         |
| ------------------------------------------------------------------- | -------------------------------------------------------------------- |
| <img src="docs/assets/ggpdf-sentence-selection.png" width="450em"/> | <img src="docs/assets/ggpdf-paragraph-selection.png" width="450em"/> |

## Install & use

### Requirements

- macOS or Windows, and Rust/Cargo
- PDFium for your OS and architecture (not included): `pdfium/libpdfium.dylib`
  on macOS or `pdfium/pdfium.dll` on Windows

### macOS

```sh
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
./install-macos.sh
```

This builds and installs `/Applications/ggPdf.app`. Run it again to update the
app. The existing default PDF handler is not changed. To make ggPdf the default,
choose it in Finder → Get Info → Open with → Change All.

Open PDFs by double-clicking in Finder, using Open with, dragging into the app,
or pressing Ctrl+O. Finder opens files as tabs on cold launch and in a running
app. From a terminal:

```sh
open -a /Applications/ggPdf.app /path/to/document.pdf
```

### Windows

From PowerShell in the cloned repository:

```powershell
.\install-windows.ps1
```

This builds the app and installs it for the current user at
`%LOCALAPPDATA%\Programs\ggPdf`. Run it again to update. It adds a Start menu
shortcut and an **Open with** entry for PDFs; it does not change your default
PDF handler. You can open PDFs with **Open with → ggPdf**, drag them into the
window, or press Ctrl+O. If PowerShell blocks local scripts, run
`powershell -ExecutionPolicy Bypass -File .\install-windows.ps1` from this
repository (or `pwsh -ExecutionPolicy Bypass -File .\install-windows.ps1`).

### Without installing

Run the native app directly from this checkout:

```sh
cargo run -- assets/test-pdfs/letter.pdf
```

## Keys

| Keys                | Action                       |
| ------------------- | ---------------------------- |
| `gg` / `G`          | Go to top / bottom           |
| `j` / `k`           | Scroll down / up 50 px       |
| `Ctrl+d` / `Ctrl+u` | Scroll down / up 150 px      |
| `Ctrl+k` / `Ctrl+j` | Zoom in / out                |
| `Ctrl+0`            | Reset zoom to 100%           |
| `i`                 | Toggle dark/light mode       |
| `q`                 | Close ggPdf                  |
| `c`                 | Enter/Leave text cursor mode |
| `v` / `Shift+v`     | `VISUAL` / `VISUAL LINE`     |

### Text cursor and selection

| Keys            | Action                                              |
| --------------- | --------------------------------------------------- |
| `h` / `l`       | Move left / right one character                     |
| `j` / `k`       | Move down / up one text line                        |
| `w` / `b` / `e` | Next word / previous word / word end                |
| `W` / `B` / `E` | WORD motions (punctuation stays in a word)          |
| `0` / `^` / `$` | Go to line start / first nonblank / end             |
| `Esc`           | Clear selection, or exit from `CURSOR`              |
| `y`             | Copy selection and exit (linewise includes newline) |
| `is` / `ip`     | Select sentence / paragraph in `VISUAL`             |
| `yis` / `yip`   | Copy sentence / paragraph in `CURSOR`, exit         |

Prefix `h/j/k/l` and word motions with a count (e.g. `3j`, `2w`, `4l`).

## Appearance

Edit `~/.config/ggpdf/config.toml` to adjust the canvas colors and page gap:

```toml
[appearance]
canvas_light = "#ededed"
canvas_dark = "#282828"
page_gap = 12
```

The native app renders its own UI; the old `theme.css` and `style.css` no longer
apply.

## Local PDFs

Website PDF links retain Chrome's normal handling. This app opens local PDFs
without a Python server or browser profile.

## License

ggPdf's own code is MIT-licensed (see `LICENSE`). PDFium's license is in
`pdfium/LICENSE.txt`.
