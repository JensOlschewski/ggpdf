# ggpdf

Heavily vibecoded minimal, keyboard-driven PDF viewer with Vim-style motions and
text selection.

## Install & use

### Requirements
 - Python 3

### macOS

```sh
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
./install.sh
```

This installs `~/Applications/ggpdf.app`. Run it again to update the launcher;
it also replaces an older `PDF Preview.app` installed by ggpdf. The existing
default PDF handler is not changed.

### Windows (PowerShell)


```powershell
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Run the installer again to update ggpdf's **Open with** registration (including
the path to this checkout). It does not change the default PDF app; keep the
checkout in place after installing.

### Without installing

On macOS, Linux, or Windows, run a PDF directly with Python 3:

```sh
python3 preview.py document.pdf
```

## Keys

| Keys                | Action                  |
| ------------------- | ----------------------- |
| `gg` / `G`          | Go to top / bottom      |
| `j` / `k`           | Scroll down / up 50 px  |
| `Ctrl+d` / `Ctrl+u` | Scroll down / up 150 px |
| `Ctrl+k` / `Ctrl+j` | Zoom in / out           |
| `Ctrl+0`            | Reset zoom to 100%      |
| `i`                 | Toggle dark/light mode  |
| `q`                 | Close preview           |
| `Shift+v`           | Enter text cursor mode  |

### Text cursor and selection

`Shift+v` enters `CURSOR` mode with a text cursor and line numbers. Navigate
without selecting, then press `v` to anchor the cursor and enter `VISUAL` mode.
Motions extend the selection from that anchor. Press `v` or `Esc` in `VISUAL`
to clear the selection and return to `CURSOR`; `Esc` in `CURSOR` returns to the
plain PDF view. `Shift+v` exits directly to the PDF view from either text mode.
This entry/exit shortcut is ggpdf-specific; in Vim, `V` starts linewise Visual
mode.

| Keys            | Action                                      |
| --------------- | ------------------------------------------- |
| `h` / `l`       | Move left / right one character             |
| `j` / `k`       | Move down / up one text line                |
| `w` / `b` / `e` | Next word / previous word / word end        |
| `0` / `$`       | Go to line start / end                      |
| `v`             | Start or clear a Visual selection           |
| `Esc`           | Clear selection, or exit from `CURSOR`      |
| `Shift+v`       | Exit directly from either text mode         |
| `y`             | Copy Visual selection and exit              |
| `is` / `ip`   | Select sentence / paragraph in `VISUAL`     |
| `yis` / `yip` | Copy sentence / paragraph in `CURSOR`, exit |

Prefix `j` / `k` with a count (e.g. `3j`, `10k`). Movement crosses pages and
extends the selection when anchored. `Cmd+c` (or `Ctrl+c`) also copies an
active Visual selection. `q` closes the preview even in a text mode. Zooming
or rotating the PDF exits text mode.

## Appearance

Edit this repository's `theme.css` to adjust the canvas and dark-page filter:

```css
:root {
  --preview-canvas-light: #ededed;
  --preview-canvas-dark: #282828;
  --preview-dark-page-filter: invert(0.9) hue-rotate(180deg);
}
```

Reload the viewer after editing. Viewer-specific layout lives in `style.css`.

## Server

The launcher starts an on-demand Python standard-library server on IPv6
loopback, `http://[::1]:18765`. Requests require an unguessable URL prefix; only
explicitly opened PDFs and viewer assets are exposed. There is no directory
browsing or arbitrary-file endpoint. The server stays running for tab reloads
and subsequent documents. After a reboot, opening a PDF starts it again.

Runtime state and logs: `~/Library/Caches/pdf-preview/` on macOS and
`%LOCALAPPDATA%\ggpdf\` on Windows. To stop the server on macOS:

```sh
kill "$(cat ~/Library/Caches/pdf-preview/server.pid)"
```

Website PDF links retain Chrome's normal handling. This application opens local
PDFs. It reads the original files, so reloading reflects saved changes; it does
not auto-refresh documents while they are being edited.

## License

ggpdf's own code is MIT-licensed (see `LICENSE`). The bundled PDF.js retains its
Apache-2.0 license in `vendor/LICENSE`.
