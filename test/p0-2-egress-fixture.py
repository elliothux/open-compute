#!/usr/bin/env python3
"""Dual-stack HTTP fixture for the privileged Linux P0.2 egress Gate."""

import http.server
import signal
import socket
import threading

PORT = 38080
PUBLIC_IPV4 = "93.184.216.34"
PUBLIC_IPV6 = "2606:4700:4700::1111"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/redirect-private":
            self.send_response(302)
            self.send_header("Location", f"http://127.0.0.1:{PORT}/private")
            self.end_headers()
            return
        body = b"fixture-ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        return


class IPv6Server(http.server.ThreadingHTTPServer):
    address_family = socket.AF_INET6


servers = [
    http.server.ThreadingHTTPServer((PUBLIC_IPV4, PORT), Handler),
    IPv6Server((PUBLIC_IPV6, PORT), Handler),
]
stopping = threading.Event()


def stop(_signum, _frame):
    stopping.set()


signal.signal(signal.SIGTERM, stop)
threads = [
    threading.Thread(target=server.serve_forever, daemon=True) for server in servers
]
for thread in threads:
    thread.start()
stopping.wait()
for server in servers:
    server.shutdown()
    server.server_close()
for thread in threads:
    thread.join()
