# ggpdf native rewrite plan (Rust + PDFium)

Status: scaffolding only. `cargo init` has been run in this `native/` directory
(default `Cargo.toml` + `src/main.rs` "Hello, world!"). No real code yet.
Branch: `rust-pdfium-rewrite`.

## Goal

Replace the Chrome/PDF.js/Python-server stack with a native, keyboard-driven
PDF viewer in Rust, using PDFium for rendering and text extraction, running on
macOS, Windows, and Linux. One user-editable config file defines all key
mappings and appearance. Keep the current ggpdf (Python + PDF.js) usable and
unchanged until the rewrite reaches parity.

## Why this approach

- Chrome is only the *window* today; PDF.js is the renderer and supplies the
  positioned text layer that `visual.js` depends on for Vim motions and
  selection. A native rewrite must replace both the window and the text model.
- PDFium (BSD-style license, the engine behind Chrome's own PDF viewer) fits
  an MIT-licensed project. MuPDF is AGPL/commercial and was rejected for that
  reason.
- `eframe`/`egui` gives one Rust UI across macOS, Windows, and Linux with
  direct control over custom drawing (cursor, selection highlights, gutter),
  which this app needs. PDFium stays a bundled C++ dependency behind
  `pdfium-render`; it does not become Rust.

## Biggest risk — validate first

The hard problem is not rendering a page; it's rebuilding the text model that
`visual.js` relies on: reading-order lines, grapheme-level character bounds,
multi-column detection, sentence/paragraph text objects, and stable cursor
positions for Vim-style motions.

Before restructuring the full project, build a disposable spike that:

1. Opens a PDF and renders one page via PDFium at correct DPI.
2. Extracts Unicode text **and per-character bounds** from PDFium.
3. Draws a cursor and a selection highlight over the rendered page.
4. Copies selected text correctly from a two-column, a rotated, and a
   ligature-heavy PDF.
5. Runs on macOS, Windows, and Linux.

**Gate:** if PDFium's extracted text order/geometry cannot reliably support
the motions in `visual.js` (word/WORD, sentence/paragraph objects, line
numbering across columns), revise the text model approach before building
the rest of the app. Do not proceed past this gate silently.

## Project structure

Single crate to start, split into modules by responsibility. Do not split
into multiple crates or add a generic "renderer backend" abstraction until
there's a second real backend to justify it.

```text
native/
  Cargo.toml
  src/
    main.rs             CLI args, startup, error reporting
    app.rs              windows, open documents, active tab/pane
    config.rs           load, validate, and report config errors
    input.rs            key sequences, counts, modes, action dispatch
    document.rs         PDFium document/page lifetime management
    text.rs             reading-order lines, graphemes, geometry model
    selection.rs         cursor, motions, text objects (is/ip), yank
    render.rs           page raster jobs, cache, cancellation, generations
    viewer.rs           scrolling, zoom, overlays, status line/UI
    platform.rs         open-with handoff, clipboard, single-instance IPC
  assets/
    test-pdfs/          fixtures: columns, rotated, ligatures, scanned, etc.
  PLAN.md               this file
```

Keep PDFium handles behind the `document.rs`/`render.rs` boundary. The UI
layer (`viewer.rs`, `selection.rs`) consumes owned page images and text
geometry, never raw PDFium pointers.

## Config file — one file, keys + appearance

TOML. `${XDG_CONFIG_HOME:-~/.config}/ggpdf/config.toml` is checked first
on every platform (decided over implementation's objection that it's
non-native on macOS/Windows — user wants one dotfiles-friendly location
available everywhere); if that file doesn't exist, falls back to the
platform's standard user-config directory:

- macOS: `~/Library/Application Support/ggpdf/config.toml`
- Windows: `%APPDATA%\ggpdf\config.toml`
- Linux: same as the `~/.config` path above (no separate fallback)

Example shape:

```toml
[appearance]
canvas_light = "#ededed"
canvas_dark = "#282828"
page_gap = 12
gutter_width = 48
selection = "#4285f459"
cursor = "#787878a6"

[keys.normal]
"j" = "scroll_down"
"k" = "scroll_up"
"gg" = "document_start"
"G" = "document_end"
"c" = "cursor_mode"
"v" = "visual_mode"
"q" = "close"

[keys.cursor]
"h" = "character_left"
"l" = "character_right"
"w" = "word_forward"
"y i s" = "copy_sentence"
```

Requirements:

- Ship built-in defaults; the app must run with no config file present.
- Parse key sequences into named actions. Reject unknown action names,
  duplicate sequences, and ambiguous prefixes at load time with a message
  that names the config file path and the offending key.
- Emergency close/cancel (e.g. `q`, `Esc`) must stay reachable even if the
  rest of the config fails to parse.
- Document modifier-key names and any platform differences explicitly in
  the shipped example config.
- Dark-mode page rendering needs its own small prototype and a decision
  recorded here once made: the current CSS filter inverts whole pages,
  including embedded images, and a native equivalent may need to choose
  differently (e.g. invert only background, or match CSS behavior exactly).
  Do not assume parity with the CSS filter without a side-by-side check.

## Port order (dependency order)

1. **Document viewer shell** — CLI file args, OS "Open with" handoff,
   opening multiple PDFs, tab/pane switching, close, scroll, zoom, rotation,
   light/dark mode, search.
2. **Text model** — PDFium characters + bounds -> visual lines -> Unicode
   graphemes -> stable cursor positions. Handle whitespace-driven line
   joins, ligatures, multi-column pages, rotated pages, and pages with no
   extractable text (OCR stays out of scope, same as today).
3. **Modal input** — normal / cursor / visual / visual-line modes; counts;
   `gg`/`G`; `h j k l`; `w b e`; `W B E`; `0 ^ $`; `is`/`ip`; yank;
   leader-key-based pane switching (mirrors current `keys.js` + `shell.js`).
4. **Selection UI** — line-number gutter, cursor/selection highlight
   rendering, clipboard text (respecting linewise trailing-newline
   behavior), status/error messages.
5. **Lifecycle** — route new "open this PDF" requests to an existing window
   where supported, without a persistent background server; use a local,
   per-user, access-controlled IPC endpoint if needed, otherwise start a
   new instance. No unauthenticated network-reachable endpoint.

Rendering/performance rules:

- Rasterize only visible pages plus a small prefetch margin.
- Cache rendered pages keyed by document, page, scale, rotation, and theme.
- Tag render jobs with a generation id so a stale zoom/rotation/closed-doc
  result can never overwrite current state.
- Keep PDFium work off the UI thread; set sane memory limits for large PDFs.

## Testing and release gates

- Port the behavioral cases in `visual.test.js` (word/WORD motions, counted
  motions, sentence/paragraph text objects, yank, visual-mode transitions)
  into Rust unit tests against the new text/selection model.
- Add tests for config parsing (defaults, overrides, invalid keys, duplicate
  sequences) and for Unicode/grapheme handling in `text.rs`.
- Fixture set in `assets/test-pdfs/`: multi-column, rotated pages, mixed
  fonts, ligatures, non-Latin scripts, scanned (non-text) pages, malformed
  files, and at least one large document.
- Compare rendering and copied text against the current PDF.js-based ggpdf
  on macOS, Windows, and Linux. Check mouse-wheel scrolling, high-DPI
  displays, clipboard interop, keyboard focus, and "Open with" handoff.
- Build/package PDFium per target architecture; pin a specific PDFium +
  `pdfium-render` version pair; publish required third-party license
  notices; document the upgrade process.
- Sign/notarize macOS builds; sign Windows builds before broad
  distribution; test Linux on both X11 and Wayland.
- Installers must not silently change the OS default PDF handler (same
  constraint as the current `install.sh`/`install.ps1`).
- Do not ship/replace the current app until the new one passes the parity
  checklist above. Remove the old Python/PDF.js implementation only as a
  separate, later cleanup change.

## Current state / next step

Done: branch created, `cargo init` run in `native/`.

Validation spike built (`native/src/main.rs`, eframe/egui + `pdfium-render`
0.9.4). It is throwaway code, not the start of the real module structure.

- PDFium binary: downloaded `pdfium-mac-x64` from bblanchon/pdfium-binaries
  into `native/pdfium/` (gitignored — per-platform binary, not committed).
  `main.rs` loads it via `PDFIUM_LIB_DIR` (defaults to `./pdfium`); same
  mechanism will need a Windows/Linux binary dropped into that dir to test
  those platforms, not yet done.
- Fixtures added under `native/assets/test-pdfs/`: `two-column.pdf` (copied
  from `vendor/web/compressed.tracemonkey-pldi-09.pdf`), and generated
  `rotated.pdf` (page with `/Rotate 90`) and `ligatures.pdf` (fi/fl/ffi
  ligature words). No scanned/non-text fixture yet.
- App renders a page via `render_with_config`, extracts per-character
  Unicode + `loose_bounds()` via `page.text().chars()`, and overlays boxes;
  click/drag sets a char-index selection, highlighted and copyable
  (`cmd+c`, via `egui::Context::copy_text`). Keys: `1`/`2`/`3` switch
  fixture, `←`/`→` page, `b` toggle boxes, `r` toggle the rotation-mapping
  fix below.
- Rotation finding, now confirmed: `FPDFText_GetCharBox` (via
  `loose_bounds()`) returns coordinates in the page's *unrotated* space,
  while `page.width()`/`height()` and the rendered bitmap already reflect
  the intrinsic `/Rotate`. `main.rs` maps char rects through the `/Rotate`
  transform by hand (`rotate_rect_to_display`) to align them with the
  bitmap.
- Headless verification path added: `cargo run -- --dump` (no display
  needed) renders each fixture with boxes burned into the bitmap and saves
  PNGs to `native/debug_out/` (gitignored), both with the rotation fix
  (`*_fixed.png`) and without (`*_naive.png`), via a new `dump_fixtures()`/
  `draw_rect_outline()` in `main.rs`. Used because this sandbox has no
  screen-recording permission and the eframe window can't be screenshotted
  directly.
- **Gate result: PASS**, confirmed by eyes-on review of the dumped PNGs:
  - `two-column_fixed.png` — boxes track every glyph correctly through
    title, authors, two-column abstract/body, and footnote, in reading
    order (column 1 top-to-bottom, then column 2).
  - `ligatures_fixed.png` — ligature glyphs (fi, fl, ff, ffi, ffl) are each
    split into separate per-letter boxes, confirming PDFium decomposes
    ligatures to individual Unicode characters with correct bounds rather
    than returning one glyph for multiple letters.
  - `rotated_fixed.png` — boxes land tightly on every rotated glyph,
    reading top-to-bottom matching the rotated text.
  - `rotated_naive.png` (rotation fix disabled) — boxes are entirely
    off-canvas/invisible, proving the by-hand `/Rotate` transform is
    necessary and that it's correct (not just coincidentally close).
- `no-text.pdf` fixture added (vector shapes, no text operators) to stand
  in for a scanned/non-text page. `--dump` reports 0 chars and does not
  panic — the empty-text-layer path is safe. (This is a synthetic stand-in,
  not an actual scanned/rasterized page; good enough to prove the
  zero-chars code path, not a substitute for a real OCR-less scan in the
  final test fixture set from the Testing section.)
- Not yet done: live interactive click/drag selection and `cmd+c` copy
  (code exists, only exercised via the headless dump path so far, not
  through the real eframe window — same no-display constraint), and any
  platform other than macOS x86_64.

Interactive click/drag selection and `cmd+c` copy confirmed working on a
real display, including on `rotated.pdf` with the rotation fix toggled.

**Gate fully closed.** All five validation points from "Biggest risk —
validate first" are confirmed: render at correct DPI, per-character
Unicode + bounds extraction, cursor/selection overlay, correct copy on
two-column/rotated/ligature-heavy PDFs. (Windows/Linux not yet tried —
see below.)

Starting the real module structure (Project Structure section) instead of
extending the spike further. `native/src/main.rs` stays as disposable
spike code otherwise — do not keep growing it beyond this wiring; split
into the modules listed there as each one is actually needed.

**`config.rs` — done.** `native/src/config.rs`: `Action` enum (closed set,
extend as `input.rs` grows real motions — an unrecognized action name in
the config is a load error, not a silent no-op), `Appearance` +
`Config { appearance, keys: HashMap<Mode, HashMap<String, Action>> }`,
`Mode` (normal/cursor/visual/visual_line). Validation: unknown mode names,
unknown action names, and ambiguous key-sequence prefixes (e.g. `"g"` and
`"g g"` both bound in the same mode) are load errors. A config that omits
a mode entirely keeps that mode's built-in bindings rather than going
empty. `load_or_default()` never fails: missing file, parse error, or
validation error (including "no key anywhere closes/cancels") all fall
back to `Config::default()` and print the config file path + offending
key to stderr — checked manually with both a missing config and a config
with an unknown action name; both ran fine and the broken one printed the
expected error before falling back.

Config path resolution deviates from PLAN.md's original per-platform-only
table, on user request: `~/.config/ggpdf/config.toml` (or
`$XDG_CONFIG_HOME/ggpdf/config.toml`) is checked *first* on every
platform, including macOS and Windows, falling back to the platform-native
path (`~/Library/Application Support/ggpdf/config.toml` on macOS,
`%APPDATA%\ggpdf\config.toml` on Windows) only if no `~/.config` file
exists. `config_path_candidates()` returns the ordered list;
`load_or_default()` picks the first one that exists on disk and prints
which path it loaded from. Manually verified on macOS: a file at
`~/.config/ggpdf/config.toml` is picked up and its override takes effect,
path printed to stderr. Windows/Linux candidate paths are `cfg`-gated but
unexercised (Linux's XDG path is also its only candidate, so no fallback
behavior to test there). 8 unit tests in
`config.rs` (defaults, overrides, unknown mode/action, ambiguous prefix,
non-conflicting shared-prefix sequences, partial-override mode fallback)
— `cargo test`, all pass. `main()` now calls `config::load_or_default()`
on startup and prints a one-line summary; nothing else consumes the
config yet (no `input.rs` to dispatch actions through).

**`document.rs` — done.** `native/src/document.rs`: `PdfLibrary` (owns the
loaded `Pdfium` instance, `load(lib_dir)`), `Document<'lib>` (`open()`,
`page()`, `page_count()`), `Page<'a>` (`size()`, `rotation_degrees()`,
`render(pixels_per_point) -> image::RgbaImage`, `raw_chars() ->
Vec<RawChar>`). This is now the only module that imports
`pdfium_render` — `main.rs` no longer references any PDFium type
directly, only `document::{PdfLibrary, DocumentError}` plus the plain
`RawChar`/`PageSize` values. `main.rs`'s `load()` and `dump_fixtures()`
were refactored to call through this boundary instead of touching PDFium
ad hoc; the rotation-mapping math (`rotate_rect_to_display`) stays in
`main.rs` for now since it's egui-coordinate-space-specific and belongs
in `render.rs`/`text.rs` per the next item below, not in `document.rs`
(which only hands back raw, unrotated point-space bounds, matching
PDFium's own `FPDFText_GetCharBox` convention).

Regression-checked: `cargo run -- --dump` output is byte-identical
(md5-compared) before and after the refactor across all 8 PNGs, so the
already-verified rendering/rotation/ligature/no-text behavior is
unchanged. `cargo test` (8 tests) and `cargo build` (no warnings) both
pass.

**`app.rs` — done (open/tab-switching slice only).** `native/src/app.rs`:
`App<'lib>` owns `Vec<OpenDocument>` (path + `document::Document`) and an
active-tab index. API: `open(path)`, `active()`, `tab_count()`,
`active_index()`, `tab_label()`, `next_tab()`/`prev_tab()` (wrap around),
`jump_to_tab(index)` (no-op if out of range), `close_active()` (not yet
wired to a key — `#[allow(dead_code)]`, no close-tab binding exists in
the spike UI). 7 unit tests covering open/active-index tracking,
wraparound, out-of-range jump, close-tab fallback-to-previous, close on
empty app, a failed open leaving state unchanged, and tab labeling —
using the real fixture PDFs through `PdfLibrary`/`document.rs`, not
mocks. `cargo test`: 15/15 pass (8 config + 7 app).

`main.rs` wired onto `App` instead of the flat `FIXTURES[fixture_index]`
indexing: `SpikeApp::new(library, initial_paths)` opens CLI file args as
tabs (one per arg), falling back to the bundled `FIXTURES` when none are
given — so `--dump` and plain `cargo run` behave exactly as before.
`1`-`4` jump to tab N, `Tab`/`Shift+Tab` cycle. Manually verified:
`cargo run -- assets/test-pdfs/ligatures.pdf assets/test-pdfs/does-not-exist.pdf assets/test-pdfs/rotated.pdf`
opens the two valid files as tabs, prints `could not open ...:
IoError(...)` for the bad path, and the window stays up (no crash, no
aborted startup from one bad path in the list) — see `App::open`'s
error-and-skip handling in `main.rs`. `--dump` output re-verified
md5-identical across all 8 PNGs after this refactor too — no behavior
regression.

Deferred, explicitly out of scope for this increment (not needed to
exercise CLI-arg multi-document opening, and would pull in OS-specific
work disproportionate to where the plan is): "Open with" OS handoff and
single-instance IPC (Port order item 5, Lifecycle) — the CLI-args path
this step built is what a real "Open with" handoff would ultimately call
into, so this isn't wasted work, just not the OS registration/IPC part of
it. Likewise zoom/rotation-toggle/dark-mode/search UI stay out of
`app.rs` — those are `render.rs`/`viewer.rs` concerns.

**`text.rs` — done (rotation-mapping slice only).** `native/src/text.rs`:
plain `Rect`/`PositionedChar` (no egui, no PDFium types) and
`position_chars(raw_chars, size, rotation_degrees, pixels_per_point) ->
Vec<PositionedChar>`, carrying over the already-gate-validated
`/Rotate` transform math verbatim from the spike. 4 unit tests
(unrotated Y-flip, scale, the 90° top-left→top-right case matching the
gate's visual check, empty input) — `cargo test`: 19/19 pass (8 config +
7 app + 4 text). `main.rs`'s `load()` now calls `text::position_chars`
for the real (rotation-fix) path and converts the plain `Rect` to
`egui::Rect` only at the UI boundary; the old `rotate_rect_to_display` in
`main.rs` is deleted. The "naive" (no rotation fix) comparison mapping
stays inline in `main.rs` — it's diagnostic-only scaffolding for the gate
check, not part of the real text model, so it doesn't belong in
`text.rs`. `--dump` output re-verified md5-identical across all 8 PNGs
after this move — no behavior regression.

`render.rs` (page raster jobs, cache, cancellation, generations per the
Project structure section) is explicitly **not** created yet: there's no
scrolling/prefetch/cache need until `viewer.rs` exists, and an empty
cache module with nothing to cache would be speculative scaffolding.
`document.rs`'s `Page::render()` covers today's single-page-at-a-time
raster need; `render.rs` gets built when `viewer.rs` actually needs job
queuing/cancellation.

---

**Scope check, since the ask is now "look and behave very similar to
ggpdf with Chrome":** everything above is still foundational plumbing
(config, PDFium boundary, multi-doc/tab state, rotation-aware text
geometry) — none of it is visible yet as a PDF viewer. What "look and
behave similar" actually requires, roughly in the order it'll get built:

1. **`viewer.rs`** — continuous multi-page scrolling layout (today: one
   static page, no scroll), zoom, the page-gap/gutter-width/canvas-color
   appearance from `config.rs` actually applied (`config::Appearance`'s
   defaults were cross-checked against `theme.css`/`style.css` this turn
   and already match the current app's colors — `#ededed`/`#282828`
   canvas, `rgb(66 133 244 / 35%)`/`rgb(120 120 120 / 65%)` selection/
   cursor — so the target values are already right, just not wired to
   any rendering yet), status/error messages, light/dark mode.
2. **`input.rs`** — the actual modal key-sequence dispatcher consuming
   `config.rs`'s `Action`s (today: `config.rs` parses and validates
   bindings but nothing executes them — `main.rs`'s key handling is
   still hardcoded, not config-driven). This is where `visual.js`'s
   normal/cursor/visual/visual-line modes, counts, and leader-key pane
   switching get ported.
3. **`selection.rs`** — word/WORD motions, `is`/`ip` text objects,
   yank, counted motions: the behavioral core of `visual.test.js`, which
   PLAN.md's Testing section says to port as Rust unit tests. This is
   the single largest remaining chunk of porting work.
4. Gutter line numbers, clipboard linewise-trailing-newline behavior,
   "Open with" OS handoff, packaging/signing — later Port-order items.

This is the bulk of the rewrite described in PLAN.md's Port order
section, not a small polish pass — realistically more sessions of work,
not one more step.

**`viewer.rs` — done (continuous scroll + zoom + appearance slice).**
`native/src/viewer.rs`: `Viewer::load(doc, pixels_per_point, ctx)` renders
every page of a document up front (eager, no cache/generations/
cancellation — see the `render.rs` note above for why that's still
deferred) into `PageView { texture, chars }`, `chars` positioned via the
already-built `text::position_chars`. `main.rs`'s `SpikeApp` rewritten
around it: single-page/page_index navigation is gone, replaced by a
`ScrollArea` stacking every page of the active tab vertically with
`config.appearance.page_gap` between them; `+`/`-` zoom (re-renders all
pages at the new scale); `d` toggles the canvas fill between
`config.appearance.canvas_light`/`canvas_dark` (parsed via a small
`parse_hex_color` hex parser in `main.rs`, falling back to a gray on a
malformed config string rather than panicking — `config.rs` doesn't
validate color *syntax*, only structure, so this has to tolerate bad
input); selection/cursor colors also now come from
`config.appearance.selection`/`cursor` instead of hardcoded constants.
Click/drag selection and copy are still single-page (the page under the
cursor when the drag starts) — full multi-page text objects/motions are
explicitly `selection.rs`'s job (item 3 above), not built here. The
"naive" (no-rotation-fix) comparison path was dropped from the
interactive viewer entirely (it already served its purpose — the gate is
closed — and `--dump` still exercises it standalone for the regression
PNGs); `r` stopped being a key binding.

`dump_fixtures` decoupled from `SpikeApp`/`viewer.rs` entirely — it never
needed egui or a window, so it now calls `document.rs`/`text.rs` directly
instead of constructing a throwaway `SpikeApp` just to get char data. Its
PNG output is confirmed md5-identical across all 8 files versus every
prior checkpoint, through this entire `viewer.rs` change — no regression
in the already-verified rendering/rotation/ligature/no-text behavior.

New tests: 2 headless `viewer.rs` smoke tests in `main.rs` (multi-page
load — proves `two-column.pdf`'s real page count all load+texture
without panicking — and a zoom-scales-pixel-size check), using a bare
`egui::Context::default()`, which works fine for texture bookkeeping
without any live window/GPU. `cargo test`: 21/21 pass. While adding these
I hit and fixed a real pre-existing test-isolation bug: `Pdfium::bind_to_library`
can only succeed once per process (global `OnceCell`), but `cargo test`
runs `#[test]`s on parallel threads of the same process, so any two tests
each calling `PdfLibrary::load` directly would race and intermittently
fail with `PdfiumLibraryBindingsAlreadyInitialized` — `app.rs`'s existing
7 tests had this latent risk too, just lower collision odds with fewer
concurrent callers. Fixed with `document::test_library()`, a
`std::sync::OnceLock`-backed shared `&'static PdfLibrary` for the whole
test binary; `app.rs` and `main.rs`'s test modules both now go through
it — every future test module needing a `PdfLibrary` must too. Verified
by running `cargo test` 5x in a row with no failures (previously flaky
expansion would show within 1-2 runs).

Manually verified live (not just `--dump`): `cargo run -- <4 files>`
opens all four fixtures as tabs and stays up; `cargo run` (no args, falls
back to bundled fixtures) also stays up. Neither produced any error/panic
output. Visual confirmation of the scroll/zoom/dark-mode/selection
*behavior* itself (not just "doesn't crash") still needs a real display —
same constraint as before, not newly introduced.

Known gap surfaced, not fixed yet: zoom is currently clamped to
`SCALE * 0.25`..`SCALE * 8` with no awareness of the real backend's
texture-size limit (`ctx.input(|i| i.max_texture_side)` or similar isn't
consulted) — a large/landscape page at high zoom could in principle
exceed some GPU's max texture side and panic, the same failure mode hit
while writing the zoom-scaling test above (there, against the bare test
harness's conservative 2048px default, not a real GPU limit). Not fixed
now because it's exactly the kind of per-page-size/scale bookkeeping
`render.rs` is supposed to own once it exists (job cache + "sane memory
limits for large PDFs" per the Rendering/performance rules) — clamping
blindly here would be the wrong layer for it.

**`input.rs` — done (dispatcher mechanism; most motions still stubs).**
`native/src/input.rs`: `Dispatcher` tracks current `Mode` and a pending
token buffer; `feed(cfg, token)` joins pending tokens space-separated and
looks them up in `cfg.keys[mode]` — exact match resolves and clears the
buffer (also applying the mode switch if the action is
`CursorMode`/`VisualMode`/`VisualLineMode`/`NormalMode`); a prefix match
keeps waiting; a dead end (matches nothing, prefixes nothing) silently
resets. This can never be ambiguous because `config.rs` already rejects,
at load time, any binding that's a strict prefix of another in the same
mode — `feed`'s dead-end/prefix logic leans directly on that guarantee
rather than re-validating it. `token_for_event` reads `egui::Event::Text`
(not `Event::Key`) for printable tokens so shift-adjusted symbols (`^`,
`$`, `G`, `V`) and keyboard layout resolve correctly without hand-tracking
shift state, plus a special case mapping `Key::Escape` to the `"Esc"`
token (Escape doesn't produce a Text event). 7 unit tests (single-token
resolve, multi-token wait-then-resolve, dead-end doesn't poison the next
sequence, mode switch changes which bindings apply, Esc returns to
Normal, a full 3-token sequence, `reset()` behavior) — `cargo test`:
28/28 pass, 3x in a row with no flakes.

Wired into `main.rs`: every unmodified `Text`/`Escape` event feeds the
dispatcher (events under Cmd/Ctrl/Alt are skipped, so a chrome shortcut's
own text artifact, if any, can't also feed a stray token). `apply_action`
gives `ScrollDown`/`ScrollUp`/`DocumentStart`/`DocumentEnd`/`Close` a real
effect (scroll via `ui.scroll_with_delta`, consumed once per frame inside
the `ScrollArea` closure since that's the only place it can be called
from; `Close` sends `ViewportCommand::Close`); `Cancel` clears the mouse
selection; the four mode-switch actions are no-ops beyond what
`Dispatcher::feed` already did. Every other `Action` (the word/WORD
motions, line motions, `is`/`ip` text objects, yank) resolves correctly
through the dispatcher but has no keyboard-cursor model to act on yet —
`apply_action`'s fallback arm sets the status line to `"<Action>: not
implemented yet (selection.rs)"` rather than silently no-opping, so it's
visible exactly which bindings are live versus still pending
`selection.rs`. The toolbar now shows the current mode (`-- Normal --` /
`-- Cursor --` / etc.) alongside the load status.

App-chrome shortcuts (tab switch, zoom, the debug box/dark toggles) were
deliberately kept off plain letters and moved to modifier combos
(`Cmd+B`/`Cmd+D`, `Cmd+C` already was) specifically because plain letters
collide with real vim bindings — e.g. the debug box-toggle used to be
plain `b`, which is `word_backward` in Cursor mode by default; left as
`b` it would have silently double-fired (toggle boxes *and* feed the
dispatcher) the moment Cursor mode existed. Tab-switching keeps using
`Tab`/`Shift+Tab`/`1`-`4` since those aren't config token vocabulary.
Switching tabs now also calls `dispatcher.reset()` so a half-typed
sequence from one document doesn't leak into the next.

Known unverified detail, flagged rather than silently assumed: the sign
of `SCROLL_STEP`/`SCROLL_MAX` (which direction `j` actually scrolls) was
picked from reading `ui.scroll_with_delta`'s docs, not confirmed against
a live display — flip the constant in `main.rs` if `j` scrolls up instead
of down.

Manually re-verified after this change: `--dump` output still
md5-identical across all 8 PNGs (dispatcher/input changes don't touch
the dump path at all — confirms the boundary held), and `cargo run` (no
args) stays up with no panics or error output.

**`selection.rs` — done (single-page motion/text-object core, ported and
test-verified against the JS oracle).** `native/src/selection.rs`:
`group_into_lines` (the same vertical-overlap/leftward-jump heuristic as
`visual.js`'s `collect()`, relying on PDFium already yielding reading
order — confirmed on the gate's two-column fixture), `move_cursor` for
`h`/`l`/`j`/`k`/`w`/`b`/`e`/`W`/`B`/`E`/`0`/`^`/`$` (the `positions`-array
word/WORD algorithm is a line-by-line direct port of `visual.js`'s
`move()`), and `text_object_paragraph`/`text_object_sentence` (direct
ports of `textObject("p"/"s")`). 8 unit tests **transcribed directly from
`visual.test.js`'s own data-driven fixtures** (not reinvented) — sentence
spanning lines, paragraph line-gap detection, word motion crossing
lines, counted word vs. WORD motion punctuation handling, counted
horizontal clamp + `^`, counted line motion with sticky-column
preservation, plus 2 new `group_into_lines` edge cases. All 8 passed on
the first run with no port bugs found — strong signal the translation is
faithful. `cargo test`: 36/36 pass (8 config + 7 app + 4 text + 7 input +
8 selection + 2 viewer smoke), 3x stable.

At this checkpoint, `split_sentences` used a `.`/`!`/`?`-plus-whitespace
heuristic instead of UAX #29; the later sentence-segmentation step below
replaced it. At this checkpoint, `document.rs` also
doesn't synthesize a space character between text runs separated by a
rendering gap the way `visual.js`'s `collect()` does, so a PDF whose
content stream omits explicit space glyphs could misjoin words — not
fixed now (would need a `document.rs`/`text.rs` change, not
`selection.rs`).

Wired into `main.rs`'s `apply_action`, replacing every motion/text-object
"not implemented yet" stub: `CharacterLeft`/`Right`,
`Word*`/`BigWord*`, `Line*`, `LineDown`/`LineUp` (new — see below),
`CopySentence`/`CopyParagraph` (select *and* copy, matching `yis`/`yip`,
not just select), and `Yank` (copies the existing mouse/keyboard
selection) are all real now. A new `page_cursor: Option<selection::Cursor>`
+ `sticky_column` track the keyboard cursor separately from the existing
mouse-drag `cursor_idx`/`anchor_idx`, synced together after every motion
via a `flat_index` helper (valid because `group_into_lines` never
reorders characters) so both input paths paint the same on-screen
highlight. `viewer::PageView` gained a `lines: Vec<selection::Line>`
field, computed once per page load via `group_into_lines` rather than
every frame.

Bug caught by the compiler while wiring this up, not by inspection:
`selection::Motion::Up`/`Down` had no `config.rs` binding reaching them —
`visual.js`'s Cursor mode moves the text cursor line-wise with `j`/`k`,
distinct from Normal mode's document-scroll `j`/`k`, but `config.rs`'s
default Cursor-mode bindings never included `j`/`k` at all. Added
`Action::LineDown`/`LineUp` and the corresponding default Cursor-mode
`j`/`k` bindings (24 → 26 total default bindings) to close that gap —
another case (like the earlier `page_count`/`close_active` dead-code
warnings) where "unused code" was the compiler catching a real missing
behavior, not noise to silence.

Not done in this pass, explicitly deferred: `j`/`k` crossing a page
boundary (single-page only, per `selection.rs`'s own scope note), full
`v`/`V` visual-mode mark-as-you-move (today `CopySentence`/`CopyParagraph`
set the mouse-style anchor/cursor range directly; there's no persistent
"marking" toggle state yet), and numeric count-prefix dispatch (`1`-`9`
still aren't bound to anything in `input.rs`/`config.rs`, so every wired
motion currently always runs with `count = 1` — the `count` parameter is
threaded through `selection.rs`'s API already, so wiring real counts
later is a `main.rs`/`input.rs` change, not a `selection.rs` one).

Manually re-verified after this change: `--dump` output still
md5-identical across all 8 PNGs, and `cargo run` stays up with no panics.
Interactive keyboard-motion *behavior* in the live window (does `w`
actually land where expected, does the selection highlight track) still
needs a real display to eyeball — same constraint as every prior step;
the 8 ported unit tests are what stand in for that confirmation here,
consistent with the Testing section's instruction to verify the model
with tests rather than the live window first.

**Bug found by the user's first live check of this step:** `cargo run`
(no args) opened `no-text.pdf` (just a rectangle/ellipse, no extractable
text) as the active tab, not `two-column.pdf` — made Cursor mode's
motions look broken (nothing to move over) when the real cause was the
wrong tab being active. Root cause: `app::App::open()` always makes the
newly-opened tab active (correct and tested for opening one file at a
time, e.g. a future "Open with" handoff — see `app.rs`'s
`open_makes_new_tab_active` test), but `SpikeApp::new()`'s startup loop
opens all 4 bundled fixtures in sequence, so the *last* one ends up
active. Fixed in `main.rs` with `doc_app.jump_to_tab(0)` after the
startup-open loop. `cargo test` (36/36) and `--dump` (md5-identical)
re-confirmed after the fix.

Also still open, lower priority than continuing the real structure: a
genuine scanned/rasterized non-text fixture (current `no-text.pdf` is a
synthetic stand-in, not a real scan), Windows/Linux PDFium binaries +
smoke test and config-path check (spike + config only run on macOS x86_64
so far), and an example config file documenting modifier-key names and
platform differences (required by the Config file section, not yet
written).

## Second live-testing round — 5 issues found, 4 resolved

The user ran the live window again (first real interactive check of
`selection.rs`/`input.rs`) and found 5 problems. In order:

1. **Zoom keys didn't match `keys.js`.** Our zoom was unmodified `+`/`-`,
   invented rather than checked against the original app. The real
   keymap (`keys.js`'s `plainCtrl` table, lines ~140-148): `Ctrl+K` zoom
   in, `Ctrl+J` zoom out, `Ctrl+0` reset to 100%, `Ctrl+U`/`Ctrl+D`
   half-page scroll (±150pt). All five now implemented in `main.rs`
   matching those exact keys. `keys.js` computes zoom via a fixed
   `ZOOM_LEVELS` table (0.5× to 3× in defined steps); ours still uses a
   continuous ×1.2/÷1.2 multiplier rather than that exact table — same
   keys, not byte-identical zoom increments. Not fixed: matching the
   exact level table would need porting `ZOOM_LEVELS` too, deferred as
   low-value (the *keys* matching was what broke the user's muscle
   memory, not the exact step sizes).
2. **Scroll was inverted.** Confirmed the sign flagged as unverified
   back when `input.rs` was built — `j`/`k` were backwards. Flipped
   `ScrollDown`/`ScrollUp`/`DocumentStart`/`DocumentEnd`'s
   `pending_scroll` signs in `main.rs`.
3. **Visual mode selected nothing.** `apply_action`'s `VisualMode`/
   `VisualLineMode` arms only changed `Dispatcher`'s mode, never anchored
   a selection — entering `v`/`V` moved the cursor around with nothing
   highlighted. Fixed: entering either now sets `anchor_idx` to the
   current keyboard cursor's flat index (same mechanism `CopySentence`/
   `CopyParagraph` already used), so subsequent motions extend a visible
   highlight. `V` (VisualLineMode) still behaves exactly like `v` for now
   — whole-line extension isn't implemented, a known gap, not just
   silently broken.
4. **Dark mode doesn't invert the page.** Confirmed as the already-
   documented gap from the `viewer.rs` step: `Cmd+D` (now `i`, see below)
   only recolors the canvas background; the rendered page bitmap itself
   stays whatever PDFium rendered (usually white). The old app's
   `style.css` used a CSS `filter: invert` on top of PDF.js's canvas.
   PLAN.md's Config file section already calls out that page-content
   dark-mode needs "its own small prototype... do not assume parity with
   the CSS filter without a side-by-side check" — still true, still not
   attempted. Also moved the toggle itself from the invented `Cmd+D` to
   plain `i`, matching `keys.js` exactly (its `handleKey` lets `i` and
   `q` fall through even while cursor/visual mode is active; bare `i` is
   unbound in every default `config.rs` mode too, so feeding it to the
   dispatcher as well is harmless — it just dead-ends).
5. **"Not very smooth at all."** Not fixed — diagnosed, not solved.
   Most likely causes, not yet isolated: (a) this is a `cargo build`/
   `cargo run` **debug** binary; PDFium decode + egui painting in debug
   mode can be noticeably slower than `--release` — worth trying
   `cargo run --release` as a first check. (b) `Viewer::load` (see the
   `viewer.rs` entry above) eagerly re-renders and re-uploads *every*
   page's texture synchronously on the UI thread on every zoom change —
   for `two-column.pdf`'s 15 pages, a `Ctrl+K`/`Ctrl+J` press would
   stall for all 15 re-renders before the frame can show anything. This
   is exactly the job `render.rs` (job cache, only re-render what's
   visible, cancellation) was deferred for — see the `render.rs` note
   earlier in this doc. Not attempting that now; flagging it as the
   likely real fix rather than a quick patch.

All fixes re-verified: `cargo test` 36/36, `--dump` output md5-identical
across all 8 PNGs. Items 1-4 are code changes not independently
re-confirmed live by the user yet (this sandbox still has no display) —
worth another pass of the live-window checklist.

## Third live-testing round — confirmed fixes 1-3 live, found 3 more

User rebuilt `--release` and re-tested: scroll confirmed smooth now
(debug build was indeed the item 5 culprit for scroll), but zoom still
laggy, cursor mode showed no visible feedback at all, visual mode still
showed no highlighting, `$`/`0` seemed not to work, and asked to confirm
numeric counts (`3k`) are deferred (they are — no new work).

**Dispatcher-level check first, before touching anything:** added
`input::tests::line_start_and_line_end_tokens_resolve_in_cursor_mode`,
feeding `"0"`/`"$"`/`"^"` straight through `Dispatcher::feed` — all three
resolve correctly. **No dispatcher bug.** This reframed the "cursor mode
does nothing" / "`$`/`0` don't work" reports as a feedback/visibility
problem, not a logic bug, and pointed at the real fixes below instead of
chasing a nonexistent one.

1. **No textual feedback on any motion.** `apply_motion`/
   `apply_text_object`/entering Cursor or Visual mode all updated
   internal state but never touched `self.status` — the only observable
   change was a tiny on-screen highlight, easy to miss entirely if it
   landed off-screen (cursor always starts at page/line/char 0,0,0
   regardless of current scroll position) or was just visually subtle.
   Added `update_cursor_status()` (mirrors `visual.js`'s `message()`:
   `"page P/N line L/M char C [glyph]"`), called from every motion, both
   text objects, and on entering Cursor/Visual mode — so pressing `c`
   alone now immediately shows something changed, independent of whether
   the pixel-level box is visible.
2. **Cursor highlight too subtle to notice.** Was a single-character 2px
   *outline* — changed to a filled block (vim-style block cursor) plus a
   3px outline, both using `config.appearance.cursor`.
3. **Zoom laggy even in `--release`.** Root cause confirmed: `load_viewer`
   re-ran `Viewer::load` — re-rendering *and* re-uploading a fresh GPU
   texture for *every page* of the document — on every `Ctrl+K`/`Ctrl+J`
   keypress (15 pages for `two-column.pdf`). This is the `render.rs`
   gap flagged two rounds ago, now actually fixed rather than deferred
   further, because it's what the user is most actively feeling: **zoom
   is now a pure display-space scale, decoupled from rendering.**
   `RENDER_SCALE` (`= SCALE * MAX_ZOOM_FACTOR` = 6.0, i.e. 432dpi) is a
   fixed resolution pages render at once; `self.zoom` (`MIN_ZOOM_FACTOR`
   0.5 to `MAX_ZOOM_FACTOR` 3.0 × `SCALE`, matching `keys.js`'s
   `ZOOM_LEVELS` bounds) only changes the *displayed* size
   (`texture_size * (zoom / RENDER_SCALE)`) — `Ctrl+K`/`Ctrl+J`/`Ctrl+0`
   no longer touch `loaded_key` at all, so they trigger zero PDFium work.
   The top of the zoom range renders pixel-for-pixel crisp (texture and
   display resolution match exactly); every level below it is a cheap
   downscale. Char overlay rects (boxes, selection, cursor) and mouse
   hit-testing (`char_at_pos`) are stored in `RENDER_SCALE` space but
   drawn/hit-tested in display space — `to_egui_rect` now takes an
   explicit `scale` parameter and every call site (overlay drawing,
   `char_at_pos`'s inverse) was updated together, specifically to avoid
   a desync bug (rects computed at one resolution, drawn as if they were
   in another) that this kind of render/display split invites if done
   carelessly.

Not revisited this round: whole-line `V` extension (still behaves like
`v`), numeric count prefixes (`3k` — confirmed with the user as
already-deferred, not a new ask), real GPU texture-size limits at
`RENDER_SCALE` for unusually large pages (6.0 px/pt × a huge page could
still exceed some GPU's max texture side — Letter/A4 at 3672×4752px is
nowhere near typical 8192+ limits, so low risk for ordinary documents).

Re-verified: `cargo test` 37/37 (added the dispatcher regression test),
`--dump` still md5-identical, both debug and `--release` builds start
clean with no panics. Fixes 1-3 (feedback text, cursor visibility, zoom
decoupling) are new code not yet confirmed live by the user — next
live-window pass should check those specifically, plus re-confirm visual
mode (item 3 from the prior round, fixed then, should now also show the
new status-line feedback alongside the highlight).

## Fourth live-testing round — scroll/zoom confirmed smooth; visual mode still broken, two more root causes found

User confirmed scroll and zoom are now smooth and cursor mode's
visibility fix landed — but visual mode still showed no indicator, and
visual-*line* mode wasn't even reachable. Two separate, compounding
root causes, both in the actual default keymap this time (not a
dispatcher bug, same as the `$`/`0` false alarm two rounds ago, but this
time genuinely broken):

1. **Visual mode had no motion bindings at all.** `config.rs`'s
   `Mode::Visual`/`Mode::VisualLine` defaults only ever bound `Esc` and
   `y` — every single motion key (`h`/`j`/`k`/`l`/`w`/`b`/`e`/`0`/`^`/`$`)
   dead-ended the instant marking started, so the mark could never be
   extended past the single character it started on. Fixed: `visual`/
   `visual_line` are now built as `cursor.clone()` plus their own
   `v`/`V` overrides (see below), not two bare maps — they inherit every
   Cursor-mode motion. Also added `W`/`B`/`E` (BigWord motions) to
   Cursor mode's defaults while here — they were missing entirely,
   another gap from the same "only wired what a hand-written test
   exercised" pattern.
2. **Visual mode was only reachable from Normal mode.** `v`/`V` were
   bound in `normal` but never in `cursor` — matching `visual.js`, where
   marking starts from wherever the (single, conflated) active state
   already is, not from a separate state the user must return to first.
   Fixed: added `v`/`V` to Cursor mode's defaults too. Also implemented
   the `v`<->`V` toggle matrix properly instead of just aliasing both to
   the same thing: in Visual mode, `v` now returns to Cursor mode
   (deselects) and `V` switches to VisualLine; in VisualLine, `V`
   deselects and `v` switches to Visual. `main.rs`'s `apply_action` for
   `CursorMode` now always clears `anchor_idx` (needed since this action
   is what the `v`/`V` toggle-off bindings resolve to — previously only
   fired from `Esc`/direct cursor-mode entry, where `anchor_idx` was
   already `None` so the gap was invisible); the `VisualMode`/
   `VisualLineMode` arm now only (re-)anchors a selection when
   `anchor_idx` is `None` (a fresh entry from Cursor mode) — otherwise
   (switching Visual<->VisualLine while already marking) it leaves the
   existing mark untouched instead of collapsing it back to zero width.

A third, purely visual bug compounded both of the above even once a mark
did exist: the cursor's **filled** block was drawn after the selection
tint, completely covering it on the one character where they overlapped
— so a selection that hadn't been extended past its starting character
(exactly what issue 1 trapped every selection at) looked pixel-identical
to plain, unmarked cursor mode. Fixed in `main.rs`'s render loop: the
selection-fill loop now skips `cursor_idx`'s own cell (the cursor marker
already represents that cell), so every *other* selected character still
gets the clear blue tint, visible as soon as the mark extends past one
character.

2 new regression tests in `config.rs` pin both root causes directly
(`visual_modes_inherit_cursor_motions`, `cursor_mode_can_enter_visual_modes`)
rather than relying on hitting them again by luck — this class of bug
(a mode's keymap silently missing bindings the hand-rolled unit tests
never happened to exercise) has now shown up three times across these
live-testing rounds, so each one gets pinned down as it's found rather
than just fixed and left to regress silently.

Re-verified: `cargo test` 39/39, `--dump` still md5-identical, both
debug and `--release` builds start clean. Not yet confirmed live by the
user — this is the round that most needs another live pass, since
visual mode has now failed to work twice in a row from the user's
perspective even though each round's diagnosis was a different genuine
bug.

## Fifth live-testing round — visual mode confirmed working; one cosmetic issue

User confirmed visual mode now works (both prior root causes actually
fixed it). One remaining complaint: the selection highlight filled
whitespace characters between words with a colored block, reading as a
stray gap/box rather than a continuous selection. Fixed in `main.rs`'s
selection-fill loop: skip any character where `ch.is_whitespace()` (same
loop that already skips the cursor's own cell) — every other selected
character still gets the tint, whitespace between words just shows the
plain canvas through. Re-verified: `cargo test` 39/39, `--dump`
md5-identical, debug/release both start clean.

## Sixth live-testing round — same complaint, previous fix was backwards

User reported the gaps were still there after the whitespace-skip fix
above — that fix had the diagnosis backwards. Skipping whitespace
*guarantees* a gap at every space; it doesn't prevent one. The real
cause was never "whitespace shouldn't be highlighted" — it's that
filling the selection **per character** leaves visible gaps wherever an
individual glyph's reported box is narrow or doesn't quite touch its
neighbor, which happens most visibly at space characters (PDFium's
space-glyph bounds are often narrow/degenerate) but isn't really a
whitespace-specific problem at all.

Real fix: `selection::selection_span_rects(lines, start_flat, end_flat)`
(new, in `selection.rs` — pure data, no egui, consistent with that
module's existing boundary) walks the covered lines and returns **one
merged bounding rect per line** (min-left/top, max-right/bottom across
every character the selection covers on that line, whitespace included)
instead of one rect per character. `main.rs`'s selection-fill loop now
draws those merged spans — continuous by construction, independent of
any individual glyph's box quirks. The cursor's own filled cell is still
drawn afterward, on top, so it remains visible as a distinct marker
within the continuous bar (no need for the old "skip the cursor's cell"
workaround either — that was a point-fix for the same underlying
per-character-gaps issue, and merging the spans supersedes it directly,
so it was removed rather than layered under the new fix).

2 new `selection.rs` unit tests: one directly reproduces the reported
case (selecting "one two" across the space resolves to a single
rect spanning the full width, not two with a gap), the other confirms
multi-line selections still emit one rect per covered line rather than
over-merging into one giant rect. Re-verified: `cargo test` 41/41,
`--dump` md5-identical, debug/release both start clean.

Lesson for next time a highlight/rendering complaint comes in: check
whether the fix addresses the stated symptom's *mechanism*, not just the
literal words used to describe it — "don't want this" after "space...
between the words" read as "don't highlight whitespace," but the
mechanism (per-glyph box gaps) pointed the other way, and a render
double-check (or just asking) before applying would have caught the
inversion before shipping a backwards fix.

## Seventh round — parity pass against the old app (counts, V, is/ip, tabs, gutter) plus a real performance fix

User asked for a comprehensive pass toward "look and behave similar to
the old app," explicitly naming the missing top tab bar, `is`/`ip` not
working, and launch/runtime speed, plus everything already queued
(counts, `V` linewise, cross-page `j`/`k`, gutter numbers). Checked
`keys.js`/`shell.js`/`shell.css` again directly rather than guessing at
remembered behavior, since two recent rounds (zoom keys, visual-mode
keymap) already turned out to need that. All items below are **code
changes not yet confirmed live** — most of this turn's work, so a live
pass is the clear next step before continuing further.

- **Top tab bar.** `shell.js` renders a `"N: name"` tab row, current one
  highlighted, hidden entirely with one pane open (`shell.css`: "no
  pane-bar height" for that case). Added the same to `main.rs`: a
  `Panel::top("tabs")` row above the toolbar, shown only when
  `tab_count() > 1`, clickable to switch (bonus over the original, which
  only switched via keys/pane messages).
- **`is`/`ip` (select without yank).** `visual.js`'s `handleKey` only
  recognizes bare `is`/`ip` while `this.marking` is true — a mode our
  config.rs's `Visual`/`VisualLine` already model distinctly from
  `Cursor`. New `Action::SelectSentence`/`SelectParagraph`, bound to
  `"i s"`/`"i p"` only in `Visual`/`VisualLine` (not `Cursor`, matching
  the marking-gate) — same text-object functions as `CopySentence`/
  `CopyParagraph`, just without the clipboard copy.
- **Numeric count prefixes (`3j`, `12w`, ...).** `input::Dispatcher`
  gained a `pending_count` accumulator: `1`-`9` always start/continue a
  count when no sequence token is pending yet; a bare `0` is the
  `line_start` motion (matching `keys.js`'s `/^[1-9]$/.test(key) ||
  (this.count && key === "0")`), `0` after a nonzero digit continues the
  count. `Dispatcher::feed` now returns `(Action, usize)`; every
  `apply_motion`/scroll call in `main.rs` threads the count through —
  this was the explicitly-deferred item from several rounds ago, now
  closed. 2 new `input.rs` tests (digit accumulation, bare-0-vs-count-0).
- **`V` (VisualLine) linewise selection — actually extends now.**
  Previously `V` behaved identically to `v` (anchor/cursor stayed
  character positions, no whole-line semantics at all, a gap called out
  explicitly in earlier rounds but not acted on). Implemented the real
  `v`<->`V` toggle matrix config.rs now encodes (`v` in Visual deselects
  to Cursor, `V` switches to VisualLine; mirrored in VisualLine) plus
  `display_selected_range()` in `main.rs`, which expands a VisualLine
  selection to whole lines only for display/copy (the underlying
  anchor/cursor stay character positions — simpler than tracking a
  separate linewise flag everywhere). `copy_selection` also appends the
  trailing newline `visual.js` does for a linewise yank.
- **Cross-page `j`/`k`.** New `move_vertical_cross_page` in `main.rs`,
  a direct port of `visual.js`'s page-crossing logic in `move()`:
  steps within the current page first, then crosses into the next
  non-empty (and now also: already-rendered — see below) page,
  repeating per remaining count, matching the original including
  skipping blank pages.
- **Gutter line numbers.** New gutter column (width from
  `config.appearance.gutter_width`, previously parsed but never used)
  to the left of the page the keyboard cursor is on, shown only in
  Cursor/Visual/VisualLine mode — relative distance from the cursor's
  line, absolute on the cursor's own line, matching `visual.js`'s
  `lineNumber()`. Simplified versus the original: only computes
  same-page relative numbers, not the original's full cross-page
  relative-distance accounting — a deliberate scope cut to avoid a
  cross-page line-count pass every frame; flagged, not hidden.

**Performance — the "especially fast" ask.** Root cause: `Viewer::load`
rendered and GPU-uploaded *every page* of a document synchronously
before the window could show anything, on every document open and every
tab switch (zoom was already fixed to not do this, two rounds back).
Rewrote `viewer.rs`: `Viewer::load(doc)` now only reads each page's size
(`document::Page::size()`, no rasterization — fast regardless of page
count), producing a `PageView { size_pt, rendered: None }` per page.
`Viewer::render_page(doc, index, scale, ctx)` rasterizes and caches one
page on demand; a render failure is recorded as an empty placeholder
(`RenderedPage::empty`) instead of retried every frame. `main.rs`'s new
`progressive_render` calls it for up to 2 not-yet-rendered pages per
frame, calling `ctx.request_repaint()` while any remain so loading
continues across frames without needing new input. The page-display loop
now branches on `rendered`: a plain canvas-tinted placeholder (correctly
sized from `size_pt * zoom`, so scrolling/layout stay correct) for pages
not yet rendered, the existing display/overlay code otherwise. Net
effect: opening even a long document shows page 1 immediately and fills
in the rest over the next several frames, instead of blocking the whole
window until every page is decoded — this was real, not a false alarm
like the zoom-key/dispatcher scares in earlier rounds; confirmed the
`Viewer::load` call was genuinely synchronous and page-count-proportional
before changing anything.

This touched enough of `viewer.rs`'s public shape (`PageView` losing its
direct `texture`/`chars`/`lines` fields in favor of `rendered: Option<
RenderedPage>`) that every call site in `main.rs` needed updating — added
`SpikeApp::page_lines`/`page_chars` helpers (`None` for both "page index
invalid" and "page not rendered yet", callers treat both the same: wait,
don't crash) so the ~14 sites that read a page's lines/chars go through
one place instead of each re-deriving the same `Option` chain.
`move_vertical_cross_page` additionally now treats an unrendered
neighboring page the same as an empty one (skip past it) rather than
trying to cross into it mid-render.

2 `viewer.rs` smoke tests rewritten for the new lazy API (confirms
`load()` rasterizes nothing, `render_page` fills pages in one at a time,
scale still scales texture size correctly). Re-verified: `cargo test`
43/43 (3x stable), `--dump` still md5-identical across all 8 PNGs
(dump_fixtures was already decoupled from `viewer.rs` entirely, so this
whole rewrite didn't touch it), debug and `--release` both start clean
with multiple tabs open. Perceived startup smoothness itself still needs
the user's own eyes — no display here to confirm beyond "doesn't crash,
returns control promptly."

## Eighth round — style parity: "looks like a typical Rust app," lines look weird

User rejected the look outright: wanted the minimal old-app aesthetic,
not egui's default chrome, called the debug overlay "weird," and asked
to start from a plain single-page example like they'd used testing the
old app. Read `style.css`/`shell.css`/`theme.css` directly again rather
than going by the color values already cross-checked two rounds back —
that check only covered colors, not layout/chrome, and those turned out
to be most of the mismatch.

Key finding in `style.css` itself: `/* Keep mode/cursor announcements
accessible without an on-screen mode bar. */` — the old app has **no
permanent status/mode bar at all**. `.pdf-visual-status` is screen-reader-
only (clip-path hidden) except `.is-notice` messages, which are a small
floating toast bottom-left, auto-hidden after 3s. Our permanent toolbar
(mode label + status + a long keybinding-hint string, every frame) was
never part of the design being matched — removed entirely.

- **Debug box overlay defaulted to visible.** `show_boxes: true` was the
  default this whole time — a gate-validation tool (see PLAN.md's
  "Biggest risk" section), never meant to be part of the real viewing
  experience. Almost certainly "lines look weird." Default flipped to
  `false`; still toggleable with `Cmd+B`.
- **Permanent toolbar removed**, replaced with `notify()`: a transient
  floating notice (`egui::Area`, bottom-left anchored) matching
  `.pdf-visual-status.is-notice`'s exact styling (small rounded box,
  `#fff`/`#333` bg per theme, 12px monospace, drop shadow, 3s
  auto-hide via a stored `notice_until` time checked each frame). Wired
  to copy confirmations (`copy_selection`) and load errors
  (`load_viewer`) — the same two cases `visual.js`'s `message(text,
  true)` covers. Routine per-motion page/line/char feedback still
  updates `self.status` internally (parity hook for real accessibility
  work later) but is no longer painted at all, matching the CSS comment
  exactly instead of the earlier compromise of showing it permanently.
- **Tab bar restyled** to match `shell.css`'s `.pdf-shell-tabs` exactly:
  centered (was left-aligned), transparent background (was egui's
  default panel fill), 12px monospace, current tab in plain `#d93025`
  red/bold text with no fill or button border (was a filled
  `selection.bg_fill` button) — still hidden with one tab open.
- **Gutter colors** corrected to the CSS's actual values — `#777`/`#aaa`
  gray depending on theme, `#d93025` red for the current line — was
  using unthemed `Color32::GRAY`/`WHITE` before. (Bold weight for the
  current line's label not attempted — egui's default monospace font has
  no bold variant readily available without registering a second font
  family; color alone already carries the same distinction the CSS
  relies on.)
- **Default launch document changed.** `cargo run` (no args) now opens a
  single new fixture, `assets/test-pdfs/letter.pdf` (one page, plain
  prose, generated the same way as the other synthetic fixtures) —
  previously it opened all 4 dev fixtures as tabs, including the blank-
  shapes page, which is exactly what caused an earlier live-testing
  round to land on the wrong page by default. The dev fixtures are
  unchanged and still used by `--dump` and reachable via explicit CLI
  args; only the no-args default changed. `FIXTURES` (the const array)
  is otherwise untouched.

Not touched this round, explicitly out of scope for a style pass: page
border/shadow (already `box-shadow: none`-equivalent — egui's bare
`Image` widget adds no border by default, so no change was needed there),
dark-mode page-content inversion (already a flagged, separate gap).

Re-verified: `cargo test` 43/43, `--dump` still md5-identical across all
8 PNGs (none of this touched rendering/geometry, only chrome/defaults),
debug and `--release` both start clean. Visual confirmation of the new
look is, as always, the user's to do — no display here.

## Ninth round — horizontal centering and zoom-to-cursor (planned first, per user's explicit request)

Two more layout/UX gaps: pages pinned to the left edge instead of
centered (gray canvas margin only on the right), and zoom just changing
`self.zoom` with no anchor, so the view jumped instead of keeping a
point stationary like every other PDF/image/map viewer. Used plan mode
for this one per the user's workflow at the time; plan approved as
written, implemented as specced.

- **Horizontal centering.** `viewport_size` captured via
  `ui.available_size()` at the top of the `CentralPanel` closure, before
  entering `ScrollArea::both()` — inside the scroll area,
  `ui.available_width()` is effectively unbounded (that's what makes
  horizontal scrolling work), so it's unusable for centering math and
  has to be captured before. Each page (both the placeholder and
  rendered branches) now computes `pad = ((viewport_size.x - content_width)
  / 2.0).max(0.0)` and wraps itself in `ui.horizontal(|ui| { ui.add_space(pad);
  ... })` — `content_width` includes the gutter's width when shown, so
  the page+gutter stay centered together, not just the bare page. `pad`
  is 0 once zoomed past the window width, falling back to flush-left
  with horizontal scroll — the standard behavior every reader uses once
  content doesn't fit.
- **Zoom anchored at the cursor (viewport center if nothing's hovered).**
  New `SpikeApp` fields: `scroll_offset: egui::Vec2` (captured each frame
  from `ScrollAreaOutput::state.clamped_offset()`) and
  `hover_in_viewport: egui::Pos2` (pointer position relative to the
  scroll viewport's top-left via `ui.clip_rect()`, defaulting to the
  viewport center when nothing's hovered — same fallback `keys.js`'s
  `zoom()` uses). New `zoom_to(&mut self, new_zoom)` method: converts the
  anchor to content-space (`scroll_offset + anchor`), updates
  `self.zoom`, rescales that content point by `new_zoom/old_zoom`
  (everything — page size, gutter, gaps — already derives linearly from
  `self.zoom`, so this is exact, not an approximation), and sets
  `pending_scroll` to the delta needed to land the anchor back at the
  same screen position next frame. `Ctrl+K`/`Ctrl+J`/`Ctrl+0` now call
  `zoom_to` instead of setting `self.zoom` directly.
  `pending_scroll` was `Option<f32>` (Y-only, sufficient when only
  `j`/`k`/`gg`/`G`/half-page-scroll used it); widened to
  `Option<egui::Vec2>` so the zoom correction can shift both axes in one
  `ui.scroll_with_delta` call — existing Y-only callers now pass
  `egui::vec2(0.0, dy)`.

No test/dump changes needed — pure interactive layout/zoom code, nothing
`--dump`'s headless path exercises. Re-verified: `cargo test` 43/43,
`--dump` still md5-identical, debug/`--release` both start clean. Visual
confirmation (equal margins, point-stable zoom, cursor-off-page fallback
to center) is the user's to do next — no display here.

## Tenth round — zoom-anchor sign bug

User confirmed centering but called zoom "definitely not correct."
Re-derived the math from egui's own source rather than re-guessing:
`Ui::scroll_with_delta`'s delta is inverted from the direction the
offset actually moves (`scroll_area.rs`: `let mut delta = -scroll_delta;`
then applied as `offset += delta`) — so the delta needed to reach a
target offset is `old_offset - target_offset`, not `target_offset -
old_offset`. `zoom_to`'s `pending_scroll` was computed with that
backwards, so each zoom step scrolled the view *away* from the cursor
instead of keeping the anchor point stationary — the symptom was "weird"
because it's non-obviously wrong (the view does move, scales do change,
just the compensation fights itself).

Fixed by extracting the pure math into a standalone `zoom_scroll_delta
(old_offset, anchor, old_zoom, new_zoom) -> egui::Vec2` (no `self`, no
egui UI calls — just arithmetic) and fixing the sign; `zoom_to` now
calls it. 3 new `zoom_tests` pin this down directly: two property-style
checks (zoom in, zoom out) that apply the returned delta the same way
egui actually does (`offset - delta`, mirroring `scroll_area.rs`'s
`offset += -delta`) and assert the anchor's content point lands exactly
back where it started, plus a same-zoom no-op check. These would have
caught the sign bug immediately — written after the fact here, but this
is exactly the kind of "the formula reads right but the sign convention
doesn't match the library's actual behavior" bug a test should have
caught before it shipped, not after a live report.

Re-verified: `cargo test` 46/46 (43 + 3 new), `--dump` still
md5-identical, debug/`--release` both start clean. Zoom-to-cursor
behavior itself still needs the user's live confirmation — this is the
second time this exact feature needed a round-trip fix, so worth
checking carefully this time: zoom in/out with cursor off-center, and
confirm the point under the cursor visually stays put rather than
sliding in either direction.

## Eleventh round — zoom flicker, cursor-mode dark toggle, tab bar layout/caching

User confirmed cursor-anchored zoom works, then reported four more
issues in follow-up turns, each fixed and re-verified before moving to
the next: zoom flickered for one frame per step (egui applies
`scroll_with_delta` after painting; fixed by `ctx.request_discard()`-ing
that one frame, confirmed against `egui::Context::run_ui`'s own
multi-pass docs rather than guessed); `i` inside Cursor/Visual toggled
dark mode instead of starting `is`/`ip` (scoped the dark-mode shortcut
to Normal mode only); zoom used an invented `×1.2`/`÷1.2` multiplier
instead of `keys.js`'s actual `ZOOM_LEVELS` stops (ported the table
verbatim); switching tabs was "slow as hell" (every switch rebuilt the
`Viewer` from scratch — added a `viewers: Vec<Option<Viewer>>`
park-and-restore cache so revisiting a tab reuses its already-rendered
pages); the top tab bar wasn't actually centered or transparent
(`egui::Panel`'s default separator line and `ui.horizontal`'s
full-width claim were both silently defeating the intended styling —
disabled the separator, centered by measuring label widths and
padding by hand) and painted onto the window's own near-black default
instead of the canvas color (moved `canvas_color`'s computation above
the tab bar and filled it with that instead of `TRANSPARENT`). `cargo
test` 48/48 across these fixes, `--dump` unaffected throughout (none of
this touches rendering/geometry).

## Twelfth round — dark-mode page inversion, GPU texture-size clamp, word-join spaces, render.rs

User asked for a priority-ordered pass across the remaining PLAN.md
gaps, explicitly asking for dark-mode page-content inversion first.

- **Dark-mode page-content inversion.** The oldest flagged gap in this
  file: dark mode only ever recolored the canvas background, never the
  rendered page bitmap itself (`theme.css`'s `--preview-dark-page-filter`,
  `filter: invert(0.9) hue-rotate(180deg)`, applied to PDF.js's canvas
  in the old app). New `viewer::apply_dark_filter(&mut RgbaImage)`
  applies the CSS Filter Effects spec's own `invert()`/`hue-rotate()`
  matrices directly to the rasterized bitmap (hue-rotate(180°)
  pre-evaluated since the angle is fixed) — not reinvented, not
  approximated. `RenderedPage` gained a `dark: bool` field recording
  which theme a page's texture was rasterized under; `render_page`
  re-rasterizes on a mismatch instead of leaving a stale-themed texture,
  and `next_unrendered`/the new `render::pages_to_render` (see below)
  both treat a theme mismatch as "needs rendering" so toggling dark mode
  progressively re-renders the active tab's pages exactly like opening
  a new document does. 2 new `viewer.rs` tests pin the filter against
  hand-evaluated spec matrices (white -> near-black, black -> light
  gray, pure red -> light warm tint rather than cyan — confirms the
  filter order is invert-then-rotate, not the reverse).
- **GPU texture-size safety.** Flagged, unfixed gap from the third
  live-testing round: nothing clamped a page's rasterized resolution
  against the backend's real texture-size limit. `RenderedPage` gained
  a `render_scale: f32` field (the scale a page was *actually*
  rasterized at, which may be lower than requested); `render_page` now
  clamps to `max_texture_side` (`ctx.input(|i| i.max_texture_side)`,
  2048 default / the real GPU limit on a live backend) per page before
  rasterizing, so an unusually large/landscape page at high zoom is
  downscaled instead of panicking on texture upload. Every display-space
  consumer (`char_at_pos`, the overlay/gutter draw loop) now divides by
  the *rendered* page's own `render_scale`, not the single `RENDER_SCALE`
  constant, so a clamped page's char boxes/cursor/selection still align
  correctly instead of assuming uniform resolution across all pages.
  2 new tests (clamped texture size, scale-mismatch tracking).
- **Word-joining across rendering gaps.** `visual.js`'s `collect()`
  synthesizes a space between two PDF.js text-layer runs that sit on
  the same line with a gap wider than 15% of the shorter run's height
  and neither run already starts/ends in whitespace — `document.rs`/
  `text.rs` never had an equivalent, so a PDF whose content stream omits
  an explicit space glyph between words could misjoin them (flagged,
  unfixed gap from the `selection.rs` step). Ported directly:
  `text::position_chars` now runs its output through a new
  `insert_synthesized_spaces`, which applies the exact same
  condition/ratio per consecutive character pair instead of per text
  run (PDFium already reports individual characters, not grouped runs,
  so the unit of comparison is naturally finer-grained than `visual.js`'s
  but the condition itself is unchanged). 4 new `text.rs` tests (wide
  gap gets a space, narrow gap doesn't, a gap across *different* lines
  doesn't, an already-adjacent real space isn't duplicated). `--dump`'s
  char counts are unchanged across all 4 fixtures — expected, since none
  of them happen to omit an explicit space glyph; this only activates
  for the PDFs the gap was about.
- **`render.rs` — built (viewport-priority scheduling + memory bound;
  generation tagging explicitly explained as not applicable yet).** New
  `native/src/render.rs`, pure page-index arithmetic (no PDFium, no
  egui, same boundary discipline as `text.rs`/`selection.rs`):
  `pages_to_render(current_page, page_count, is_unrendered, budget)`
  expands outward from `current_page` (current, +-1, +-2, ...) instead
  of always scanning from page 0 — found gap: jumping to a late page
  (`G`, a future "goto page") used to render every earlier page first
  before ever reaching the one actually on screen, since
  `Viewer::next_unrendered` only ever found the first unrendered index
  in document order. `pages_to_evict(current_page, page_count,
  is_rendered, keep_radius)` returns pages far enough from
  `current_page` to free their GPU texture — `main.rs`'s
  `progressive_render` now calls both every frame (budget 2/frame to
  render, keep-radius 40 pages to evict), closing PLAN.md's "sane
  memory limits for large PDFs" rule, previously entirely unimplemented
  (pages only ever accumulated, never freed). "Current page" for both
  is derived from a new `SpikeApp::page_near(point)` helper (extracted
  from `zoom_to`'s existing nearest-page-to-a-point lookup over
  `page_rects`, now shared) evaluated at the viewport center; one frame
  stale since `progressive_render` runs before `CentralPanel` repaints
  `page_rects` that frame, same tolerated staleness as `zoom_to`'s
  pointer anchor. Generation tagging (PLAN.md's Port-order item 5
  requirement) is explicitly **not** built: every render call here is
  synchronous on the UI thread, so there's no background job whose
  result could arrive stale — the race that tagging exists to prevent
  doesn't occur in this threading model; documented in `render.rs`'s own
  module doc rather than silently skipped. One real bug caught while
  wiring this up, not by inspection: eviction guarantees some page is
  *always* unrendered once a document exceeds the keep-radius window, so
  the old "is anything in the whole document unrendered" repaint-continue
  check would never go false again, repainting forever for no visible
  reason — fixed by checking a bounded nearby window (`2 * keep_radius +
  1`) instead of the whole document. 6 new `render.rs` tests (outward
  expansion order, skipping already-rendered neighbors, terminating at
  document edges, clamping an out-of-range current page, empty-input
  no-ops, eviction's radius boundary).

Re-verified: `cargo test` 63/63 (was 48; +2 dark filter, +2 texture
clamp/scale-tracking, +4 word-join, +6 render.rs, +1 dark-toggle
re-render), `cargo build`/`--release` both clean with no warnings,
`--dump` still byte-identical across all 8 PNGs (none of this touches
the headless dump path — `apply_dark_filter`/`render.rs` are both new
code paths `dump_fixtures` never calls). None of this round's visible
behavior (page inversion look, zoom/scroll smoothness with eviction
active, word-join correctness on a real gap-omitting PDF) has been
confirmed live yet — worth a pass focused on dark mode specifically
(does inverted text stay legible, do colors look reasonable) since
PLAN.md's Config file section flagged "do not assume parity with the
CSS filter without a side-by-side check" from the very first round.

## Next up — prioritized per the user's own ordering

Asked and answered in order of what the user actually asked for next:

 1. **In-document search.** Implemented in the native viewer: `/` and `?`
    open a query prompt; Enter searches every page's extracted text without
    rasterizing it; Escape cancels; `n`/`N` navigate with wraparound and
    counts. Matches and the current match have separate configurable colors.
    Search indices survive page texture eviction; switching tabs clears them.
    `cargo test` (72 passing) and `cargo build` passed on macOS. Live keyboard/
    visual verification is still pending. Full-document extraction currently
    runs on the UI thread when Enter is pressed and may pause on large PDFs.
 2. **Open-file UX.** `Ctrl+O` opens a native file picker (a new tab, same
    as a CLI arg); drag-and-drop a PDF onto the window opens it the same
    way. Part of Port order item 5 (Lifecycle) but scoped down to just
    these two interactive paths, not "Open with" OS handoff or
    single-instance IPC yet. Implemented with `rfd` and egui's dropped-file
    input. Failed opens leave existing tabs intact; non-PDF drops report an
    error. Live picker and drop testing still pending.
3. **Sentence segmentation.** Replace the `.`/`!`/`?`+whitespace regex
    heuristic (mis-breaks on "Dr. Smith") with Unicode UAX #29 boundaries.
    Implemented using `unicode-segmentation`, plus a small common-title
    exception because UAX #29 itself splits "Dr. Smith". Paragraph gaps
    become sentence boundaries. Tests cover titles, Unicode scalar offsets,
    visual-line continuation and paragraph gaps; live behavior is unverified.
4. **Cross-page gutter numbers.** Extend the gutter's relative-line-number
   accounting across page boundaries instead of only within the
   cursor's current page. Not started.
5. **Rendering performance** (launch, opening big PDFs, scrolling, zooming
   all reported slow live). Root cause traced to two things: (a)
   `progressive_render` called `Viewer::render_page` — PDFium rasterization,
   dark-filter math, and `FPDFText_*` extraction — synchronously on the UI
   thread every frame there was nearby unrendered work, blocking input and
   repaints for as long as that took; (b) every near-viewport page was
   rendered at a fixed 3x-zoom resolution (`RENDER_SCALE`) regardless of
   the *actual* display zoom, so a long document at 100% zoom spent most
   of its rendering budget on pixels nobody could see yet.
   Fixed: added `render_worker.rs`, one dedicated background thread that
   owns rendering (`viewer::prepare_page`, factored out of the old
   `Viewer::render_page` to have no `egui::Context` dependency) and
   communicates with the UI thread over channels (`RenderJob` in,
   `RenderResult` out); `progressive_render` now only submits jobs and
   applies finished results (a cheap GPU texture upload), never blocking on
   PDFium itself. One worker thread, not a pool — `pdfium-render`'s
   `thread_safe` feature (on by default) serializes all PDFium calls behind
   one internal mutex regardless, so a pool would add complexity without
   adding throughput. `app.rs`'s `OpenDocument::document` is now
   `Arc<Document>` so the worker can hold its own cloned handle across tabs.
   Also added `target_render_scale` (zoom times a small headroom factor,
   capped at the old fixed ceiling): a page renders near its *current* zoom
   instead of always at max, and `page_needs_render` (unit-tested directly)
   decides staleness against what was *requested* rather than what was
   actually delivered, so a page clamped below target by `max_texture_side`
   doesn't get endlessly re-requested. `cargo test` (86 passing, including a
   worker-thread integration test and 6 `page_needs_render` cases) and
   `cargo build`/`--release` both clean, no warnings. Not done: live
   before/after frame-time measurement (no screen-recording permission in
   this sandbox) — the fix is architectural and unit-tested, not yet
   confirmed against a stopwatch on a real large PDF; full-document search
   extraction (`run_search`, see item 1 above) still runs synchronously on
   the UI thread and is a plausible next target if search itself is still
   slow after this.

   Two regressions found live after this landed, both fixed in the same
   round: (a) Cursor mode always started on page 0 regardless of scroll
   position — unrelated to the rendering change itself, but surfaced
   during the same live pass; `keyboard_page()` fell back to a fixed `0`
   whenever the mouse hadn't clicked a page yet, instead of asking where
   the view actually was (`page_near`, the same helper `progressive_render`
   already uses for render priority). Fixed by reusing it there too — now
   falls back to the page nearest the viewport center, only defaulting to
   `0` before the very first frame has painted anything. (b) Switching
   between *other* apps got janky while a big PDF was still catching up in
   the background — traced to `progressive_render`'s "more nearby work"
   repaint request being unconditional: previously each repaint also did
   real synchronous rendering (self-throttling by how long that took); now
   submission is nearly free, so this window could pump many more frames
   per second while catching up, continuously demanding frames from the OS
   compositor even while unfocused. Fixed with a `request_repaint` helper
   that checks `ctx.input(|i| i.focused)`: immediate while focused, a
   250ms `request_repaint_after` otherwise — background rendering still
   finishes by the time the user returns, without competing for compositor
   time while they're switching among other windows. Applied to both this
   repaint and the notice-box countdown's. Neither regression had a unit
   test catch it (focus/compositor behavior isn't exercised by
   `egui::Context::default()` in tests, and the cursor-start position was
   only ever checked indirectly) — live testing remains the only check for
   both; `cargo test` (86 passing) and `cargo build`/`--release` stayed
   clean throughout.

   Three more issues found on the next live pass, after the above:
   "still performance issues," "pages hangs when entering cursor mode,"
   and "rendering also some kind of weird" — researched against how
   other viewers actually do this (PDF.js's render queue, which always
   promotes the most-visible page over submission order; Sioyek's
   `pdf_renderer`; PDF.js/Chrome sizing their canvas as `scale *
   devicePixelRatio` rather than a fixed resolution) and fixed to match:
   - **Rendering "weird" (blurry).** `target_render_scale` computed a
     resolution from zoom alone, with no accounting for the display's
     own pixel density (`egui::Context::pixels_per_point()`, e.g. 2.0 on
     a Retina/HiDPI screen — this app's own screen). The fixed constant
     it replaced happened to still look fine at 100% zoom on a 2x
     display purely by accident (it was generously oversized for *every*
     zoom level, so the common case stayed comfortably above what 2x
     density needed even though nothing accounted for density
     explicitly) — once resolution started tracking zoom, the same gap
     became visible at the most common zoom level, not just at extremes.
     Fixed by multiplying by `pixels_per_point` directly, matching how
     PDF.js/Chrome size their own canvas; renamed the old `RENDER_SCALE`
     ceiling to `MAX_RENDER_SCALE`, now a generous safety bound rather
     than the normal target.
   - **Cursor mode hanging after a fast scroll/jump, and the underlying
     slowness.** The render worker (new this round) used a plain FIFO
     channel: queued jobs ran in *submission* order, so a fast scroll or
     a page jump (`G`, a big `j`/`k` count) could leave the page actually
     on screen stuck behind a backlog of now-irrelevant jobs for pages
     scrolled past moments earlier — Cursor mode needs that page's text
     already extracted, so it visibly "hung" waiting for its turn. Fixed
     by replacing the channel with a shared priority queue
     (`render_worker.rs`'s `SharedState`/`pick_next`): jobs are keyed by
     `(tab, page)` in a map instead of ordered in a channel, `main.rs`
     reports its current page every frame via `set_focus`, and the
     worker always renders whichever queued job is nearest that live
     focus — a stale request for a page scrolled away from simply loses
     priority to whatever's actually on screen now, with no separate
     cancellation step needed for that case. `progressive_render` also
     no longer trickles submission across several frames (a leftover
     FIFO-era caution): submitting is now a cheap map upsert, so every
     candidate in the nearby window is registered the same frame it's
     found, and the worker's priority picking — not submission order —
     decides what renders first. Eviction additionally calls the new
     `RenderWorker::cancel` to drop a not-yet-started request for a page
     that just scrolled out of the keep radius.
   `cargo test` (94 passing, including 4 new `pick_next` priority-rule
   cases and a dedup-on-resubmit case) and `cargo build`/`--release`
   stayed clean. Still not done: live confirmation on a real large PDF —
   this sandbox has no display/screen-recording access, so all three
   fixes are verified by their unit tests and by reasoning about the
   traced mechanism, not by a stopwatch or a side-by-side look.

   A fourth, bigger root cause found right after, from the user's own
   486-page technical book (profiled directly, see `--profile <path>`
   below): opening *this specific file* still froze for seconds,
   independent of every fix above. Added a `--profile <path>` headless
   CLI flag (`profile_document`) that times the exact steps this app
   does per page with no display needed, to measure instead of guess —
   it found `Viewer::load` itself (not `progressive_render`, not the
   render worker) spending **1.5 seconds** synchronously opening every
   one of the 486 pages just to read its size, before the window could
   paint anything at all. That loop predates this round's render-worker
   work entirely; moving rendering off the UI thread never touched it.
   Fixed the same way as rendering: `Viewer::load` now reads only page
   0's real size and uses it as every other page's placeholder (correct
   immediately for a uniform-page-size document, which this one is —
   confirmed by the profiler, every sampled page was identically
   476x680pt); a new one-shot background thread
   (`Viewer::measure_page_sizes`, `main.rs`'s `spawn_page_size_measurement`/
   `apply_pending_page_sizes`) measures every page's real size in the
   background and corrects any that differ once it finishes. Re-profiled
   the same file after the fix: `Viewer::load` dropped from 1.5s to
   **19.8ms**; the full 486-page measurement (now ~2.2s, off the UI
   thread) no longer blocks anything. Per-page rendering for this
   specific document is also inherently non-trivial (dense vector
   diagrams at ~2190x3129px, 24-170ms/page measured) — now happening on
   the background render worker instead of the UI thread, same as every
   other document; not something to further optimize without evidence
   it still matters after this. 3 new tests (`viewer.rs`'s
   `page_size_tests`) cover the placeholder, the correction, and the
   length-mismatch no-op. `cargo test` (97 passing) and `cargo build`/
   `--release` clean. Confirmed against the user's real file via
   `--profile`, not just a unit test — the one piece of this whole
   performance effort actually measured against the reported-slow file
   itself, not a synthetic fixture.
   Follow-up on the same 486-page PDF: each raster at 100% zoom on a 2x
   display is 2190×3129 RGBA pixels (~27 MB raw). The render window
   previously queued and retained up to 81 pages near the middle of the
   book: ~2.2 GB of raw pixels before upload copies, plus continuous
   background PDFium work. The window is now the visible page ±3
   (at most seven), queued pages outside it are canceled, and canceled
   results cannot restore evicted textures. The worker wakes egui when
   results arrive, rather than spinning repaints while rendering; the
   UI uploads only one finished texture per frame. Tests: 99 pass;
   release build clean. Memory figures are calculated from the profiled
   bitmap dimensions, not measured process RSS; GUI frame timing still
   needs a live display check.
   Cursor-mode follow-up on pages 20→21 of the same PDF: page 20 renders
   in 38 ms (39 text lines) and page 21 in 34 ms (41 lines) in the
   headless profile. The cursor state, not unusual page content, was the
   culprit: `active_page` and `page_cursor` persisted when leaving Cursor
   mode, and the old line could be reused on the newly focused page.
   Entry now selects the viewport's page and nearest line, only reuses
   a cursor that belongs to that page and remains within its text, and
   completes placement when a not-yet-rendered page arrives. Keyboard
   motions also follow the viewport when the former page is offscreen.
   Regression tests cover page changes, pending page completion, and
   re-entry after scrolling within one page.
   Zoom/tab follow-up: `progressive_render` previously picked a focus
   page and evicted textures before the frame laid out its new zoom or
   restored scroll offset. After moving result collection before layout
   and scheduling/eviction after layout, zoom correction's discarded
   intermediate pass no longer evicts from stale rectangles. A pending
   low-resolution render is superseded when zoom demands more pixels;
   its late result cannot replace a sharper texture. Each document tab
   now gives egui's ScrollArea a distinct persistent ID, retaining its
   own vertical/horizontal scroll offset through tab switches. Tests
   cover independent tab offsets and stale low-resolution results.
   Startup/maximized follow-up: release headless profile of the user's
   PDF measured config ~1 ms, PDFium load 15 ms, document open 53 ms,
   and `Viewer::load` 17 ms. The 486-page size scan still took 1.6 s
   and previously started on the first UI frame, competing with nearby
   page rendering for PDFium. It now begins only after the visible page
   and its prefetched neighbors are ready. A forced maximized window was
   tried, then removed because it conflicts with AeroSpace's window
   management; native sizing is left to macOS/the window manager. The
   default small centered 800×600 window was then replaced by a larger
   1200×800 initial size without forcing maximized state.
   This removes scan contention from first
   paint; actual GPU/window startup latency remains unmeasured here.
6. **Platform coverage** (explicitly last per the user). Windows/Linux
   PDFium binaries, smoke tests, and config-path verification — only
   macOS x86_64 has ever been exercised.
7. **Packaging** (explicitly last, after basic functionality). Signing/
   notarization, an example config file documenting modifier-key names
   and platform differences, a genuine scanned/non-text fixture, and
   — new this round — a decision on how this app actually launches as
   an installed, double-clickable native app (see below), mirroring
   `install.sh`/`install.ps1`'s job for the current Python/PDF.js app.

Already done, not re-listed as a gap: the long-term config file for
keys + appearance the user asked about — `config.rs` already is that
(TOML, `~/.config/ggpdf/config.toml` first on every platform, falling
back to the platform-native path; see "Config file" section above).
What's still missing there is just the example/documentation file
itself (folded into item 6 above), not the mechanism.
