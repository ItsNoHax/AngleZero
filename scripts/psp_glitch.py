#!/usr/bin/env python3
"""Hunt for one-frame rendering glitches in a reproducible headless run.

The flicker this looks for -- terrain popping in and out, a chunk of scenery vanishing for a frame,
a seam opening along the road -- is invisible to a single screenshot by definition. It only exists
as a *difference between consecutive frames*, so this drives a scripted run that captures a burst of
consecutive frames, then compares each frame against its two neighbours.

The signature is an A-B-A: a tile that differs from the frame before it and from the frame after it,
while those two agree with each other. Ordinary camera motion changes almost every pixel every
frame, but it changes them *progressively* -- frame N-1 and frame N+1 differ from each other at
least as much as either differs from N -- so motion cancels out of that subtraction and a one-frame
blink does not.

Requires the `harness` cargo feature (see `src/psp/harness.rs`) for the deterministic run, and
Pillow for the image comparison.

    scripts/psp_glitch.py --burst 400 --frames 40 --label hairpin

Environment: `PPSSPP_HEADLESS` for the emulator binary, `PPSSPP_MEMSTICK` for the directory
headless maps `ms0:` onto (both have sensible defaults).
"""

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

try:
    from PIL import Image, ImageChops
except ImportError:
    sys.exit("this needs Pillow: pip install --user Pillow")

DEFAULT_HEADLESS = Path.home() / ".local/src/ppsspp/build-headless/PPSSPPHeadless"
# Headless hardcodes this on Linux -- see `g_Config.memStickDirectory` in headless/Headless.cpp.
DEFAULT_MEMSTICK = Path.home() / ".ppsspp"
PRX = Path("target/mipsel-sony-psp/debug/angle-zero.prx")

# The framebuffer is 512 px wide; the right-hand 32 are stride the display never shows.
VISIBLE = (480, 272)
TILE = 8

# A per-channel difference this large or more counts as a changed pixel. The night palette is dark
# and the fog gradient dithers, so a couple of levels of noise between frames means nothing.
PIXEL_DELTA = 24
# Report a tile when this fraction of it blinked, as 0..255. 60 is a touch under a quarter.
TILE_THRESHOLD = 60
# How far, in pixels, the motion search looks for the same content in the neighbouring frame. At
# 90 km/h the fastest thing on screen is a lamp post a few metres to the side, which sweeps about
# this much in a frame. Larger costs time and starts matching unrelated content by coincidence.
MAX_SHIFT = 16


def log(msg):
    print(msg, flush=True)


