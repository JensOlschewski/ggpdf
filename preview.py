#!/usr/bin/python3
"""Open explicitly selected PDFs in a private loopback PDF.js viewer."""

import argparse
import fcntl
import hashlib
import json
import mimetypes
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote, unquote, urlencode, urlsplit
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parent
THEME = ROOT / "theme.css"
CACHE = Path.home() / "Library/Caches/pdf-preview"
PORT = 18765
ORIGIN = "http://[::1]:{}".format(PORT)
HTTP = build_opener(ProxyHandler({}))


def cache_directory():
    CACHE.mkdir(mode=0o700, parents=True, exist_ok=True)
    CACHE.chmod(0o700)


def request(token, route, data=None):
    body = None if data is None else json.dumps(data).encode()
    req = Request(ORIGIN + "/" + token + route, data=body,
                  headers={"Content-Type": "application/json"})
    with HTTP.open(req, timeout=2) as response:
        return json.load(response)


class Server(ThreadingHTTPServer):
    address_family = socket.AF_INET6
    daemon_threads = True


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
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Security-Policy", "frame-ancestors 'none'")
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
            path = Path(data["path"]).resolve(strict=True)
            if not path.is_file():
                raise ValueError("Not a regular file")
            with path.open("rb") as source:
                if b"%PDF-" not in source.read(1024):
                    raise ValueError("The selected file is not a PDF")
            identifier = hashlib.sha256(os.fsencode(path)).hexdigest()
            self.server.documents[identifier] = path
            file_url = "/{}/document/{}/{}".format(
                self.server.token, identifier, quote(path.name, safe=""))
            url = ORIGIN + "/{}/vendor/web/viewer.html?{}".format(
                self.server.token, urlencode({"file": file_url}))
            self.respond({"url": url})
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
            if not (vendor in path.parents or path in (ROOT / "keys.js", ROOT / "visual.js", ROOT / "style.css", THEME)):
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
                self.respond(html.encode(), "text/html; charset=utf-8")
                return
            # Stream PDFs instead of retaining whole documents in server memory.
            with path.open("rb") as source:
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(os.fstat(source.fileno()).st_size))
                self.end_headers()
                if self.command != "HEAD":
                    shutil.copyfileobj(source, self.wfile)
        except (FileNotFoundError, IsADirectoryError, PermissionError):
            self.send_error(404)
        except (BrokenPipeError, ConnectionResetError):
            pass


def serve():
    cache_directory()
    # Held for the entire process: concurrent launches cannot create two servers.
    with (CACHE / "server.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        token_path = CACHE / "token"
        if token_path.exists():
            token = token_path.read_text().strip()
        else:
            token = secrets.token_urlsafe(32)
            token_path.write_text(token)
            token_path.chmod(0o600)
        with Server(("::1", PORT), Handler) as server:
            server.token = token
            server.documents = {}
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
        subprocess.Popen([sys.executable, str(ROOT / "preview.py"), "--serve"],
                         stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                         start_new_session=True)
    for _ in range(50):
        time.sleep(0.1)
        token = running()
        if token:
            return token
    raise RuntimeError("PDF Preview could not start. See " + str(CACHE / "server.log"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serve", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--print-url", action="store_true", help="Print the viewer URL instead of opening Chrome")
    parser.add_argument("files", nargs="*", type=Path)
    args = parser.parse_args()
    if args.serve:
        serve()
        return
    if not args.files:
        parser.error("Provide one or more PDF files")
    token = ensure_server()
    for path in args.files:
        result = request(token, "/open", {"path": str(path.resolve(strict=True))})
        if args.print_url:
            print(result["url"])
        else:
            subprocess.run([
                "/usr/bin/open", "-na", "Google Chrome", "--args",
                "--app=" + result["url"],
            ], check=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("PDF Preview: " + str(error), file=sys.stderr)
        sys.exit(1)
