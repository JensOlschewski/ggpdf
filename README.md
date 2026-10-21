# ggpdf

Heavily vibecoded minimal, keyboard-driven PDF viewer with Vim-style motions and
text selection.

## Install & use

### macOS

```sh
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
./install.sh
```

Open any PDF with "PDF Preview" (Finder's "Open With", or set it as the default
PDF handler). Without installing, run a PDF directly:

```sh
python3 preview.py document.pdf
```

See [Open](#open) for details on what the installer does.

### Windows (PowerShell)

Install Python 3, then in PowerShell run:

```powershell
git clone https://github.com/JensOlschewski/ggpdf
cd ggpdf
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Right-click a PDF and select **Open with → Choose another app → ggpdf PDF Preview**.
The installer registers an option for the current user; it does not change the
default PDF app. Re-run the installer to update an existing registration.
If Windows previously made Python the default for PDFs, choose ggpdf in the
**Open with** dialog and select **Always** to replace that choice.
Keep the cloned folder in place after installation. Without
installing, open a PDF from PowerShell with `py -3 .\preview.py .\document.pdf`
(or `python .\preview.py .\document.pdf` if `py` is unavailable).

## Keys

| Keys            | Action                                                   |
| --------------- | -------------------------------------------------------- |
| gg / G          | Beginning / end                                          |
| j / k           | Scroll down / up 50 px                                   |
| Ctrl+d / Ctrl+u | Scroll down / up 150 px                                  |
| Ctrl+k / Ctrl+j | Zoom in / out                                            |
| Ctrl+0          | 100% zoom                                                |
| i               | Toggle dark/light mode                                   |
| q               | Close the preview window                                 |
| Shift+v         | Enter/exit selection mode (cursor + line numbers)        |
| v               | Toggle selection anchor (start/stop extending selection) |

## Keys in selection mode

Shift+v enters mode with a text cursor and visible line numbers. By default the
cursor just moves (status `COPY`); pressing `v` drops an anchor and starts
extending a selection (status `VISUAL`), pressing `v` again drops the anchor.
Shift+v exits back to the plain PDF view.

Vim motions within this mode: `h`/`l` move by character, `j`/`k` by text line,
`w`/`b`/`e` by word, `0`/`$` to line start/end. Prefix `j` or `k` with a count
(for example `3j`, `10k`) to jump that many lines; this also extends a selection
while anchored, and crosses page boundaries.

## Open

From a clone of this repository, run:

```sh
python3 preview.py document.pdf
```

On macOS, optionally run `./install.sh` to install a file-opening adapter at
`~/Applications/PDF Preview.app`. The installer refuses to overwrite an existing
app. Open the app directly to choose PDFs from a file picker, or register it as
the default PDF handler in macOS settings. Its source is `launcher.applescript`.
PDFs open in the system's default browser. On macOS, if it's Chromium-based (Chrome,
Edge, Brave, Vivaldi, Chromium, Opera, Arc), they open as a standalone app
window without the tab strip or address bar; other browsers (Safari, Firefox)
open a normal window. On Windows, the viewer opens in the default browser.
The viewer is document-only: no PDF toolbar, reserved header space, or page
borders/shadows. Use Cmd+f (macOS) or Ctrl+f (Windows) for search; its panel
appears only while searching.

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
