import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SPEEDS = {"127.0.0.2": 2_000_000, "127.0.0.3": 1_000_000}
DEFAULT_SPEED = 100_000_000
PATTERN = bytes((i * 7 + 3) & 0xFF for i in range(65536))
stats = {}
lock = threading.Lock()


class Bucket:
    def __init__(self, rate):
        self.rate = rate
        self.next = time.monotonic()
        self.lock = threading.Lock()

    def take(self, n):
        with self.lock:
            now = time.monotonic()
            start = max(self.next, now - 0.1)
            self.next = start + n / self.rate
            wait = self.next - now
        if wait > 0:
            time.sleep(wait)


buckets = {}


def bucket_for(ip):
    with lock:
        if ip not in buckets:
            buckets[ip] = Bucket(SPEEDS.get(ip, DEFAULT_SPEED))
        return buckets[ip]


def content(offset, length):
    out = bytearray()
    while length > 0:
        o = offset % 65536
        take = min(65536 - o, length)
        out += PATTERN[o:o + take]
        offset += take
        length -= take
    return bytes(out)


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        m = re.match(r"/file(\d+)mb\.bin", self.path)
        if self.path == "/stats":
            body = repr(stats).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if not m:
            self.send_error(404)
            return
        total = int(m.group(1)) * 1_000_000
        start, end, code = 0, total - 1, 200
        rng = self.headers.get("Range")
        if rng:
            r = re.match(r"bytes=(\d+)-(\d*)", rng)
            start = int(r.group(1))
            end = int(r.group(2)) if r.group(2) else total - 1
            end = min(end, total - 1)
            code = 206
        length = end - start + 1
        self.send_response(code)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        self.send_header("ETag", '"test-etag"')
        if code == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
        self.end_headers()
        ip = self.client_address[0]
        b = bucket_for(ip)
        sent = 0
        try:
            while sent < length:
                n = min(16384, length - sent)
                b.take(n)
                self.wfile.write(content(start + sent, n))
                sent += n
                with lock:
                    stats[ip] = stats.get(ip, 0) + n
        except (ConnectionError, OSError):
            pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
    ThreadingHTTPServer.daemon_threads = True
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
