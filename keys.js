(() => {
    const ZOOM_LEVELS = [
        0.5, 0.6, 0.7, 0.8, 0.9, 1,
        1.1, 1.3, 1.5, 1.7, 1.9, 2.1, 2.4, 2.7, 3,
    ];
    const ZOOM_KEY = "pdf-preview-zoom";
    const THEME_KEY = "pdf-preview-dark";
    let lastGPress = 0;
    let pointer = null;
    let visual = null;

    function load(key) {
        try { return localStorage.getItem(key); } catch { return null; }
    }

    function save(key, value) {
        try { localStorage.setItem(key, String(value)); } catch { /* Optional. */ }
    }

    function setTheme(dark) {
        document.documentElement.dataset.pdfDark = String(dark);
        document.documentElement.style.colorScheme = dark ? "dark" : "light";
        save(THEME_KEY, dark);
    }

    setTheme(load(THEME_KEY) === "true");

    document.addEventListener("webviewerloaded", () => {
        const app = window.PDFViewerApplication;
        const options = window.PDFViewerApplicationOptions;
        const stored = Number(load(ZOOM_KEY));
        options.set("disablePreferences", true);
        options.set("enableScripting", false);
        // PDF.js waits for WebGPU device initialization before fetching the PDF.
        options.set("enableWebGPU", false);
        // Ignore a PDF's requested sidebar mode and any saved sidebar state.
        options.set("sidebarViewOnLoad", 0);
        options.set("defaultUrl", "");
        options.set("defaultZoomValue", String(
            stored >= 0.5 && stored <= 3 ? stored * 100 : 100
        ));
        options.set("viewerCssTheme", load(THEME_KEY) === "true" ? 2 : 1);
        app.initializedPromise.then(() => {
            visual = new window.PDFPreviewVisualSelection(app);
            app.eventBus.on("scalechanging", ({ scale }) => {
                if (Number.isFinite(scale) && scale >= 0.5 && scale <= 3) {
                    save(ZOOM_KEY, scale);
                }
            });
        });
    }, { once: true });

    document.addEventListener("pointermove", (event) => {
        pointer = [event.clientX, event.clientY];
    }, { passive: true });

    function editable(event) {
        return event.composedPath().some((element) =>
            element instanceof HTMLElement && (
                element.matches("input, textarea, select, [role='textbox']") ||
                element.isContentEditable
            )
        );
    }

    function zoom(app, direction) {
        const current = app.pdfViewer.currentScale;
        let next = 1;
        if (direction > 0) {
            next = ZOOM_LEVELS.find((level) => level > current + 0.001) ?? 3;
        } else if (direction < 0) {
            next = ZOOM_LEVELS.findLast((level) => level < current - 0.001) ?? 0.5;
        }
        const rect = app.pdfViewer.container.getBoundingClientRect();
        const origin = pointer && pointer[0] >= rect.left && pointer[0] <= rect.right &&
            pointer[1] >= rect.top && pointer[1] <= rect.bottom
            ? pointer : [rect.left + rect.width / 2, rect.top + rect.height / 2];
        app.updateZoom(null, next / current, origin);
    }

    document.addEventListener("keydown", (event) => {
        const app = window.PDFViewerApplication;
        if (!app?.pdfDocument || !app.isInitialViewSet || event.defaultPrevented ||
            event.isComposing || editable(event) || app.overlayManager.active ||
            app.pdfViewer.isInPresentationMode) {
            lastGPress = 0;
            return;
        }

        const target = app.pdfViewer.container;
        if (visual?.handleKey(event)) {
            lastGPress = 0;
            event.preventDefault();
            event.stopImmediatePropagation();
            return;
        }
        const scroll = (top) => target.scrollBy({ top, behavior: "instant" });
        const jump = (top) => target.scrollTo({ top, behavior: "instant" });
        let action;
        const plainCtrl = event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey;
        if (plainCtrl) {
            action = {
                k: () => zoom(app, 1),
                j: () => zoom(app, -1),
                "0": () => zoom(app, 0),
                u: () => scroll(-150),
                d: () => scroll(150),
            }[event.key.toLowerCase()];
        } else if (!event.ctrlKey && !event.metaKey && !event.altKey) {
            if (event.key === "g") {
                const now = Date.now();
                if (lastGPress && now - lastGPress <= 500) {
                    action = () => jump(0);
                    lastGPress = 0;
                } else {
                    lastGPress = now;
                    action = () => {};
                }
            } else {
                action = {
                    j: () => scroll(50),
                    k: () => scroll(-50),
                    G: () => jump(target.scrollHeight),
                    i: () => setTheme(document.documentElement.dataset.pdfDark !== "true"),
                    q: () => window.close(),
                }[event.key];
            }
        }
        if (event.key !== "g") lastGPress = 0;
        if (action) {
            event.preventDefault();
            event.stopImmediatePropagation();
            action();
        }
    }, true);
})();
