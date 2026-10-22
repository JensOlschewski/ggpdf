const assert = require("node:assert/strict");
const test = require("node:test");

global.window = {};
global.CSS = { highlights: new Map() };
global.Highlight = class { constructor(...ranges) { this.ranges = ranges; } };
global.document = {
    createElement: () => ({ setAttribute() {}, classList: { toggle() {} } }),
    body: { append() {} },
    addEventListener() {},
    querySelectorAll: () => [],
};
require("./visual.js");

function viewer(texts, breaks = []) {
    const handlers = new Map();
    const app = {
        eventBus: { on(event, handler) { handlers.set(event, handler); },
            emit(event) { handlers.get(event)(); } }, pdfDocument: {}, pagesCount: 1,
        pdfViewer: { viewer: { classList: { add() {}, remove() {}, toggle() {} } },
            currentPageNumber: 1, container: { getBoundingClientRect: () => ({
                top: -1, bottom: 500, left: -1, right: 500,
            }) } },
    };
    const visual = new window.PDFPreviewVisualSelection(app);
    const lines = texts.map((text, index) => ({
        characters: [...text].map((value) => ({ text: value,
            startNode: { isConnected: true }, endNode: { isConnected: true }, start: 0, end: 1 })),
        rect: { top: index * 12 + (breaks.includes(index) ? 15 : 0),
            bottom: index * 12 + (breaks.includes(index) ? 15 : 0) + 10, left: 0, right: 100 },
    }));
    visual.models.set(1, { page: 1, lines, element: { querySelectorAll: () => [] } });
    visual.cursor = { page: 1, line: 0, character: 0 };
    visual.active = true;
    return visual;
}

test("sentence text object spans lines and stops before the next sentence", () => {
    const visual = viewer(["First sentence", "continues. Second one."]);
    visual.cursor = { page: 1, line: 1, character: 3 };
    assert.equal(visual.selectedLines(...visual.textObject("s"))
        .map((line) => line.map((character) => character.text).join("")).join("\n"),
    "First sentence\ncontinues.");
    visual.cursor.character = 18;
    assert.equal(visual.selectedLines(...visual.textObject("s"))
        .map((line) => line.map((character) => character.text).join("")).join("\n"),
    "Second one.");
});

test("paragraph text object uses line gap and remains on the cursor's page", () => {
    const visual = viewer(["First line", "still first", "Next paragraph"], [2]);
    visual.cursor.line = 1;
    assert.deepEqual(visual.textObject("p").map(({ line }) => line), [0, 1]);
    visual.cursor.line = 2;
    assert.deepEqual(visual.textObject("p").map(({ line }) => line), [2, 2]);
});

test("yis copies without a mark, flashes the range, and exits", async () => {
    const visual = viewer(["One sentence. Another."]);
    Object.defineProperty(globalThis, "navigator", { configurable: true,
        value: { clipboard: { writeText: async (text) => { assert.equal(text, "One sentence."); } } } });
    visual.range = () => ({ getBoundingClientRect: () => ({}) });
    visual.handleKey({ key: "y" });
    visual.handleKey({ key: "i" });
    visual.handleKey({ key: "s" });
    await visual.pending;
    assert.equal(visual.active, false);
    assert.equal(CSS.highlights.get("pdf-visual-yank").ranges.length, 1);
    visual.clearFlash();
});

test("word motions cross lines and distinguish punctuation", async () => {
    const visual = viewer(["one, two", "three"]);
    visual.range = () => null;
    await visual.move("w", visual.generation);
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [0, 3]);
    await visual.move("w", visual.generation);
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [0, 5]);
    await visual.move("e", visual.generation);
    assert.equal(visual.cursor.character, 7);
    await visual.move("w", visual.generation);
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [1, 0]);
    await visual.move("b", visual.generation);
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [0, 5]);
});

test("counted word motions and WORD motions handle punctuation differently", async () => {
    const visual = viewer(["one,two three", "four-five six"]);
    visual.range = () => null;
    visual.handleKey({ key: "2" });
    visual.handleKey({ key: "w" });
    await visual.pending;
    assert.equal(visual.cursor.character, 4); // two
    visual.handleKey({ key: "B" });
    await visual.pending;
    assert.equal(visual.cursor.character, 0); // one,two
    visual.handleKey({ key: "2" });
    visual.handleKey({ key: "W" });
    await visual.pending;
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [1, 0]);
    visual.handleKey({ key: "E" });
    await visual.pending;
    assert.equal(visual.cursor.character, 8); // four-five
    visual.handleKey({ key: "4" });
    visual.handleKey({ key: "b" });
    await visual.pending;
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [0, 8]); // three
});

