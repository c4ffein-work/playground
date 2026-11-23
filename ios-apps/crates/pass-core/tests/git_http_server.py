#!/usr/bin/env python3
"""Serves the git repositories of a directory over smart HTTP, for the e2e tests:

    git_http_server.py --root DIR --port N [--user U --password P] [--cert PEM --key PEM]

It is `git http-backend` (the real server side of git over http) behind the smallest
web server that can run a CGI. With --user, every request needs that Basic
authentication; with --cert, it speaks https. Pushing is allowed.
"""

import argparse
import base64
import http.server
import os
import ssl
import subprocess


class Handler(http.server.BaseHTTPRequestHandler):
    # HTTP/1.0: one request per connection, the response ends when the connection closes.
    protocol_version = "HTTP/1.0"

    def log_message(self, *args):
        pass

    def authorized(self):
        if self.server.credentials is None:
            return True
        expected = "Basic " + base64.b64encode(self.server.credentials.encode()).decode()
        return self.headers.get("Authorization") == expected

    def serve(self):
        if not self.authorized():
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="git"')
            self.end_headers()
            return
        path, _, query = self.path.partition("?")
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        env = dict(
            os.environ,
            GIT_PROJECT_ROOT=self.server.root,
            GIT_HTTP_EXPORT_ALL="1",
            REQUEST_METHOD=self.command,
            PATH_INFO=path,
            QUERY_STRING=query,
            CONTENT_TYPE=self.headers.get("Content-Type", ""),
            CONTENT_LENGTH=str(len(body)),
            # http-backend only accepts a push from an authenticated user.
            REMOTE_USER="e2e",
            REMOTE_ADDR="127.0.0.1",
            GIT_PROTOCOL=self.headers.get("Git-Protocol", ""),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_CONFIG_GLOBAL=os.devnull,
        )
        done = subprocess.run(["git", "http-backend"], input=body, env=env, capture_output=True)
        head, _, content = done.stdout.partition(b"\r\n\r\n")
        status = 200
        headers = []
        for line in head.decode().split("\r\n"):
            name, _, value = line.partition(":")
            if name.lower() == "status":
                status = int(value.split()[0])
            elif name:
                headers.append((name, value.strip()))
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        # Over TLS, a response that just stops cannot be told from one that was cut short.
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)

    do_GET = serve
    do_POST = serve


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--user")
    parser.add_argument("--password")
    parser.add_argument("--cert")
    parser.add_argument("--key")
    args = parser.parse_args()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.root = os.path.abspath(args.root)
    server.credentials = f"{args.user}:{args.password}" if args.user else None
    if args.cert:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(args.cert, args.key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
