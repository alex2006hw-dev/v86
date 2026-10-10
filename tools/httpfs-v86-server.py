#!/usr/bin/env python3
"""Serve a directory over plain HTTP, reading the bytes through httpfs.

v86 loads disk and CD images with `fetch()`, which wants an ordinary HTTP
file: a GET that returns the bytes, ideally answering `Range` requests.
The httpfs server does not speak that -- it exposes a JSON RPC for POSIX
operations and refuses any request without an `HttpFsClient` user agent.

This bridges the two. The bytes genuinely come from an httpfs server
(OP_OPEN then OP_READ); only the protocol in front is translated. So a boot
test driven by this exercises the httpfs path end to end rather than quietly
bypassing it.

    python -m httpfs.server 8099 /path/to/images/          # upstream server
    tools/httpfs-v86-server.py --upstream 8099 --port 8100

Then point v86 at, for example,
`http://127.0.0.1:8100/debian-12.1.0-i386-netinst.iso`.

The file descriptor is opened once per file and kept open, because
httpfs's OP_READ takes a descriptor rather than a path -- there is no way to
read a byte range without one, and re-opening per request would double the
round trips for every sector the firmware reads.
"""

import argparse
import base64
import json
import os
import sys
import threading
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Operation numbers from httpfs/common/HttpFsRequest.py.
OP_OPEN = 8
OP_READ = 9
OP_GET_ATTR = 4
OP_READDIR = 10

USER_AGENT = "HttpFsClient/0.1"

# The server base64-encodes every read, and accepts a size argument. A read
# big enough to matter for throughput would turn into a large JSON string
# (a third more on the wire, plus a decode on both ends), so cap it and let
# the range handler loop.
MAX_READ = 1 << 20


class HttpFsError(Exception):
    pass


class HttpFsClient:
    """A very small httpfs client: no FUSE, no fuse_get_context.

    Upstream's `HttpFsClient` is a FUSE Operations class, so it calls
    `fuse_get_context()` for uid/gid on every operation and cannot be driven
    outside a FUSE request. The wire protocol is a POST of
    `{"type": N, "args": {...}}` answering `{"error_no": 0,
    "response_data": {...}}`, which is all that is needed here.

    Requests are serialised: the httpfs server keeps real OS file
    descriptors, and sharing one across concurrent range requests would
    interleave seeks.
    """

    def __init__(self, host, port):
        self.url = "http://%s:%d/" % (host, port)
        self.lock = threading.Lock()
        self._fds = {}

    def _post(self, op, args):
        body = json.dumps({"type": op, "args": args}).encode("utf-8")
        request = urllib.request.Request(
            self.url,
            data=body,
            headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = json.loads(response.read())
        except urllib.error.HTTPError as error:
            raise HttpFsError("httpfs returned HTTP %s" % error.code) from error
        except urllib.error.URLError as error:
            raise HttpFsError("cannot reach httpfs at %s: %s" % (self.url, error.reason)) from error

        if payload.get("error_no"):
            raise HttpFsError(payload.get("response_data", {}).get("message", "error"))
        return payload["response_data"]

    def listdir(self, path):
        return self._post(OP_READDIR, {"path": path, "uid": 0, "gid": 0})["dir_listing"]

    def stat(self, path):
        data = self._post(OP_GET_ATTR, {"path": path, "uid": 0, "gid": 0})
        return data["st_size"]

    def _open(self, name):
        with self.lock:
            fd = self._fds.get(name)
            if fd is None:
                fd = self._post(OP_OPEN, {"path": name, "flags": 0, "uid": 0, "gid": 0})
                fd = fd["file_descriptor"]
                self._fds[name] = fd
        return fd

    def read(self, name, offset, size):
        fd = self._open(name)
        size = min(size, MAX_READ)
        with self.lock:
            data = self._post(OP_READ, {
                "file_descriptor": fd,
                "size": size,
                "offset": offset,
                "uid": 0,
                "gid": 0,
            })
        return base64.b64decode(data["bytes_read"])


class ImageHandler(BaseHTTPRequestHandler):
    """Plain GET/HEAD with `Range` support, backed by an httpfs client."""

    protocol_version = "HTTP/1.1"
    server_version = "httpfs-v86/1.0"

    def log_message(self, fmt, *args):
        if self.server.verbose:
            sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))

    def do_HEAD(self):
        self.respond(body=False)

    def do_GET(self):
        self.respond(body=True)

    def respond(self, body):
        name = os.path.basename(self.path.split("?", 1)[0].lstrip("/"))

        try:
            client = self.server.client
            size = client.stat(name)
        except HttpFsError as error:
            self.send_error(404, str(error))
            return

        start, end = 0, size - 1
        partial = False
        header = self.headers.get("Range")

        if header and header.startswith("bytes="):
            spec = header[len("bytes="):].split(",")[0].strip()
            first, _, last = spec.partition("-")
            try:
                if first:
                    start = int(first)
                    end = int(last) if last else size - 1
                elif last:
                    # Suffix range: the final N bytes.
                    start = max(0, size - int(last))
                    end = size - 1
            except ValueError:
                self.send_error(416, "malformed Range")
                return

            if start >= size or start > end:
                self.send_response(416)
                self.send_header("Content-Range", "bytes */%d" % size)
                self.send_header("Content-Length", 0)
                self.end_headers()
                return
            end = min(end, size - 1)
            partial = True

        length = end - start + 1

        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(length))
        self.send_header("Accept-Ranges", "bytes")
        if partial:
            self.send_header("Content-Range", "bytes %d-%d/%d" % (start, end, size))
        self.end_headers()

        if not body:
            return

        offset = start
        try:
            while offset <= end:
                chunk = client.read(name, offset, min(MAX_READ, end - offset + 1))
                if not chunk:
                    break
                self.wfile.write(chunk)
                offset += len(chunk)
        except (BrokenPipeError, ConnectionResetError):
            pass
        except HttpFsError as error:
            # The status line is long gone, so the only honest signal left is
            # to stop writing and let the client notice the short read.
            sys.stderr.write("httpfs read error: %s\n" % error)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--upstream", default="127.0.0.1:8099",
                        help="host:port of the httpfs server (default 127.0.0.1:8099)")
    parser.add_argument("--port", type=int, default=8100, help="port to serve plain HTTP on")
    parser.add_argument("-v", "--verbose", action="store_true", help="log every request")
    parser.add_argument("--list", action="store_true", help="list the images and exit")
    args = parser.parse_args()

    host, _, port = args.upstream.partition(":")
    client = HttpFsClient(host, int(port))

    try:
        names = [n for n in client.listdir(".") if not n.startswith(".")]
    except HttpFsError as error:
        sys.exit("cannot list images over httpfs: %s" % error)

    if args.list:
        for name in sorted(names):
            print("%12d  %s" % (client.stat(name), name))
        return

    for name in sorted(names):
        sys.stderr.write("serving %s (%d bytes)\n" % (name, client.stat(name)))
    sys.stderr.write("http://%s:%d/  ->  httpfs at %s\n" % ("127.0.0.1", args.port, client.url))

    server = ThreadingHTTPServer(("0.0.0.0", args.port), ImageHandler)
    server.client = client
    server.verbose = args.verbose
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()