test("counted horizontal motions clamp and ^ finds first nonblank", async () => {
    const visual = viewer(["  hello"]);
    visual.range = () => null;
    visual.handleKey({ key: "^" });
    await visual.pending;
    assert.equal(visual.cursor.character, 2);
    visual.handleKey({ key: "9" });
    visual.handleKey({ key: "l" });
    await visual.pending;
    assert.equal(visual.cursor.character, 6);
    visual.handleKey({ key: "3" });
    visual.handleKey({ key: "h" });
    await visual.pending;
    assert.equal(visual.cursor.character, 3);
});

test("v then y in quick succession still copies the marked character", async () => {
    const visual = viewer(["hello"]);
    let copied;
    Object.defineProperty(globalThis, "navigator", { configurable: true,
        value: { clipboard: { writeText: async (text) => { copied = text; } } } });
    visual.range = () => ({});
    visual.handleKey({ key: "v" });
    visual.handleKey({ key: "y" });
    await visual.pending;
    assert.equal(copied, "h");
    assert.equal(visual.active, false);
    visual.clearFlash();
});

test("v then is/ip replaces the mark with the text object for yanking", async () => {
    const visual = viewer(["One. Two.", "Next paragraph."], [1]);
    let copied;
    Object.defineProperty(globalThis, "navigator", { configurable: true,
        value: { clipboard: { writeText: async (text) => { copied = text; } } } });
    visual.range = () => ({});
    visual.cursor.character = 6;
    visual.handleKey({ key: "v" });
    visual.handleKey({ key: "i" });
    visual.handleKey({ key: "s" });
    await visual.pending;
    assert.equal(visual.selectedText(), "Two.");
    visual.handleKey({ key: "i" });
    visual.handleKey({ key: "p" });
    await visual.pending;
    assert.equal(visual.selectedText(), "One. Two.");
    visual.handleKey({ key: "y" });
    await visual.pending;
    assert.equal(copied, "One. Two.");
    visual.clearFlash();
});

test("counted line motions move the cursor and extend a visual selection", async () => {
    const visual = viewer(["abc", "d", "ef", "ghi", "jkl"]);
    visual.range = () => null;
    visual.cursor.character = 2;
    visual.column = 2;
    visual.handleKey({ key: "3" });
    visual.handleKey({ key: "j" });
    await visual.pending;
    assert.deepEqual([visual.cursor.line, visual.cursor.character], [3, 2]);
    visual.handleKey({ key: "v" });
    visual.handleKey({ key: "2" });
    visual.handleKey({ key: "k" });
    await visual.pending;
    assert.deepEqual([visual.anchor.line, visual.cursor.line, visual.cursor.character], [3, 1, 0]);
    visual.handleKey({ key: "0" });
    await visual.pending;
    assert.equal(visual.cursor.character, 0);
});

test("counted jumps cross pages, skip empty pages, and stop at document edges", async () => {
    const visual = viewer(["first", "second"]);
    visual.app.pagesCount = 3;
    visual.models.set(2, { lines: [], element: { querySelectorAll: () => [] } });
    visual.models.set(3, { lines: ["third", "fourth"].map((text) => ({
        characters: [...text].map((value) => ({ text: value })),
    })), element: { querySelectorAll: () => [] } });
    visual.ensurePage = async (page) => visual.models.get(page);
    visual.range = () => null;
    visual.handleKey({ key: "9" });
    visual.handleKey({ key: "j" });
    await visual.pending;
    assert.deepEqual([visual.cursor.page, visual.cursor.line], [3, 1]);
    visual.handleKey({ key: "3" });
    visual.handleKey({ key: "k" });
    await visual.pending;
    assert.deepEqual([visual.cursor.page, visual.cursor.line], [1, 0]);
});

test("line numbers are relative except for the cursor's absolute page line", () => {
    const visual = viewer(["one", "two", "three"]);
    visual.cursor.line = 1;
    assert.deepEqual([0, 1, 2].map((index) => visual.lineNumber(1, index)), [1, 2, 1]);
    visual.models.set(2, { lines: [{}, {}] });
    assert.deepEqual([0, 1].map((index) => visual.lineNumber(2, index)), [2, 3]);
    visual.cursor = { page: 2, line: 0, character: 0 };
    assert.deepEqual([0, 1, 2].map((index) => visual.lineNumber(1, index)), [3, 2, 1]);
});

