/* Hosts one or more PDF.js viewer panes, switched like tmux windows.
 * Panes arrive over a Server-Sent Events channel: one for the documents this
 * page was opened to show, more whenever another ggpdf invocation runs while
 * this window is the active session. */
(() => {
    const THEME_KEY = "pdf-preview-dark";
    const tabsEl = document.getElementById("pdf-shell-tabs");
    const panesEl = document.getElementById("pdf-shell-panes");
    const panes = [];
    let current = -1;
    let intentionalClose = false;

    function applyTheme() {
        let dark = false;
        try { dark = localStorage.getItem(THEME_KEY) === "true"; } catch { /* Optional. */ }
        document.documentElement.dataset.pdfDark = String(dark);
    }
    applyTheme();
    // A pane's own keys.js writes this key; the storage event tells every
    // other same-origin document (including this shell) that it changed.
    window.addEventListener("storage", (event) => {
        if (event.key === THEME_KEY) applyTheme();
    });

    function renderTabs() {
        const multiple = panes.length > 1;
        tabsEl.hidden = !multiple;
        tabsEl.innerHTML = "";
        if (!multiple) return;
        panes.forEach((pane, index) => {
            const tab = document.createElement("span");
            tab.className = "pdf-shell-tab" + (index === current ? " is-current" : "");
            tab.textContent = (index + 1) + ": " + pane.name;
            tabsEl.append(tab);
        });
    }

    function switchTo(index) {
        if (index < 0 || index >= panes.length) return;
        current = index;
        panes.forEach((pane, i) => pane.iframe.classList.toggle("is-current", i === index));
        panes[index].iframe.focus();
        renderTabs();
    }

    function addPane(doc) {
        const iframe = document.createElement("iframe");
        iframe.className = "pdf-shell-pane";
        iframe.allow = "clipboard-write";
        iframe.title = doc.name;
        iframe.src = doc.viewerUrl;
        panesEl.append(iframe);
        panes.push({ id: doc.id, name: doc.name, iframe });
        switchTo(panes.length - 1);
    }

    function closeCurrentPane() {
        if (panes.length <= 1) {
            intentionalClose = true;
            window.close();
            return;
        }
        const [removed] = panes.splice(current, 1);
        removed.iframe.remove();
        switchTo(Math.min(current, panes.length - 1));
    }

    // An OS-level close (window chrome, Cmd+Q) hits this with intentionalClose
    // still false, so the browser's own confirmation prompt applies. Our own
    // q-driven window.close() above sets the flag first to skip it.
    window.addEventListener("beforeunload", (event) => {
        if (intentionalClose) return;
        event.preventDefault();
        event.returnValue = "";
    });

    window.addEventListener("message", (event) => {
        if (event.origin !== location.origin) return;
        const data = event.data;
        if (!data || data.type !== "ggpdf-pane") return;
        if (data.action === "close") closeCurrentPane();
        else if (data.action === "next") switchTo((current + 1) % panes.length);
        else if (data.action === "prev") switchTo((current - 1 + panes.length) % panes.length);
        else if (data.action === "goto") switchTo(data.index);
    });

    const token = location.pathname.split("/")[1];
    const source = new EventSource("/" + token + "/events");
    source.onmessage = (event) => {
        const payload = JSON.parse(event.data);
        if (payload.type === "open") payload.docs.forEach(addPane);
    };
})();
