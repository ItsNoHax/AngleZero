#!/usr/bin/env python3
"""Convert a video into ICON1.PMF, the PSP's animated XMB icon.

usage: make_pmf.py INPUT.mp4 OUTPUT.PMF

PMF (PSMF) is Sony's container: a 2048-byte header followed by an MPEG-2
program stream of 2048-byte packs carrying one H.264 video stream. ffmpeg can
demux it but has no muxer, so this script re-encodes with libx264 and writes
the container itself.

The layout mirrors a Sony-authored 144x80 ICON1 (PPSSPP's
pspautotests/tests/video/pmf/test.pmf), field for field:

* Video: H.264 Main profile, level 2.1, CABAC, I and P pictures only, one
  reference frame, access-unit delimiters, NAL HRD buffering-period and
  pic-timing SEI. x264's own user-data SEI is dropped.
* Every GOP starts a new pack. That pack carries the system header, a
  private-stream-2 (0xBF) index of the GOP's access units, and a video PES
  with PTS, DTS and a P-STD buffer extension. The GOP's last pack is padded
  with a padding stream (0xBE).
* The 0xBF index: '01 e0', the pack index (relative to the GOP) holding the
  end of each of the first four access units, four zero bytes, then the
  byte size of every access unit. Checked against both Sony GOP layouts in
  test.pmf and a second, independently authored ICON1.
* PTS/DTS appear on the GOP's first video PES, then on the first PES that
  starts an access unit at least 16 frames after the last stamped one.

Ported unchanged from SSB64PSP (tools/make-pmf.py), where its output animates on a PSP
Slim, firmware 6.61. See docs/assets.md.
"""
import struct
import subprocess
import sys

SECTOR = 2048
PACK_HEADER = 14
FRAME_TICKS = 3003  # 90 kHz ticks per frame at 29.97 fps
FIRST_PTS = 90000
PTS_INTERVAL = 16 * FRAME_TICKS
MUX_RATE = 25000  # units of 50 bytes/s
SCR_START = 3044236  # 27 MHz, as in the Sony reference
SCR_PER_PACK = SECTOR * 27_000_000 / (MUX_RATE * 50)
DELIVERY_LEAD = 76850  # 90 kHz; reference pack 0 arrives this long before its DTS
GOP = 59  # reference GOP length
STD_BUFFER_KB = 0x55

SYSTEM_HEADER = bytes.fromhex("000001bb000c80c35180f07fb9e055bde008")

X264_PARAMS = ":".join([
    f"keyint={GOP}", f"min-keyint={GOP}", "scenecut=0",
    "bframes=0", "ref=1", "weightp=0",
    "aud=1", "nal-hrd=vbr", "pic-struct=1", "force-cfr=1",
    # The P-STD buffer is 85 KiB; keep the VBV well inside it.
    "vbv-maxrate=600", "vbv-bufsize=600",
])


def encode(src):
    cmd = [
        "ffmpeg", "-v", "error", "-i", src, "-an",
        "-vf", "scale=144:80,fps=30000/1001", "-pix_fmt", "yuv420p",
        "-c:v", "libx264", "-profile:v", "main", "-level", "2.1",
        "-crf", "18", "-x264-params", X264_PARAMS,
        "-bsf:v", "h264_mp4toannexb", "-f", "h264", "-",
    ]
    return subprocess.run(cmd, check=True, stdout=subprocess.PIPE).stdout


def nal_units(stream):
    """Split an Annex B stream into NAL payloads (start codes removed)."""
    starts = []
    i = 0
    while True:
        i = stream.find(b"\0\0\1", i)
        if i < 0:
            break
        starts.append(i + 3)
        i += 3
    for n, s in enumerate(starts):
        end = starts[n + 1] - 3 if n + 1 < len(starts) else len(stream)
        while end > s and stream[end - 1] == 0:
            end -= 1  # trailing zero of a 4-byte start code
        yield stream[s:end]


def access_units(stream):
    """Group NALs into access units at each delimiter; return [(bytes, idr)]."""
    units = []
    for nal in nal_units(stream):
        kind = nal[0] & 0x1F
        if kind == 9:
            units.append([b"", False])
        if kind == 6 and nal[1] == 5:
            continue  # x264's user-data-unregistered version string
        if kind == 5:
            units[-1][1] = True
        units[-1][0] += b"\0\0\0\1" + nal
    return [(bytes(a), idr) for a, idr in units]


def ts5(prefix, t):
    return bytes([
        (prefix << 4) | (((t >> 30) & 7) << 1) | 1,
        (t >> 22) & 0xFF, (((t >> 15) & 0x7F) << 1) | 1,
        (t >> 7) & 0xFF, ((t & 0x7F) << 1) | 1,
    ])


def pack_header(scr):
    base, ext = divmod(scr, 300)
    b = [
        0x40 | (((base >> 30) & 7) << 3) | 0x04 | ((base >> 28) & 3),
        (base >> 20) & 0xFF,
        (((base >> 15) & 0x1F) << 3) | 0x04 | ((base >> 13) & 3),
        (base >> 5) & 0xFF,
        ((base & 0x1F) << 3) | 0x04 | ((ext >> 7) & 3),
        ((ext & 0x7F) << 1) | 1,
    ]
    rate = (MUX_RATE << 2) | 3
    return b"\0\0\1\xba" + bytes(b) + rate.to_bytes(3, "big") + b"\xf8"


