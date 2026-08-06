#!/usr/bin/env python3
"""Hold PSP buttons in PPSSPPHeadless via its WebSocket debugger.

PPSSPPHeadless has no input device, but `--debugger=<port>` starts the WebSocket debugger, whose
`input.buttons.send` call feeds the same HLE entry point (`__CtrlUpdateButtons`) that real input
uses -- so the guest's sceCtrlReadBufferPositive sees a genuine press.

Usage:
    PPSSPPHeadless --graphics=software --debugger=9333 \
        --screenshot-save=out.bmp --timeout=10 \
        target/mipsel-sony-psp/debug/angle-zero.prx &
    python3 scripts/psp_input.py 9333 cross

Buttons are held until this script is killed. That is deliberate: the app screenshots itself
periodically, so releasing early lets a later idle frame overwrite the held one. Let headless hit
its own --timeout while the button is still down.

Button names: cross, circle, triangle, square, up, down, left, right, start, select,
ltrigger, rtrigger (see PPSSPP's InputSubscriber.cpp for the full list).

No third-party dependencies -- implements just enough of RFC 6455 to talk to the debugger.
"""

import base64
import json
import os
import socket
import struct
import sys
import threading
import time

SUBPROTOCOL = "debugger.ppsspp.org"


def handshake(sock, port):
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall(
        (
            f"GET /debugger HTTP/1.1\r\n"
            f"Host: localhost:{port}\r\n"
            f"Upgrade: websocket\r\n"
            f"Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            f"Sec-WebSocket-Version: 13\r\n"
            f"Sec-WebSocket-Protocol: {SUBPROTOCOL}\r\n\r\n"
        ).encode()
    )
    resp = b""
    while b"\r\n\r\n" not in resp:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("connection closed during handshake")
        resp += chunk
    status = resp.split(b"\r\n", 1)[0]
    if b"101" not in status:
        raise RuntimeError(f"handshake rejected: {status!r}")


def send(sock, obj):
    """Send one masked text frame. Clients must mask; servers must not."""
    payload = json.dumps(obj).encode()
    mask = os.urandom(4)
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    n = len(payload)
    if n < 126:
        header = struct.pack("!BB", 0x81, 0x80 | n)
    elif n < 65536:
        header = struct.pack("!BBH", 0x81, 0x80 | 126, n)
    else:
        header = struct.pack("!BBQ", 0x81, 0x80 | 127, n)
    sock.sendall(header + mask + masked)


def read_responses(sock, verbose):
    """Drain incoming frames. Failed requests come back as {"event":"error",...}."""
    buf = b""
    while True:
        try:
            chunk = sock.recv(4096)
        except OSError:
            return
        if not chunk:
            return
        buf += chunk
        while len(buf) >= 2:
            length = buf[1] & 0x7F
            offset = 2
            if length == 126:
                if len(buf) < 4:
                    break
                length = struct.unpack_from("!H", buf, 2)[0]
                offset = 4
            elif length == 127:
                if len(buf) < 10:
                    break
                length = struct.unpack_from("!Q", buf, 2)[0]
                offset = 10
            if len(buf) < offset + length:
                break
            opcode, payload = buf[0] & 0x0F, buf[offset:offset + length]
            buf = buf[offset + length:]
            if opcode != 0x1:
                continue
            text = payload.decode(errors="replace")
            if verbose or '"error"' in text:
                print(f"  << {text}", flush=True)


def parse_phases(args):
    """Splits `a b --then 8 c d` into [(0, [a, b]), (8.0, [c, d])].

    One connection has to drive the whole sequence: headless exits at its own --timeout, and a
    second script started later usually finds the port already closed.
    """
    phases, current, delay = [], [], 0.0
    i = 0
    while i < len(args):
        if args[i] == "--then":
            phases.append((delay, current))
            delay = float(args[i + 1])
            current = []
            i += 2
            continue
        current.append(args[i])
        i += 1
    phases.append((delay, current))
    return [(d, b or ["cross"]) for d, b in phases]


def main():
    if len(sys.argv) < 2:
        sys.exit(
            f"usage: {sys.argv[0]} <debugger-port> [button ...] "
            f"[--then <seconds> button ...] [--verbose]\n"
            f"  e.g. {sys.argv[0]} 9333 cross --then 8 cross circle left"
        )

    verbose = "--verbose" in sys.argv
    args = [a for a in sys.argv[1:] if a != "--verbose"]
    port = int(args[0])
    phases = parse_phases(args[1:])

    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    handshake(sock, port)
    threading.Thread(target=read_responses, args=(sock, verbose), daemon=True).start()

    # --debugger implies startBreak, so the core sits halted at boot until resumed.
    send(sock, {"event": "cpu.resume"})
    time.sleep(1.5)

    held = set()
    try:
        for delay, buttons in phases:
            if delay:
                time.sleep(delay)
            # Release anything this phase drops, then press what it wants.
            release = held - set(buttons)
            if release:
                send(sock, {"event": "input.buttons.send",
                            "buttons": {b: False for b in release}})
            send(sock, {"event": "input.buttons.send", "buttons": {b: True for b in buttons}})
            held = set(buttons)
            print(f"holding: {', '.join(buttons)}", flush=True)

        print("(ctrl-c or kill to release)", flush=True)
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        send(sock, {"event": "input.buttons.send", "buttons": {b: False for b in held}})
    finally:
        sock.close()


if __name__ == "__main__":
    main()
