# Windows PDF startup: investigation handoff

The first ggpdf `--app` window takes 2–3 seconds to display its PDF. A second
ggpdf window opens and renders much faster, even though it also creates a new
`--app` window.

## Established

- The Python server remains listening on port 18765 after all ggpdf windows
  close; restarting the server is not the recurring delay.
- Keeping ordinary Chrome open does not speed up the first ggpdf window.
- Opening the same ggpdf viewer URL in an ordinary Chrome tab *does* speed up
  the next `--app` window.
- HTTP caching of static viewer assets did not noticeably reduce this delay.

## Next investigation on Windows

The evidence points to a cold load of ggpdf's local PDF.js page in Chrome, but
does not yet distinguish script initialization, worker startup, and first-page
rendering. Profile the first load through the first rendered page before
changing the viewer. `preview.py` serves the page and starts Chrome with
`--app=`; `vendor/web/viewer.html` loads `vendor/web/viewer.mjs`,
`vendor/build/pdf.mjs`, and the PDF.js worker at
`vendor/build/pdf.worker.mjs`.

Keeping one ggpdf PDF open in a pinned Chrome tab is a confirmed workaround.
