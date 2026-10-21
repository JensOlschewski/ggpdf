# ggpdf

Minimal, keyboard-driven PDF viewer with Vim-style motions and text selection.
It serves PDF.js locally and opens a standalone Chrome window; no extension is
required. The launcher currently supports macOS and needs Python 3 and Google
Chrome. PDF.js 6.4.299 is bundled in `vendor/` under Apache-2.0.

## Open

From a clone of this repository, run:

```sh
python3 preview.py document.pdf
```

Optionally run `python3 install.py` to install a macOS file-opening adapter at
`~/Applications/PDF Preview.app`. The installer refuses to overwrite an existing
app. Open the app directly to choose PDFs from a file picker, or register it as
the default PDF handler in macOS settings. Its source is `launcher.applescript`.
PDFs open in standalone Chrome app windows without the tab strip or address bar.
The viewer is document-only: no PDF toolbar, reserved header space, or page
borders/shadows. Use Cmd+f for search; its panel appears only while searching.

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

## Keys

| Keys | Action |
| --- | --- |
| j / k | Scroll down / up 50 px |
| Ctrl+d / Ctrl+u | Scroll down / up 150 px |
| gg / G | Beginning / end |
| Ctrl+k / Ctrl+j | Zoom in / out |
| Ctrl+0 | 100% zoom |
| i | Toggle dark mode |
| q | Close the preview window |
| Shift+v | Enter cursor/copy mode and reveal line numbers |

### Visual selection (PDF only)

Press `Shift+v` to enter cursor/copy mode. A left gutter shows line numbers,
with the cursor's absolute page line in red and other lines relative to it.
The cursor is gray until marking with `v`, then purple. Use `h`/`l` to move
by a character, `j`/`k` by a text line, `w`/`b`/`e` to move by words within a
page, and `0`/`$` to reach the
beginning/end of a line. Nothing is selected yet.
Prefix `j` or `k` with a count (for example `3j`, `10k`) to jump that many
text lines; this also extends a selection after `v` and crosses page boundaries.
The initial cursor is on the first visible text line; re-entering resumes the
previous cursor if it is still in view. Only `Shift+v` enters this mode;
lowercase `v` does nothing in normal mode. There is no on-screen mode toolbar,
only line numbers and the cursor/selection. Brief notices report copy results
or errors; mode/cursor announcements remain available to screen readers.

Press `v` at the desired cursor position to **start marking text**. Movements
now extend the selection, including both its starting and current character,
as in Vim. Press `v`, `Esc`, or `Shift+v` to clear the mark and continue moving
the cursor. Another `Esc` or `Shift+v` exits cursor/copy mode.
While marking, `is` selects the sentence under the cursor and `ip` selects its
paragraph; press `y` to copy it.
`j`/`k` can cross page boundaries, loading text as needed. On multi-column
pages, later columns have their own line-number gutter.

Press `y` to copy the selected text to the clipboard and return to normal
scrolling. `Cmd+c` also copies marked text. From unmarked cursor mode,
`Esc` or `Shift+v` exits and hides the line numbers. Zooming or rotating ends
cursor/copy mode to avoid stale text positions. The normal `i` and `q`
shortcuts remain available.

Without a mark, `yis` copies the sentence under the cursor and `yip` copies
its paragraph. Both commands blink the copied text and exit copy mode;
marked `y` does the same. Sentence boundaries follow browser text segmentation.
Paragraph boundaries are inferred from line spacing and indentation within a
page, so unusual PDF layouts may need a manual `v` selection instead.

Lines follow PDF.js's extracted reading order and line breaks, with geometry
as a fallback. Complex layouts can differ from editor lines. Scans without a
text layer need OCR. Line numbers are display-only and never copied/printed.

Zoom and theme are remembered in Chrome. PDF.js supplies search, selectable
text, and printing. Its toolbar stays hidden for a minimal view. Shortcuts leave
editable fields and open dialogs alone. The viewer disables PDF JavaScript.

## Server

The launcher starts an on-demand Python standard-library server on IPv6
loopback, `http://[::1]:18765`. Requests require an unguessable URL prefix; only
explicitly opened PDFs and viewer assets are exposed. There is no directory
browsing or arbitrary-file endpoint. The server stays running for tab reloads
and subsequent documents. After a reboot, opening a PDF starts it again.

Runtime state and logs: `~/Library/Caches/pdf-preview/`. To stop the server:

```sh
kill "$(cat ~/Library/Caches/pdf-preview/server.pid)"
```

Website PDF links retain Chrome's normal handling. This application opens local
PDFs. It reads the original files, so reloading reflects saved changes; it does
not auto-refresh documents while they are being edited.

## License

ggpdf's own code is MIT-licensed (see `LICENSE`). The bundled PDF.js retains
its Apache-2.0 license in `vendor/LICENSE`.
