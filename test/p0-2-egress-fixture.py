#!/usr/bin/env python3
"""Dual-stack HTTP, raw TCP, half-close, and TLS fixture for the Linux Gate."""

import http.server
import os
import signal
import socket
import socketserver
import ssl
import threading
import time

HTTP_PORT = 38080
TCP_PORT = 38081
TLS_PORT = 38082
PUBLIC_IPV4 = "93.184.216.34"
PUBLIC_IPV6 = "2606:4700:4700::1111"
CERTIFICATE = os.environ["OPEN_COMPUTE_EGRESS_FIXTURE_CERT"]
PRIVATE_KEY = os.environ["OPEN_COMPUTE_EGRESS_FIXTURE_KEY"]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/redirect-private":
            self.send_response(302)
            self.send_header("Location", f"http://127.0.0.1:{HTTP_PORT}/private")
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


class ThreadingTcpServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def handle_error(self, _request, _client_address):
        return


class IPv6ThreadingTcpServer(ThreadingTcpServer):
    address_family = socket.AF_INET6


def read_line(connection, maximum):
    data = bytearray()
    while len(data) <= maximum:
        chunk = connection.recv(1)
        if not chunk:
            raise ConnectionError("unexpected EOF while reading command")
        data.extend(chunk)
        if chunk == b"\n":
            return bytes(data)
    raise ValueError("command is too long")


def read_exact(connection, size):
    data = bytearray()
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise ConnectionError("unexpected EOF while reading payload")
        data.extend(chunk)
    return bytes(data)


class RawTcpHandler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(5)
        command = read_line(self.request, 64).decode("ascii").strip().split()
        if len(command) == 2 and command[0] == "ECHO":
            size = int(command[1])
            if not 0 <= size <= 1024 * 1024:
                raise ValueError("payload size is outside the fixture bound")
            payload = read_exact(self.request, size)
            for offset in range(0, len(payload), 257):
                self.request.sendall(payload[offset : offset + 257])
            if isinstance(self.request, ssl.SSLSocket):
                self.request.unwrap().close()
            else:
                self.request.shutdown(socket.SHUT_WR)
            return
        if command == ["HALF"]:
            self.request.sendall(b"peer-half-close")
            self.request.shutdown(socket.SHUT_WR)
            while self.request.recv(4096):
                pass
            return
        if command == ["STALL"]:
            time.sleep(2)
            return
        raise ValueError("unknown fixture command")


class TlsTcpServer(ThreadingTcpServer):
    def __init__(self, address, handler, context):
        self.context = context
        super().__init__(address, handler)

    def get_request(self):
        connection, address = super().get_request()
        try:
            return self.context.wrap_socket(connection, server_side=True), address
        except BaseException:
            connection.close()
            raise


class IPv6TlsTcpServer(TlsTcpServer):
    address_family = socket.AF_INET6


tls_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
tls_context.load_cert_chain(CERTIFICATE, PRIVATE_KEY)
servers = [
    http.server.ThreadingHTTPServer((PUBLIC_IPV4, HTTP_PORT), Handler),
    IPv6Server((PUBLIC_IPV6, HTTP_PORT), Handler),
    http.server.ThreadingHTTPServer(("127.0.0.1", HTTP_PORT), Handler),
    ThreadingTcpServer((PUBLIC_IPV4, TCP_PORT), RawTcpHandler),
    IPv6ThreadingTcpServer((PUBLIC_IPV6, TCP_PORT), RawTcpHandler),
    ThreadingTcpServer(("127.0.0.1", TCP_PORT), RawTcpHandler),
    TlsTcpServer((PUBLIC_IPV4, TLS_PORT), RawTcpHandler, tls_context),
    IPv6TlsTcpServer((PUBLIC_IPV6, TLS_PORT), RawTcpHandler, tls_context),
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
