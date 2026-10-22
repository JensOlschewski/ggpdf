#!/usr/bin/python3
"""Open explicitly selected PDFs in a private loopback PDF.js viewer."""

import argparse
from contextlib import contextmanager
import errno
import hashlib
import json
import mimetypes
import ntpath
import os
from pathlib import Path
import queue
import secrets
import shutil
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote, unquote, urlencode, urlsplit
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parent
THEME = ROOT / "theme.css"
WINDOWS = sys.platform == "win32"
CACHE = (Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData/Local")) / "ggpdf"
         if WINDOWS else Path.home() / "Library/Caches/pdf-preview")
PORT = 18765
ORIGIN = "http://[::1]:{}".format(PORT)
HTTP = build_opener(ProxyHandler({}))

# Browsers whose --app=URL flag opens a window without a tab strip or address
# bar. Safari and Firefox have no equivalent, so they fall back to a normal
# window via the default handler.
CHROMIUM_BUNDLE_IDS = {
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.microsoft.edgemac",
    "com.brave.Browser",
    "com.vivaldi.Vivaldi",
    "org.chromium.Chromium",
    "com.operasoftware.Opera",
    "company.thebrowser.Browser",
}
CHROMIUM_WINDOWS_EXES = {
    "chrome.exe", "msedge.exe", "brave.exe", "vivaldi.exe",
    "chromium.exe", "opera.exe",
}


def cache_directory():
    CACHE.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not WINDOWS:
        CACHE.chmod(0o700)


@contextmanager
def server_lock():
    with (CACHE / "server.lock").open("a+b") as lock:
        if WINDOWS:
            import msvcrt
            lock.seek(0)
            try:
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError as error:
                if error.errno == errno.EACCES:
                    yield False
                    return
                raise
            try:
                yield True
            finally:
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                yield False
                return
            yield True


def request(token, route, data=None):
    body = None if data is None else json.dumps(data).encode()
    req = Request(ORIGIN + "/" + token + route, data=body,
                  headers={"Content-Type": "application/json"})
    with HTTP.open(req, timeout=2) as response:
        return json.load(response)


class Server(ThreadingHTTPServer):
    address_family = socket.AF_INET6
    daemon_threads = True


def register(server, paths):
    """Validate PDFs, record them, and describe them for an /open response."""
    docs = []
    for raw in paths:
        path = Path(raw).resolve(strict=True)
        if not path.is_file():
            raise ValueError("Not a regular file")
        with path.open("rb") as source:
            if b"%PDF-" not in source.read(1024):
                raise ValueError("The selected file is not a PDF")
        identifier = hashlib.sha256(os.fsencode(path)).hexdigest()
        server.documents[identifier] = path
        file_url = "/{}/document/{}/{}".format(
            server.token, identifier, quote(path.name, safe=""))
        viewer_url = ORIGIN + "/{}/vendor/web/viewer.html?{}".format(
            server.token, urlencode({"file": file_url}))
        docs.append({"id": identifier, "name": path.name, "viewerUrl": viewer_url})
    return docs