test("zoom exits cursor mode and clears the marked range", () => {
    const visual = viewer(["abc", "def"]);
    visual.cursor = { page: 1, line: 1, character: 1 };
    visual.anchor = { page: 1, line: 0, character: 2 };
    visual.marking = true;
    visual.app.eventBus.emit("scalechanging");
    assert.equal(visual.active, false);
    assert.equal(visual.anchor, null);
    assert.equal(visual.models.size, 0);
});

test("Escape or v clears a mark and leaves cursor mode active", async () => {
    for (const key of ["Escape", "v"]) {
        const visual = viewer(["abc"]);
        visual.range = () => null;
        visual.handleKey({ key: "v" });
        visual.handleKey({ key });
        await visual.pending;
        assert.equal(visual.active, true);
        assert.equal(visual.anchor, null);
        assert.equal(visual.marking, false);
        assert.match(visual.status.textContent, /^CURSOR ·/);
        visual.handleKey({ key: "Escape" });
        assert.equal(visual.active, false);
    }
});

test("v enters character selection, V enters whole-line selection, c enters cursor mode", async () => {
    global.window.getSelection = () => ({ removeAllRanges() {} });
    for (const [key, expected] of [["v", "a"], ["V", "abc\n"], ["c", ""]]) {
        const visual = viewer(["abc"]);
        const model = visual.models.get(1);
        visual.ensurePage = async () => {
            visual.models.set(1, model);
            return model;
        };
        visual.range = () => null;
        visual.active = false;
        visual.handleKey({ key });
        await visual.pending;
        assert.equal(visual.active, true, visual.status.textContent);
        if (key !== "c") assert.ok(visual.anchor, visual.status.textContent);
        assert.equal(visual.selectedText(), expected);
        assert.equal(visual.linewise, key === "V");
        assert.equal(visual.marking, key !== "c");
        visual.handleKey({ key: "Escape" });
        await visual.pending;
        assert.equal(visual.active, key !== "c");
        if (key !== "c") {
            assert.equal(visual.anchor, null);
            visual.handleKey({ key: "Escape" });
            assert.equal(visual.active, false);
        }
    }
});

test("V selects complete lines, y copies a trailing newline, and v switches back", async () => {
    const visual = viewer(["abc", "def", "ghi"]);
    visual.range = () => ({});
    visual.cursor = { page: 1, line: 1, character: 1 };
    visual.column = 1;
    visual.handleKey({ key: "V" });
    visual.handleKey({ key: "k" });
    await visual.pending;
    assert.equal(visual.selectedText(), "abc\ndef\n");
    visual.handleKey({ key: "v" });
    await visual.pending;
    assert.equal(visual.selectedText(), "bc\nde");
    visual.handleKey({ key: "V" });
    await visual.pending;
    assert.equal(visual.selectedText(), "abc\ndef\n");
    let copied;
    Object.defineProperty(globalThis, "navigator", { configurable: true,
        value: { clipboard: { writeText: async (text) => { copied = text; } } } });
    visual.handleKey({ key: "y" });
    await visual.pending;
    assert.equal(copied, "abc\ndef\n");
    assert.equal(visual.active, false);
    visual.clearFlash();
});

test("c behaves like Escape: clears a mark first, then exits cursor mode", async () => {
    const visual = viewer(["abc"]);
    visual.range = () => null;
    visual.handleKey({ key: "v" });
    visual.handleKey({ key: "c" });
    await visual.pending;
    assert.equal(visual.active, true);
    assert.equal(visual.anchor, null);
    assert.equal(visual.marking, false);
    visual.handleKey({ key: "c" });
    assert.equal(visual.active, false);
});

test("repeating v or V clears its selection without leaving cursor mode", async () => {
    for (const key of ["v", "V"]) {
        const visual = viewer(["abc"]);
        visual.range = () => null;
        visual.handleKey({ key });
        visual.handleKey({ key });
        await visual.pending;
        assert.equal(visual.active, true);
        assert.equal(visual.anchor, null);
        assert.equal(visual.marking, false);
    }
});
