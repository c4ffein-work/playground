#!/usr/bin/env python3
"""Runs companion.py for the e2e tests: run_companion.py /path/to/companion.py server --port N

companion binds a dual-stack IPv6 socket. Where the kernel has no IPv6 at all
(some containers), this makes it bind IPv4 instead; anywhere else it changes nothing.
"""

import runpy
import socket
import sys

try:
    socket.socket(socket.AF_INET6).close()
except OSError:
    socket.AF_INET6 = socket.AF_INET
    _bind = socket.socket.bind

    def bind(self, address):
        host, port = address[:2]
        return _bind(self, ("0.0.0.0" if host == "::" else host, port))

    socket.socket.bind = bind

sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name="__main__")