def pes(stream_id, header, payload):
    body = header + payload
    return b"\0\0\1" + bytes([stream_id]) + len(body).to_bytes(2, "big") + body


def video_header(pts=None, pstd=False, stuffing=0):
    fields = b""
    flags = 0
    if pts is not None:
        flags |= 0xC0
        fields += ts5(3, pts) + ts5(1, pts - FRAME_TICKS)
    if pstd:
        flags |= 0x01
        fields += bytes([0x1E, 0x60 | (STD_BUFFER_KB >> 8), STD_BUFFER_KB & 0xFF])
    fields += b"\xff" * stuffing
    return bytes([0x81, flags, len(fields)]) + fields


def mux_gop(units, first_frame):
    """Lay one GOP out in packs. Returns [(payload_packets, dts_of_first_byte)]."""
    data = b"".join(u for u, _ in units)
    starts = []  # byte offset of each access unit
    off = 0
    for u, _ in units:
        starts.append(off)
        off += len(u)
    index_len = 6 + 18 + 4 * len(units)

    packs = []  # [ [packets], stamped_frame_or_None, first_byte, end_byte ]
    pos = 0
    last_stamp = None
    while pos < len(data):
        room = SECTOR - PACK_HEADER
        first = not packs
        if first:
            room -= len(SYSTEM_HEADER) + index_len
        if first:
            stamp, header_len = 0, 9 + 13
        else:
            # Stamp the first access unit that starts in this pack, if it is
            # far enough from the last stamped one.
            header_len = 9 + 10
            end = pos + min(room - header_len, len(data) - pos)
            stamp = next((i for i, s in enumerate(starts) if pos <= s < end), None)
            if stamp is None or stamp - last_stamp < PTS_INTERVAL // FRAME_TICKS:
                stamp, header_len = None, 9
        take = min(room - header_len, len(data) - pos)
        if stamp is not None:
            last_stamp = stamp
        packs.append([stamp, pos, pos + take, room - header_len - take])
        pos += take

    # Pack (relative to the GOP) holding the last byte of each access unit.
    def pack_of(byte):
        return next(n for n, p in enumerate(packs) if p[1] <= byte < p[2])

    ends = [pack_of(starts[i] + len(units[i][0]) - 1) for i in range(len(units))]
    ends += [ends[-1]] * (4 - len(ends))
    index = b"\x01\xe0" + b"".join(e.to_bytes(2, "big") for e in ends[:4])
    index += b"\0\0\0\0" + (2 + 4 * len(units)).to_bytes(2, "big")
    index += len(units).to_bytes(2, "big")
    for i, (u, _) in enumerate(units):
        flag = 0x0080 if i + 1 < len(units) else 0
        index += flag.to_bytes(2, "big") + len(u).to_bytes(2, "big")
    assert 6 + len(index) == index_len

    out = []
    for n, (stamp, a, b, spare) in enumerate(packs):
        pts = None if stamp is None else FIRST_PTS + (first_frame + stamp) * FRAME_TICKS
        stuffing = 0
        padding = b""
        if spare >= 6:
            padding = pes(0xBE, b"", b"\xff" * (spare - 6))
        else:
            stuffing = spare
        packets = b""
        if n == 0:
            packets += SYSTEM_HEADER + pes(0xBF, b"", index)
        packets += pes(0xE0, video_header(pts, n == 0, stuffing), data[a:b]) + padding
        assert PACK_HEADER + len(packets) == SECTOR
        au = next(i for i in range(len(units)) if starts[i] + len(units[i][0]) > a)
        out.append((packets, FIRST_PTS + (first_frame + au - 1) * FRAME_TICKS))
    return out


def header(stream_size, frames):
    end = FIRST_PTS + frames * FRAME_TICKS
    t6 = lambda t: t.to_bytes(6, "big")
    h = bytearray(SECTOR)
    h[0:8] = b"PSMF0014"
    h[8:16] = struct.pack(">II", SECTOR, stream_size)
    meta = struct.pack(">I", 0x3E) + t6(FIRST_PTS) + t6(end)
    meta += struct.pack(">II", MUX_RATE, 90000) + b"\x01\x01"
    meta += struct.pack(">I", 0x24) + t6(FIRST_PTS) + t6(end)
    meta += struct.pack(">HIH", 1, 0x12, 1)
    meta += bytes.fromhex("e0002055") + bytes(8) + bytes([144 // 16, 80 // 16, 0, 0])
    h[0x50:0x50 + len(meta)] = meta
    return bytes(h)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    units = access_units(encode(sys.argv[1]))
    assert units and units[0][1], "stream must open with an IDR picture"
    assert all(len(u) < 0x10000 for u, _ in units), "access unit too large for the index"

    gops = []
    for i, unit in enumerate(units):
        if unit[1]:
            gops.append((i, []))
        gops[-1][1].append(unit)

    body = b""
    scr = SCR_START
    for first_frame, gop in gops:
        for packets, dts in mux_gop(gop, first_frame):
            scr = max(scr, (dts - DELIVERY_LEAD) * 300)
            body += pack_header(round(scr)) + packets
            scr += SCR_PER_PACK

    with open(sys.argv[2], "wb") as f:
        f.write(header(len(body), len(units)) + body)
    print(f"{sys.argv[2]}: {len(units)} frames, {len(gops)} GOPs, "
          f"{len(body) // SECTOR} packs, {SECTOR + len(body)} bytes")


if __name__ == "__main__":
    main()
