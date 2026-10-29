# ggPdf

Heavily vibecoded minimal keyboard-driven PDF viewer with Vim-style motions and
text selection, based on PDFium engine.

<p align="center">
  <img src="assets/icons/icon.svg" alt="ggPdf icon" width="96">
</p>

| Cursormode                                                  | Darkmode                                                  |
| ----------------------------------------------------------- | --------------------------------------------------------- |
| <img src="docs/assets/ggpdf-linenumber.png" width="450em"/> | <img src="docs/assets/ggpdf-darkmode.png" width="450em"/> |

| Sentence selection in vim-like visual mode                          | Paragraph selection with vip                                         |
| ------------------------------------------------------------------- | -------------------------------------------------------------------- |
| <img src="docs/assets/ggpdf-sentence-selection.png" width="450em"/> | <img src="docs/assets/ggpdf-paragraph-selection.png" width="450em"/> |

## Install & use

### Prebuilt installers

Download an installer from the
[releases page](https://github.com/JensOlschewski/ggpdf/releases). Installers
include PDFium; Rust/Cargo is only needed to build from source.

### Build from source

Requires Rust/Cargo and PDFium for your OS and architecture. Place PDFium at
`pdfium/libpdfium.dylib` (macOS) or `pdfium/pdfium.dll` (Windows).

```sh
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
```

Build and install (rerun to update):

- **macOS:** `./install.sh` installs to `/Applications/ggPdf.app`.
- **Windows (PowerShell):** `.\install.ps1` installs to
  `%LOCALAPPDATA%\Programs\ggPdf` and adds Start menu and **Open with** entries.

If PowerShell blocks the script, use
`powershell -ExecutionPolicy Bypass -File .\install.ps1`. Neither script
changes your default PDF viewer.

Or run without installing:

```sh
cargo run -- assets/test-pdfs/letter.pdf
```

### Open PDFs

Use **Open with → ggPdf**, drag a PDF into the window, or press **Ctrl+O**. On
macOS, you can also run `open -a ggPdf /path/to/document.pdf`.

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

Outside text modes, `yis` / `yip` copy the sentence / paragraph under the mouse,
or at the center of the window if the mouse is not over a page.

Prefix `h/j/k/l` and word motions with a count (e.g. `3j`, `2w`, `4l`).

## Configuration

ggPdf reads the first existing file of:

1. `$XDG_CONFIG_HOME/ggpdf/config.toml` (or `~/.config/ggpdf/config.toml`)
2. macOS: `~/Library/Application Support/ggpdf/config.toml`
3. Windows: `%APPDATA%\ggpdf\config.toml`

[`config.example.toml`](config.example.toml) lists every option and key binding
with its default. Copy it and edit. An invalid file is reported on stderr and
the defaults are used. Omitted options keep their defaults. `ggpdf --print-config`
prints the resolved config, a good starting point for your own file.

### Appearance

`[appearance]` holds colors (`#rrggbb` or `#rrggbbaa`), sizes, and fonts. Colors
for the gutter and tab bar come as `_light` / `_dark` pairs. `font_file` points
to a `.ttf`/`.otf` file used before the built-in font; empty means built-in.
Unknown options are an error.

### Key bindings

Bindings are `"<key sequence>" = "<action>"` in four tables:

| Table             | Applies to                                  |
| ----------------- | ------------------------------------------- |
| `[keys.document]` | Normal mode (scrolling, file-level keys)    |
| `[keys.motion]`   | Shared by cursor and selection modes        |
| `[keys.cursor]`   | Cursor mode: `motion` plus this table       |
| `[keys.selection]`| `VISUAL` and `VISUAL LINE`: `motion` plus this table |

A mode's own table wins over `motion`. A table you leave out keeps its defaults;
a table you include replaces that table's defaults, so copy it whole. A key
sequence may not be a prefix of another in the same mode (`y` and `y i s` cannot
both exist), and one binding must be `close` or `cancel`.

Old names still load: `[keys.normal]` is `document`, `[keys.visual]` and
`[keys.visual_line]` are `selection`, and `cursor_mode`, `visual_mode`,
`visual_line_mode`, `normal_mode`, `copy_sentence`, `copy_paragraph` are aliases
of `*_toggle`, `document_mode`, `yank_sentence`, `yank_paragraph`. An unknown
action name lists all valid ones.

The native app renders its own UI; the old `theme.css` and `style.css` no longer
apply.

## Local PDFs

Website PDF links retain Chrome's normal handling. This app opens local PDFs
without a Python server or browser profile.

## License

ggPdf's own code is MIT-licensed (see `LICENSE`). PDFium's license is in
`pdfium/LICENSE.txt`.