class Handler(BaseHTTPRequestHandler):
    # No directory listing, arbitrary-path endpoint, or cross-origin access.
    def log_message(self, format, *args):
        pass  # Document capability URLs must not appear in access logs.

    def route(self):
        if self.headers.get("Host") != "[::1]:{}".format(PORT):
            self.send_error(403)
            return None
        if self.headers.get("Origin", ORIGIN) != ORIGIN:
            self.send_error(403)
            return None
        path = unquote(urlsplit(self.path).path)
        prefix = "/" + self.server.token + "/"
        if not path.startswith(prefix):
            self.send_error(404)
            return None
        return path[len(prefix):]

    def end_headers(self):
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("Cache-Control", getattr(self, "cache_control", "no-store"))
        # viewer.html opts into same-origin framing so shell.html can host it as a pane.
        self.send_header("Content-Security-Policy",
                          "frame-ancestors " + getattr(self, "frame_ancestors", "'none'"))
        super().end_headers()

    def respond(self, data, content_type="application/json", status=200):
        if isinstance(data, dict):
            data = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def do_POST(self):
        route = self.route()
        if route is None:
            return
        if route != "open":
            self.send_error(404)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 65536:
                raise ValueError("Invalid request size")
            data = json.loads(self.rfile.read(length))
            paths = data["paths"]
            if not isinstance(paths, list) or not paths:
                raise ValueError("No files provided")
            docs = register(self.server, paths)
            # An already-open ggpdf window gets the documents pushed to it as new
            # panes; otherwise they wait as "pending" for the window this request
            # causes the caller to open.
            with self.server.lock:
                session = self.server.session
                if session and session["alive"]:
                    session["queue"].put_nowait({"type": "open", "docs": docs})
                else:
                    session = None
                    self.server.pending.extend(docs)
            if session:
                self.respond({"attached": True})
            else:
                self.respond({"url": ORIGIN + "/{}/shell.html".format(self.server.token)})
        except (OSError, ValueError, KeyError, TypeError) as error:
            self.respond({"error": str(error)}, status=400)

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        route = self.route()
        if route is None:
            return
        if route == "health":
            self.respond({"service": "pdf-preview"})
            return
        if route == "events":
            self.serve_events()
            return
        if route.startswith("document/"):
            parts = route.split("/", 2)
            path = self.server.documents.get(parts[1])
            if path is None:
                self.send_error(404)
                return
            content_type = "application/pdf"
        else:
            path = (ROOT / route).resolve()
            vendor = (ROOT / "vendor").resolve()
            if not (vendor in path.parents or path in (
                    ROOT / "keys.js", ROOT / "visual.js", ROOT / "style.css", THEME,
                    ROOT / "shell.html", ROOT / "shell.js", ROOT / "shell.css")):
                self.send_error(404)
                return
            content_type = mimetypes.guess_type(str(path))[0] or "application/octet-stream"
            if path.suffix in (".mjs", ".js"):
                content_type = "text/javascript"
        try:
            if path == ROOT / "vendor/web/viewer.html":
                html = path.read_text().replace("</head>",
                    '<link rel="icon" href="data:,">'
                    '<link rel="stylesheet" href="../../theme.css">'
                    '<link rel="stylesheet" href="../../style.css">'
                    '<script src="../../visual.js"></script>'
                    '<script src="../../keys.js"></script></head>')
                # Prevent PDF-driven requests to remote hosts; assets are local.
                html = html.replace("connect-src * blob: data:", "connect-src 'self' blob: data:")
                # shell.html hosts this document in a same-origin pane iframe.
                self.frame_ancestors = "'self'"
                self.respond(html.encode(), "text/html; charset=utf-8")
                return
            # Stream PDFs instead of retaining whole documents in server memory.
            with path.open("rb") as source:
                stat = os.fstat(source.fileno())
                if not route.startswith("document/"):
                    etag = '"{:x}-{:x}"'.format(stat.st_mtime_ns, stat.st_size)
                    self.cache_control = "private, max-age=0, must-revalidate"
                    if self.headers.get("If-None-Match") == etag:
                        self.send_response(304)
                        self.send_header("ETag", etag)
                        self.end_headers()
                        return
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(stat.st_size))
                if not route.startswith("document/"):
                    self.send_header("ETag", etag)
                self.end_headers()
                if self.command != "HEAD":
                    shutil.copyfileobj(source, self.wfile)
        except (FileNotFoundError, IsADirectoryError, PermissionError):
            self.send_error(404)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def serve_events(self):
        """Hold one long-lived connection: the window that gets pushed new panes.

        Becoming the session also claims any documents that were waiting for a
        window (this connection's window is the one that request caused to open).
        A later connection replaces the previous session outright; only one
        window receives pushes at a time.
        """
        session = {"queue": queue.Queue(), "alive": True}
        with self.server.lock:
            previous = self.server.session
            self.server.session = session
            pending, self.server.pending = self.server.pending, []
        if previous:
            previous["alive"] = False
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        if pending:
            session["queue"].put_nowait({"type": "open", "docs": pending})
        try:
            while session["alive"]:
                try:
                    item = session["queue"].get(timeout=15)
                except queue.Empty:
                    self.wfile.write(b": keepalive\n\n")
                    self.wfile.flush()
                    continue
                self.wfile.write(b"data: " + json.dumps(item).encode() + b"\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            session["alive"] = False
            with self.server.lock:
                if self.server.session is session:
                    self.server.session = None


def serve():
    cache_directory()
    # Held for the entire process: concurrent launches cannot create two servers.
    with server_lock() as acquired:
        if not acquired:
            return
        token_path = CACHE / "token"
        if token_path.exists():
            token = token_path.read_text().strip()
        else:
            token = secrets.token_urlsafe(32)
            token_path.write_text(token)
            if not WINDOWS:
                token_path.chmod(0o600)
        with Server(("::1", PORT), Handler) as server:
            server.token = token
            server.documents = {}
            server.session = None
            server.pending = []
            server.lock = threading.Lock()
            (CACHE / "server.pid").write_text(str(os.getpid()))
            server.serve_forever()


def ensure_server():
    cache_directory()
    token_path = CACHE / "token"

    def running():
        try:
            token = token_path.read_text().strip()
            if request(token, "/health").get("service") == "pdf-preview":
                return token
        except (OSError, ValueError):
            pass
        return None

    token = running()
    if token:
        return token
    with (CACHE / "server.log").open("ab") as log:
        flags = subprocess.CREATE_NO_WINDOW if WINDOWS else 0
        subprocess.Popen([sys.executable, str(ROOT / "preview.py"), "--serve"],
                          stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                          start_new_session=not WINDOWS, creationflags=flags)
    for _ in range(50):
        time.sleep(0.1)
        token = running()
        if token:
            return token
    raise RuntimeError("ggpdf could not start. See " + str(CACHE / "server.log"))


def default_browser_bundle_id():
    script = (
        'ObjC.import("AppKit");'
        'var url = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL('
        '$.NSURL.URLWithString("http://example.com"));'
        '$.NSBundle.bundleWithURL(url).bundleIdentifier.js'
    )
    result = subprocess.run(["/usr/bin/osascript", "-l", "JavaScript", "-e", script],
                             capture_output=True, text=True, check=True)
    return result.stdout.strip()


def default_windows_browser():
    import ctypes
    from ctypes import wintypes

    query = ctypes.windll.shlwapi.AssocQueryStringW
    query.argtypes = (wintypes.DWORD, wintypes.DWORD, wintypes.LPCWSTR,
                      wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD))
    query.restype = wintypes.LONG
    size = wintypes.DWORD()
    # ASSOCSTR_EXECUTABLE for the default HTTP handler.
    if query(0, 2, "http", "open", None, ctypes.byref(size)) != 1 or not size.value:
        return None
    result = ctypes.create_unicode_buffer(size.value)
    if query(0, 2, "http", "open", result, ctypes.byref(size)) != 0:
        return None
    return result.value


