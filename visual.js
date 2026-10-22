/* Visual selection over PDF.js's text layer, in its extracted reading order. */
window.PDFPreviewVisualSelection = class {
    constructor(app) {
        this.app = app;
        this.active = false;
        this.generation = 0;
        this.count = "";
        this.models = new Map();
        this.readyPages = new Set();
        this.segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
        this.sentences = new Intl.Segmenter(undefined, { granularity: "sentence" });
        this.pending = Promise.resolve();
        this.status = document.createElement("div");
        this.status.className = "pdf-visual-status";
        this.status.setAttribute("role", "status");
        this.status.setAttribute("aria-live", "polite");
        this.status.hidden = true;
        document.body.append(this.status);

        app.eventBus.on("textlayerrendered", ({ pageNumber, error }) => {
            if (error) return;
            this.readyPages.add(pageNumber);
            if (this.active) {
                this.collect(pageNumber);
                this.paint();
            }
        });
        for (const event of ["scalechanging", "rotationchanging", "pagesdestroy"]) {
            app.eventBus.on(event, () => {
                if (this.active) this.exit();
                this.clearFlash();
                this.models.clear();
                this.readyPages.clear();
                if (event === "pagesdestroy") {
                    this.savedCursor = this.savedDocument = null;
                }
            });
        }
        document.addEventListener("copy", (event) => {
            if (!this.active || !this.anchor ||
                document.activeElement?.matches("input, textarea, select") ||
                document.activeElement?.isContentEditable) return;
            event.clipboardData.setData("text/plain", this.selectedText());
            event.preventDefault();
        });
    }

    message(text, temporary = false) {
        clearTimeout(this.statusTimer);
        this.status.textContent = text;
        this.status.hidden = !text;
        this.status.classList.toggle("is-notice", temporary);
        if (temporary) {
            this.statusTimer = setTimeout(() => {
                if (!this.active) this.status.hidden = true;
            }, 3000);
        }
    }

    handleKey(event) {
        if (event.ctrlKey || event.metaKey || event.altKey) return false;
        if (!this.active && ["c", "v", "V"].includes(event.key)) {
            this.enter(event.key);
            return true;
        }
        if (!this.active) return false;
        if (event.key === "Escape" || event.key === "c") {
            if (this.marking) this.stopMark();
            else this.exit();
            return true;
        }
        if (this.command) {
            const command = this.command + event.key;
            this.command = null;
            if (command === "yi") {
                this.command = command;
                return true;
            }
            if (command === "is" || command === "ip") {
                this.queue(() => this.selectObject(command.at(-1)));
                return true;
            }
            if (command === "yis" || command === "yip") {
                this.queue(() => this.yankObject(command.at(-1)));
                return true;
            }
            // An incomplete text object is cancelled; handle the new key normally.
        }
        if (/^[1-9]$/.test(event.key) || (this.count && event.key === "0")) {
            this.count = (this.count + event.key).slice(0, 6);
            return true;
        }
        const count = this.count ? Number(this.count) : 1;
        this.count = "";
        if (event.key === "y" && !this.marking) {
            this.command = "y";
            return true;
        }
        if (event.key === "i" && this.marking) {
            this.command = "i";
            return true;
        }
        if (!["h", "j", "k", "l", "w", "b", "e", "W", "B", "E", "0", "^", "$", "y", "v", "V"].includes(event.key)) {
            // Do not let normal-mode j/k/gg/G scroll a stale selection.
            if (event.key === "q" || event.key === "i") return false;
            return true;
        }
        if (event.key === "v" || event.key === "V") {
            const linewise = event.key === "V";
            const deselect = this.marking && this.linewise === linewise;
            this.marking = !deselect;
            this.linewise = !deselect && linewise;
            this.queue(() => {
                this.anchor = deselect ? null : this.anchor ?? { ...this.cursor };
                this.paint();
            });
            return true;
        }
        this.queue(async (generation) => {
            if (event.key === "y") await this.yank();
            else await this.move(event.key, generation, count);
        });
        return true;
    }

    queue(action) {
        const generation = this.generation;
        this.pending = this.pending.then(async () => {
            if (this.active && generation === this.generation && this.cursor) await action(generation);
        }).catch((error) => {
            if (this.active && generation === this.generation) {
                this.message("VISUAL · " + error.message + " · Esc to cancel", true);
            }
        });
    }

    stopMark() {
        this.marking = false;
        this.linewise = false;
        this.command = null;
        this.count = "";
        this.queue(() => {
            this.anchor = null;
            this.paint();
        });
    }

    async enter(mode = "c") {
        this.clearFlash();
        const generation = ++this.generation;
        this.active = true;
        this.command = null;
        this.count = "";
        this.marking = mode !== "c";
        this.linewise = mode === "V";
        this.anchor = this.cursor = null;
        this.models.clear();
        this.app.pdfViewer.viewer.classList.add("pdf-visual-mode");
        this.message("CURSOR · Loading text…");
        this.pending = (async () => {
            try {
                await this.ensurePage(this.app.pdfViewer.currentPageNumber);
                if (!this.active || generation !== this.generation) return;
                for (const page of this.readyPages) this.collect(page);
                const viewport = this.app.pdfViewer.container.getBoundingClientRect();
                const candidates = [...this.models.values()].sort((a, b) => a.page - b.page);
                let start;
                for (const model of candidates) {
                    const line = model.lines.findIndex(({ rect }) =>
                        rect.bottom > viewport.top && rect.top < viewport.bottom &&
                        rect.right > viewport.left && rect.left < viewport.right);
                    if (line >= 0) {
                        start = { page: model.page, line, character: 0 };
                        break;
                    }
                }
                // Re-enter at the previous cursor if it is still in view.
                const saved = this.savedDocument === this.app.pdfDocument && this.savedCursor;
                const savedLine = saved && this.models.get(saved.page)?.lines[saved.line];
                if (savedLine && savedLine.rect.bottom > viewport.top && savedLine.rect.top < viewport.bottom) {
                    start = { ...saved, character: Math.min(saved.character, savedLine.characters.length - 1) };
                }
                if (!start) {
                    this.exit();
                    this.message("No selectable text in view. Scanned PDFs need OCR.", true);
                    return;
                }
                window.getSelection()?.removeAllRanges();
                this.cursor = { ...start };
                if (this.marking) this.anchor = { ...start };
                this.column = start.character;
                this.paint();
            } catch (error) {
                if (generation !== this.generation) return;
                this.exit();
                this.message("Cannot start visual mode: " + error.message, true);
            }
        })();
    }

    exit() {
        if (this.cursor) {
            this.savedCursor = { ...this.cursor };
            this.savedDocument = this.app.pdfDocument;
        }
        this.active = false;
        this.command = null;
        this.count = "";
        this.marking = false;
        this.linewise = false;
        ++this.generation;
        this.anchor = this.cursor = null;
        this.app.pdfViewer.viewer.classList.remove("pdf-visual-mode");
        this.app.pdfViewer.viewer.classList.remove("pdf-visual-marking");
        document.querySelectorAll(".pdf-visual-gutter").forEach((element) => element.remove());
        CSS.highlights.delete("pdf-visual-selection");
        CSS.highlights.delete("pdf-visual-cursor");
        this.models.clear();
        this.message("");
    }

    async ensurePage(page) {
        const view = this.app.pdfViewer.getPageView(page - 1);
        if (this.readyPages.has(page) && view.textLayer?.div.isConnected &&
            !view.textLayer.div.hidden) {
            return this.collect(page);
        }
        await new Promise((resolve, reject) => {
            const done = ({ pageNumber, error }) => {
                if (pageNumber !== page) return;
                cleanup();
                if (error) reject(new Error("This page's text could not be loaded"));
                else resolve();
            };
            const cleanup = () => {
                clearTimeout(timer);
                this.app.eventBus.off("textlayerrendered", done);
            };
            const timer = setTimeout(() => {
                cleanup();
                reject(new Error("Timed out loading this page's text"));
            }, 10000);
            this.app.eventBus.on("textlayerrendered", done);
            this.app.pdfViewer.scrollPageIntoView({ pageNumber: page });
        });
        return this.collect(page);
    }

    collect(page) {
        const view = this.app.pdfViewer.getPageView(page - 1);
        const layer = view?.textLayer?.div;
        if (!layer?.isConnected || layer.hidden) return this.models.get(page);
        const model = { page, element: view.div, lines: [] };
        let line = null;
        const finish = () => {
            if (line?.characters.some(({ text }) => /\S/u.test(text))) model.lines.push(line);
            line = null;
        };
        const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
        while (walker.nextNode()) {
            const node = walker.currentNode;
            if (node.nodeName === "BR") {
                finish();
                continue;
            }
            if (node.nodeType !== Node.TEXT_NODE || !node.textContent) continue;
            const range = document.createRange();
            range.selectNodeContents(node);
            const rect = range.getBoundingClientRect();
            if (!rect.height || !rect.width) continue;
            // PDF.js supplies BRs for explicit end-of-line markers. Geometry
            // handles PDFs whose extraction omits those markers.
            if (line && (rect.top >= line.rect.bottom || rect.bottom <= line.rect.top ||
                rect.left < line.lastRect.left - 2)) finish();
            if (!line) {
                line = { characters: [], rect: {
                    top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right,
                }, lastRect: rect };
            } else {
                const gap = rect.left - line.lastRect.right;
                const previous = line.characters.at(-1);
                if (previous && !/\s$/u.test(previous.text) && !/^\s/u.test(node.textContent) &&
                    gap > Math.min(rect.height, line.lastRect.height) * 0.15) {
                    line.characters.push({ text: " ", startNode: previous.endNode,
                        start: previous.end, endNode: node, end: 0 });
                }
                line.rect.top = Math.min(line.rect.top, rect.top);
                line.rect.bottom = Math.max(line.rect.bottom, rect.bottom);
                line.rect.left = Math.min(line.rect.left, rect.left);
                line.rect.right = Math.max(line.rect.right, rect.right);
            }
            for (const { segment, index } of this.segmenter.segment(node.textContent)) {
                line.characters.push({ text: segment, startNode: node, start: index,
                    endNode: node, end: index + segment.length });
            }
            line.lastRect = rect;
        }
        finish();
        this.models.set(page, model);
        if (this.active) this.gutter(model);
        return model;
    }

    gutter(model) {
        model.element.querySelector(".pdf-visual-gutter")?.remove();
        const gutter = document.createElement("div");
        gutter.className = "pdf-visual-gutter";
        gutter.setAttribute("aria-hidden", "true");
        const pageRect = model.element.getBoundingClientRect();
        const scale = pageRect.height / model.element.offsetHeight;
        model.lines.forEach((line, index) => {
            const label = document.createElement("span");
            label.className = "pdf-visual-line";
            label.textContent = String(index + 1);
            label.style.top = (line.rect.top - pageRect.top) / scale + "px";
            label.style.height = (line.rect.bottom - line.rect.top) / scale + "px";
            // Two-column pages have distinct text lines at the same height.
            // Put the later column's numbers immediately left of its text,
            // rather than printing both columns' numbers over one another.
            const hasLeftColumn = model.lines.some((other) =>
                other !== line && other.rect.right < line.rect.left - 6 &&
                Math.min(other.rect.bottom, line.rect.bottom) -
                    Math.max(other.rect.top, line.rect.top) >
                    Math.min(other.rect.bottom - other.rect.top,
                        line.rect.bottom - line.rect.top) * 0.25);
            if (hasLeftColumn) {
                label.style.transform = `translateX(${(line.rect.left - pageRect.left) / scale + 5}px)`;
            }
            gutter.append(label);
        });
        model.element.append(gutter);
    }

    lineNumber(page, index) {
        if (!this.cursor || page === this.cursor.page) {
            return page === this.cursor?.page && index !== this.cursor.line
                ? Math.abs(index - this.cursor.line) : index + 1;
        }
        const direction = page < this.cursor.page ? -1 : 1;
        let distance = direction < 0 ? this.cursor.line + 1
            : this.models.get(this.cursor.page).lines.length - this.cursor.line;
        for (let between = this.cursor.page + direction; between !== page; between += direction) {
            const model = this.models.get(between);
            if (!model) return index + 1; // Unrendered pages have unknown line counts.
            distance += model.lines.length;
        }
        const model = this.models.get(page);
        return distance + (direction < 0 ? model.lines.length - 1 - index : index);
    }

    bounds() {
        const compare = (a, b) => a.page - b.page || a.line - b.line || a.character - b.character;
        return compare(this.anchor, this.cursor) <= 0
            ? [this.anchor, this.cursor] : [this.cursor, this.anchor];
    }

    selectedLines(start, end) {
        if (!start || !end) {
            if (!this.anchor || !this.cursor) return [];
            [start, end] = this.bounds();
        }
        const result = [];
        for (let page = start.page; page <= end.page; page++) {
            const model = this.models.get(page);
            if (!model) continue;
            const first = page === start.page ? start.line : 0;
            const last = page === end.page ? end.line : model.lines.length - 1;
            for (let index = first; index <= last; index++) {
                const line = model.lines[index];
                if (!line) continue;
                const from = !this.linewise && page === start.page && index === start.line ? start.character : 0;
                const to = !this.linewise && page === end.page && index === end.line ? end.character + 1 : line.characters.length;
                result.push(line.characters.slice(from, to));
            }
        }
        return result;
    }

    selectedText() {
        return this.selectedLines().map((line) => line.map(({ text }) => text).join(""))
            .join("\n").replaceAll("\u0000", "") + (this.linewise && this.anchor ? "\n" : "");
    }

    range(characters) {
        const first = characters[0];
        const last = characters.at(-1);
        if (!first?.startNode.isConnected || !last?.endNode.isConnected) return null;
        const range = document.createRange();
        range.setStart(first.startNode, first.start);
        range.setEnd(last.endNode, last.end);
        return range;
    }

    clearFlash() {
        clearTimeout(this.flashTimer);
        CSS.highlights.delete("pdf-visual-yank");
    }

    textObject(kind) {
        const { page, line: lineIndex, character: characterIndex } = this.cursor;
        const lines = this.models.get(page).lines;
        const position = (line, character) => ({ page, line, character });
        if (kind === "p") {
            const startsParagraph = (index) => {
                if (index === 0) return true;
                const previous = lines[index - 1].rect;
                const current = lines[index].rect;
                const height = previous.bottom - previous.top;
                return current.top < previous.top || current.top - previous.bottom > height * 0.6 ||
                    current.left - previous.left > height * 0.6;
            };
            let first = lineIndex;
            let last = lineIndex;
            while (first > 0 && !startsParagraph(first)) first--;
            while (last + 1 < lines.length && !startsParagraph(last + 1)) last++;
            return [position(first, 0), position(last, lines[last].characters.length - 1)];
        }

        // Join visual lines with spaces for sentence detection, retaining the
        // character positions needed to map the result back onto the text layer.
        let text = "";
        const entries = [];
        let cursorOffset = 0;
        lines.forEach((line, index) => {
            if (index) text += " ";
            line.characters.forEach(({ text: value }, character) => {
                if (index === lineIndex && character === characterIndex) cursorOffset = text.length;
                entries.push({ start: text.length, end: text.length + value.length,
                    position: position(index, character), value });
                text += value;
            });
        });
        const segments = [...this.sentences.segment(text)];
        const sentence = segments.find(({ index, segment }) =>
            cursorOffset >= index && cursorOffset < index + segment.length) ?? segments.at(-1);
        if (!sentence) return null;
        const members = entries.filter(({ start, end, value }) =>
            start < sentence.index + sentence.segment.length && end > sentence.index && /\S/u.test(value));
        if (!members.length) return null;
        return [members[0].position, members.at(-1).position];
    }

    async yankObject(kind) {
        const bounds = this.textObject(kind);
        if (bounds) await this.yank(bounds);
    }

    selectObject(kind) {
        const bounds = this.textObject(kind);
        if (!bounds) return;
        [this.anchor, this.cursor] = bounds;
        this.linewise = false;
        this.column = this.cursor.character;
        this.paint();
    }

    paint() {
        if (!this.active || !this.cursor) return;
        this.app.pdfViewer.viewer.classList.toggle("pdf-visual-marking", Boolean(this.anchor));
        const ranges = this.selectedLines().map((line) => this.range(line)).filter(Boolean);
        CSS.highlights.set("pdf-visual-selection", new Highlight(...ranges));
        const model = this.models.get(this.cursor.page);
        const character = model?.lines[this.cursor.line]?.characters[this.cursor.character];
        const cursorRange = character && this.range([character]);
        CSS.highlights.set("pdf-visual-cursor", new Highlight(...(cursorRange ? [cursorRange] : [])));
        for (const [page, pageModel] of this.models) {
            pageModel.element.querySelectorAll(".pdf-visual-line").forEach((label, index) => {
                const current = page === this.cursor.page && index === this.cursor.line;
                label.textContent = String(this.lineNumber(page, index));
                label.classList.toggle("is-current", current);
            });
        }
        const mode = this.anchor ? this.linewise ? "VISUAL LINE" : "VISUAL" : "CURSOR";
        const help = this.anchor ? "h/j/k/l/w/b/e/W/B/E select · counts · 0/^/$ · v/V switch · is/ip · y copy" : "h/j/k/l/w/b/e/W/B/E move · counts · 0/^/$ · yis/yip · v/V mark";
        this.message(`${mode} · Page ${this.cursor.page} · Line ${this.cursor.line + 1} · ${help} · Esc ${this.anchor ? "unmark" : "exit"}`);
    }

    async move(key, generation, count = 1) {
        const next = { ...this.cursor };
        let model = this.models.get(next.page);
        let line = model.lines[next.line];
        if ("wbeWBE".includes(key)) {
            // Lowercase motions treat punctuation as a word; uppercase motions
            // treat everything between whitespace as one WORD.
            const bigWord = key !== key.toLowerCase();
            const motion = key.toLowerCase();
            const positions = model.lines.flatMap((row, index) => row.characters.map((entry, character) => ({
                line: index, character, type: /\s/u.test(entry.text) ? 0 :
                    bigWord || /[\p{L}\p{N}_]/u.test(entry.text) ? 1 : 2,
            })));
            const current = positions.findIndex((item) =>
                item.line === next.line && item.character === next.character);
            let index = current;
            for (let step = 0; step < count; step++) {
                const before = index;
                if (motion === "w") {
                    const type = positions[index].type;
                    if (type) while (index + 1 < positions.length &&
                        positions[index + 1].line === positions[index].line &&
                        positions[index + 1].type === type) index++;
                    index++;
                    while (index < positions.length && positions[index].type === 0) index++;
                    if (index === positions.length) index = before;
                } else if (motion === "b") {
                    index = Math.max(0, index - 1);
                    while (index > 0 && positions[index].type === 0) index--;
                    const type = positions[index].type;
                    while (index > 0 && positions[index - 1].line === positions[index].line &&
                        positions[index - 1].type === type) index--;
                } else {
                    const type = positions[index].type;
                    if (!type) {
                        while (index + 1 < positions.length && positions[index].type === 0) index++;
                    } else if (index + 1 < positions.length &&
                        (positions[index + 1].type !== type || positions[index + 1].line !== positions[index].line)) {
                        index++;
                        while (index + 1 < positions.length && positions[index].type === 0) index++;
                    }
                    const word = positions[index].type;
                    while (index + 1 < positions.length && positions[index + 1].line === positions[index].line &&
                        positions[index + 1].type === word) index++;
                }
                if (index === before) break;
            }
            next.line = positions[index].line;
            next.character = positions[index].character;
            line = model.lines[next.line];
            this.column = next.character;
        } else if (key === "j" || key === "k") {
            const direction = key === "j" ? 1 : -1;
            let remaining = count;
            while (remaining) {
                const available = direction > 0
                    ? model.lines.length - 1 - next.line : next.line;
                const step = Math.min(remaining, available);
                next.line += direction * step;
                remaining -= step;
                if (!remaining || next.page + direction < 1 ||
                    next.page + direction > this.app.pagesCount) break;
                let page = next.page + direction;
                while (page >= 1 && page <= this.app.pagesCount) {
                    const nextModel = await this.ensurePage(page);
                    if (!this.active || generation !== this.generation) return;
                    if (nextModel?.lines.length) {
                        next.page = page;
                        model = nextModel;
                        next.line = direction > 0 ? 0 : model.lines.length - 1;
                        remaining--;
                        break;
                    }
                    page += direction;
                }
                if (page < 1 || page > this.app.pagesCount) break;
            }
            line = model.lines[next.line];
            next.character = Math.min(this.column, line.characters.length - 1);
        } else {
            if (key === "h") next.character -= count;
            if (key === "l") next.character += count;
            if (key === "0") next.character = 0;
            if (key === "^") next.character = Math.max(0,
                line.characters.findIndex(({ text }) => /\S/u.test(text)));
            if (key === "$") next.character = line.characters.length - 1;
            next.character = Math.max(0, Math.min(next.character, line.characters.length - 1));
            this.column = next.character;
        }
        this.cursor = next;
        this.paint();
        const character = line.characters[next.character];
        const range = this.range([character]);
        if (range) {
            const rect = range.getBoundingClientRect();
            const target = this.app.pdfViewer.container;
            const viewport = target.getBoundingClientRect();
            if (rect.top < viewport.top + 12) target.scrollBy({ top: rect.top - viewport.top - 12 });
            if (rect.bottom > viewport.bottom - 12) target.scrollBy({ top: rect.bottom - viewport.bottom + 12 });
            if (rect.left < viewport.left + 48) target.scrollBy({ left: rect.left - viewport.left - 48 });
            if (rect.right > viewport.right - 12) target.scrollBy({ left: rect.right - viewport.right + 12 });
        }
    }

    async yank(bounds = this.bounds()) {
        const lines = this.selectedLines(...bounds);
        const text = lines.map((line) => line.map(({ text }) => text).join(""))
            .join("\n").replaceAll("\u0000", "") + (this.linewise && this.anchor ? "\n" : "");
        const generation = this.generation;
        try {
            await navigator.clipboard.writeText(text);
            if (generation !== this.generation) return;
            const ranges = lines.map((line) => this.range(line)).filter(Boolean);
            this.exit();
            this.clearFlash();
            CSS.highlights.set("pdf-visual-yank", new Highlight(...ranges));
            this.flashTimer = setTimeout(() => this.clearFlash(), 160);
            this.message("Copied selection to clipboard", true);
        } catch {
            if (generation === this.generation) {
                this.message("VISUAL · Clipboard unavailable. Press Cmd+c to copy, or Esc to cancel.", true);
            }
        }
    }
};