def build():
    log(">> building with --features harness")
    # cargo psp writes the .prx; the abicalls/discarded-section linker warnings are expected.
    proc = subprocess.run(
        ["cargo", "psp", "--features", "harness"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0 or not PRX.exists():
        sys.stderr.write(proc.stdout[-4000:] + proc.stderr[-4000:])
        sys.exit("build failed")


def write_script(anglezero, burst, frames, script_lines, node=None, kph=90, mode=0):
    """Writes the input script the guest reads at boot."""
    body = list(script_lines) if script_lines else ["0 -", "90 x"]
    if node is not None:
        # Start the run immediately, then drop the car where it is wanted: there is no reason to sit
        # through the title camera when the point of the run is a corner half a mile down the hill.
        body = ["0 -", "1 x", f"place 3 {node} {kph}"]
    head = [f"burst {burst} {frames}"]
    if mode:
        head.append(f"mode {mode}")
    text = "\n".join([*head, *body]) + "\n"
    anglezero.mkdir(parents=True, exist_ok=True)
    (anglezero / "SCRIPT.TXT").write_text(text)
    log(f">> script:\n{text.rstrip()}")


def clear_stale(anglezero):
    """Removes captures from a previous run so the harvest below cannot mix two runs together.

    Only the harness's own output is touched. Anything already harvested is in `captures/`, and
    `capture::capture` numbers from the first free slot, so leaving these would also push a second
    run's frames to different indices.
    """
    for pattern in ("SHOT*.BMP", "SHOT*.TXT", "TRACE*.TXT"):
        for path in anglezero.glob(pattern):
            path.unlink()


def run_headless(headless, timeout, graphics):
    """Runs the burst. The guest exits itself once its script is done, so `--timeout` is only a
    backstop against a hang -- it is not how long this is expected to take."""
    log(f">> running headless ({graphics}, backstop timeout {timeout}s)")
    start = time.monotonic()
    proc = subprocess.run(
        [
            str(headless),
            f"--graphics={graphics}",
            f"--timeout={timeout}",
            str(PRX),
        ],
        capture_output=True,
        text=True,
    )
    elapsed = time.monotonic() - start
    tail = (proc.stdout or "").strip().splitlines()[-3:]
    log(f">> emulator returned after {elapsed:.0f}s {tail}")
    return elapsed


def frame_of(png, burst):
    """The run frame a capture came from. The burst is consecutive, so this is just an offset."""
    if burst is None:
        return None
    return burst + int(png.stem.replace("SHOT", ""))


def read_burst(out):
    """The burst frame this directory was captured with, for `--scan-only` on an old run."""
    meta = out / "run.txt"
    if not meta.exists():
        return None
    for line in meta.read_text().splitlines():
        if line.startswith("burst "):
            return int(line.split()[1])
    return None


def harvest(anglezero, out):
    """Moves the run's output into `out`, converting frames to cropped PNGs."""
    out.mkdir(parents=True, exist_ok=True)
    frames = []
    for bmp in sorted(anglezero.glob("SHOT*.BMP")):
        png = out / (bmp.stem + ".png")
        Image.open(bmp).convert("RGB").crop((0, 0, *VISIBLE)).save(png)
        frames.append(png)
        bmp.unlink()
    for txt in sorted(anglezero.glob("SHOT*.TXT")) + sorted(anglezero.glob("TRACE*.TXT")):
        shutil.move(str(txt), str(out / txt.name))
    return frames


# --- the comparison -------------------------------------------------------------------------


def worst_channel_diff(a, b):
    """Per-pixel difference, taking the largest of the three channels.

    Not a luminance mix: a change that lands mostly in one channel -- the blue car against grey
    tarmac, a red tail lamp -- must not be averaged away.
    """
    r, g, bl = ImageChops.difference(a, b).split()
    return ImageChops.lighter(ImageChops.lighter(r, g), bl)


def motion_diff(a, b, max_dx=MAX_SHIFT, dys=(-1, 0, 1)):
    """Difference between two frames after allowing for the scene having *moved*.

    A plain difference cannot tell a one-frame blink from something thin travelling fast. A lamp
    post a few metres to the side sweeps 15-odd pixels a frame, which is wider than the post: at a
    given tile it is absent, present, absent on three consecutive frames, and the neighbours agree
    with each other because the post has passed through. That is pixel-for-pixel the signature of
    a blink.

    Taking the smallest difference over a range of shifts distinguishes them. A post that merely
    moved matches its neighbour at *some* offset, so its difference collapses; geometry that was
    genuinely absent matches at none of them and keeps its difference. Shifts are compared over
    their overlap only, so the strip a shift exposes at the edge is not mistaken for a change.
    """
    best = worst_channel_diff(a, b)
    w, h = VISIBLE
    for dx in range(-max_dx, max_dx + 1):
        for dy in dys:
            if dx == 0 and dy == 0:
                continue
            x0, x1 = max(0, dx), min(w, w + dx)
            y0, y1 = max(0, dy), min(h, h + dy)
            if x1 - x0 < 1 or y1 - y0 < 1:
                continue
            shifted = worst_channel_diff(
                a.crop((x0 - dx, y0 - dy, x1 - dx, y1 - dy)), b.crop((x0, y0, x1, y1))
            )
            best.paste(ImageChops.darker(best.crop((x0, y0, x1, y1)), shifted), (x0, y0))
    return best


def outside_bracket(prev, cur, nxt):
    """Per-pixel mask of where `cur` is not between its two neighbours in time.

    This is the test that separates a blink from fast motion, and it needs no motion model at all.
    While the scene merely moves, a pixel's value slides from what it was towards what it will be,
    so it lies inside the interval the neighbours span -- even when the motion is accelerating, which
    is what defeats a shift search. Something that appears for one frame and goes again lands
    *outside* that interval, brighter or darker than both sides.

    Without this a light pool sweeping under the car scores as high as a real dropout: it is a wide
    soft gradient that expands as it approaches, and no translation matches an expansion.
    """
    hi = ImageChops.lighter(prev, nxt)
    lo = ImageChops.darker(prev, nxt)
    # `subtract` clamps at zero, so each of these is only non-zero on the side that overshoots.
    over = ImageChops.subtract(cur, hi)
    under = ImageChops.subtract(lo, cur)
    r, g, b = ImageChops.lighter(over, under).split()
    return ImageChops.lighter(ImageChops.lighter(r, g), b)


def blink_scores(prev, cur, nxt):
    """Tiles where this frame both differs from its neighbours and cannot be explained by motion.

    Two independent filters, because either alone has a false-positive class that the other kills:
    the shift search forgives anything that merely moved sideways, and the bracket test forgives
    anything that moved smoothly in time.
    """
    d_before = motion_diff(prev, cur)
    d_after = motion_diff(cur, nxt)
    unmatched = ImageChops.darker(d_before, d_after)
    suspicious = ImageChops.darker(unmatched, outside_bracket(prev, cur, nxt))
    mask = suspicious.point(lambda v: 255 if v >= PIXEL_DELTA else 0)
    return mask.resize((VISIBLE[0] // TILE, VISIBLE[1] // TILE), Image.BOX)


def scan_frames(frames):
    """Scores every frame that has two neighbours. Returns findings, worst first."""
    if len(frames) < 3:
        log(f"!! only {len(frames)} frames -- need at least 3 to compare")
        return []

    images = [Image.open(p).convert("RGB") for p in frames]
    findings = []
    for i in range(1, len(images) - 1):
        score = blink_scores(images[i - 1], images[i], images[i + 1])
        peak = score.getextrema()[1]
        if peak < TILE_THRESHOLD:
            continue
        # Where it is, in screen pixels, so the finding can be looked at rather than trusted.
        tiles = [
            (v, n % (VISIBLE[0] // TILE), n // (VISIBLE[0] // TILE))
            for n, v in enumerate(score.getdata())
            if v >= TILE_THRESHOLD
        ]
        xs = [t[1] for t in tiles]
        ys = [t[2] for t in tiles]
        findings.append(
            {
                "frame": frames[i].stem,
                "peak": peak,
                "tiles": len(tiles),
                "box": (
                    min(xs) * TILE,
                    min(ys) * TILE,
                    (max(xs) + 1) * TILE,
                    (max(ys) + 1) * TILE,
                ),
            }
        )
    findings.sort(key=lambda f: (-f["peak"], -f["tiles"]))
    return findings


def mark(frames, finding, out):
    """Draws the flagged region on the offending frame and its neighbours, side by side, so the
    three can be compared the way the detector compared them."""
    names = [f.stem for f in frames]
    i = names.index(finding["frame"])
    trio = [Image.open(frames[j]).convert("RGB") for j in (i - 1, i, i + 1)]
    x0, y0, x1, y1 = finding["box"]
    for im in trio:
        px = im.load()
        for x in range(max(0, x0 - 1), min(VISIBLE[0], x1 + 1)):
            for y in (max(0, y0 - 1), min(VISIBLE[1] - 1, y1)):
                px[x, y] = (255, 0, 255)
        for y in range(max(0, y0 - 1), min(VISIBLE[1], y1 + 1)):
            for x in (max(0, x0 - 1), min(VISIBLE[0] - 1, x1)):
                px[x, y] = (255, 0, 255)
    sheet = Image.new("RGB", (VISIBLE[0], VISIBLE[1] * 3 + 8), (0, 0, 0))
    for n, im in enumerate(trio):
        sheet.paste(im, (0, n * (VISIBLE[1] + 4)))
    path = out / f"blink-{finding['frame']}.png"
    sheet.save(path)
    return path


# --- the trace ------------------------------------------------------------------------------


def has_hole(mask):
    """True when a chunk is missing from the middle of the drawn run.

    The counts alone hide this: a chunk dropped out of the middle of the visible span leaves a gap
    across the screen while the count merely dips by one.

    Only meaningful for a ribbon where every chunk holds geometry. Props and glows are sparse -- a
    stretch of hill with no lamps or trees on it has empty chunks that are *correctly* not drawn --
    so a gap there says nothing, and the A-B-A test below is the only trustworthy signal for them.
    """
    bits = bin(mask)[2:]
    return "0" in bits.strip("0")


def scan_trace(out):
    """Reads the per-frame draw tally and reports what pixels cannot say: whether a draw was
    issued at all."""
    traces = sorted(out.glob("TRACE*.TXT"))
    if not traces:
        return []
    # frame road terrain lines rails dashes props verts node kph us roadmask terrainmask
    # propmask glowmask
    columns = 15
    masks = (("road", 11), ("terrain", 12), ("props", 13), ("glows", 14))
    # Every chunk of these two carries ribbon, so a gap in them is always a dropped draw.
    dense = {"road", "terrain"}

    rows = []
    for line in traces[0].read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = line.split()
        if len(parts) < columns:
            continue
        rows.append([int(p) for p in parts[:columns]])

    notes = []
    for n, row in enumerate(rows):
        frame = row[0]
        for label, col in masks:
            if label in dense and has_hole(row[col]):
                notes.append(f"frame {frame}: hole in the {label} drawn chunks (mask {row[col]:#x})")
        # A-B-A in the submitted set: something was dropped for exactly one frame and came back.
        # Unlike the pixel comparison this is exact -- it is the draw call itself, not its result.
        if 0 < n < len(rows) - 1:
            for label, col in masks:
                if rows[n - 1][col] == rows[n + 1][col] != row[col]:
                    dropped = rows[n - 1][col] & ~row[col]
                    notes.append(
                        f"frame {frame}: {label} chunk set blinked "
                        f"({rows[n - 1][col]:#x} -> {row[col]:#x} -> {rows[n + 1][col]:#x}"
                        + (f", chunk {dropped.bit_length() - 1} dropped)" if dropped else ")")
                    )
    return notes


def scan_counters(out):
    """The two silent failure modes: an exhausted vertex arena and an overrun display list."""
    notes = []
    for txt in sorted(out.glob("SHOT*.TXT")):
        fields = {}
        for line in txt.read_text().splitlines():
            parts = line.split()
            if len(parts) == 2:
                fields[parts[0]] = parts[1]
        failures = int(fields.get("scratch_failures", 0))
        list_bytes = int(fields.get("list_bytes", 0))
        if failures:
            notes.append(
                f"{txt.stem}: {failures} refused vertex-arena allocations -- draws were dropped"
            )
        if list_bytes > 900_000:
            notes.append(f"{txt.stem}: display list at {list_bytes} bytes of the 1 MB buffer")
    return notes


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--burst", type=int, default=400, help="frame the capture window starts on")
    ap.add_argument("--frames", type=int, default=40, help="how many consecutive frames to capture")
    ap.add_argument(
        "--hold",
        action="append",
        default=[],
        help="a script line, e.g. --hold '90 x' --hold '400 xl' (repeatable)",
    )
    ap.add_argument(
        "--node",
        type=int,
        help="drop the car on this centreline node (0-2620) instead of driving from the start",
    )
    ap.add_argument("--kph", type=int, default=90, help="speed to place the car at")
    ap.add_argument(
        "--mode",
        type=int,
        default=0,
        help="render override to bisect a cause: 1 no cull, 2 no depth, 3 no fog, 5 no sky, "
        "6 road only, 7 terrain only, 8 no HUD (see render::DEBUG_MODES)",
    )
    ap.add_argument("--label", default="run", help="names the output directory under captures/glitch")
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--timeout", type=int, default=900, help="backstop only; the guest exits itself")
    ap.add_argument("--graphics", default="software", help="software is deterministic; try opengl to compare")
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--scan-only", type=Path, help="re-scan an already harvested directory")
    args = ap.parse_args()

    if args.scan_only:
        out = args.scan_only
        frames = sorted(out.glob("SHOT*.png"))
    else:
        headless = Path(os.environ.get("PPSSPP_HEADLESS", DEFAULT_HEADLESS))
        if not headless.exists():
            sys.exit(f"no PPSSPPHeadless at {headless} -- see docs/diagnostics.md")
        anglezero = Path(os.environ.get("PPSSPP_MEMSTICK", DEFAULT_MEMSTICK)) / "ANGLEZERO"
        out = args.out or Path("captures/glitch") / args.label

        if not args.no_build:
            build()
        write_script(
            anglezero, args.burst, args.frames, args.hold, args.node, args.kph, args.mode
        )
        clear_stale(anglezero)
        run_headless(headless, args.timeout, args.graphics)
        frames = harvest(anglezero, out)
        # So a later --scan-only can still say which run frame a capture was, and so the run is
        # reproducible from the directory alone.
        (out / "run.txt").write_text(
            f"burst {args.burst} {args.frames}\n"
            f"node {args.node}\nkph {args.kph}\ngraphics {args.graphics}\n"
            f"hold {args.hold}\n"
        )

    burst = args.burst if not args.scan_only else read_burst(out)
    log(f">> {len(frames)} frames in {out}")
    if not frames:
        sys.exit(
            "no frames captured. The guest writes them itself, so either it never reached the "
            "burst frame (raise --timeout, or lower --burst) or it is not a --features harness build."
        )

    findings = scan_frames(frames)
    notes = scan_trace(out) + scan_counters(out)

    log("")
    if findings:
        log(f"== {len(findings)} frame(s) with a one-frame blink, worst first")
        for f in findings[:12]:
            x0, y0, x1, y1 = f["box"]
            sheet = mark(frames, f, out)
            at = frame_of(out / f["frame"], burst)
            where = f"frame {at}" if at is not None else f["frame"]
            log(
                f"   {f['frame']} ({where})  peak {f['peak']}/255  {f['tiles']} tiles  "
                f"x {x0}-{x1} y {y0}-{y1}  -> {sheet.name}"
            )
    else:
        log("== no one-frame blinks above threshold")
    if notes:
        log(f"== {len(notes)} note(s) from the draw tally")
        for n in notes[:20]:
            log(f"   {n}")
    else:
        log("== draw tally clean: no holes, no blinked chunk sets, no dropped draws")


if __name__ == "__main__":
    main()