def open_browser(url):
    if WINDOWS:
        browser = default_windows_browser()
        if browser and ntpath.basename(browser).lower() in CHROMIUM_WINDOWS_EXES:
            try:
                # A running Chromium instance ignores new window-state flags.
                # A dedicated profile keeps these flags on its browser process.
                profile = CACHE / ("browser-" + ntpath.basename(browser).lower())
                subprocess.Popen([browser, "--user-data-dir=" + str(profile),
                                  "--no-first-run", "--no-default-browser-check",
                                  "--start-maximized", "--app=" + url])
                return
            except OSError:
                pass
        os.startfile(url)
        return
    try:
        bundle_id = default_browser_bundle_id()
    except (subprocess.CalledProcessError, OSError):
        bundle_id = None
    if bundle_id in CHROMIUM_BUNDLE_IDS:
        subprocess.run(["/usr/bin/open", "-n", "-b", bundle_id, "--args", "--app=" + url], check=True)
    else:
        subprocess.run(["/usr/bin/open", url], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serve", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--print-url", action="store_true", help="Print the viewer URL instead of opening a browser")
    parser.add_argument("files", nargs="*", type=Path)
    args = parser.parse_args()
    if args.serve:
        serve()
        return
    if not args.files:
        parser.error("Provide one or more PDF files")
    token = ensure_server()
    paths = [str(path.resolve(strict=True)) for path in args.files]
    result = request(token, "/open", {"paths": paths})
    if "error" in result:
        raise ValueError(result["error"])
    if result.get("attached"):
        return  # Pushed as new panes into the already-open ggpdf window.
    if args.print_url:
        print(result["url"])
    else:
        open_browser(result["url"])


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        message = "ggpdf: " + str(error)
        if WINDOWS and not any(arg == "--serve" for arg in sys.argv[1:]) and sys.executable.lower().endswith("pythonw.exe"):
            import ctypes
            ctypes.windll.user32.MessageBoxW(None, message, "ggpdf", 0x10)
        else:
            print(message, file=sys.stderr)
        sys.exit(1)
