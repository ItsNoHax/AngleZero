#!/usr/bin/env python3
"""Golden screenshots of every compiled car, from all round and from above and below.

A car is judged by looking at it, and one or two hand-picked views hide most of what is wrong with
one: a hole under the sill only shows from low down, a roof seam only from above, a wheel turned
inside out only from the side it faces. So this orbits each car on a fixed grid — eight headings
round it, five heights from underneath to nearly overhead — and renders every variant the console
can draw with `azview`, which uses the console's own draw order, culling and 16-bit depth:

    lod0   the car as it is driven
    lod1…  each lower level the file carries
    sil    the flat stand-in drawn while the car is still streaming in

Two more sheets per car answer questions a whole-car grid cannot:

    detail.png    each wheel alone-ish and large from outboard and from low in front, then the
                  cabin through the side glass, `--only interior`, `--no-cull` and `--no-tex`
                  views, which is where rims through tyres, missing interiors, holes culling
                  opens and wrong textures show
    match_<v>.png every lower variant against lod0 from every view of the grid: grey where both
                  draw, red where lod0 draws and the variant does not (missing geometry), blue
                  where the variant draws outside lod0. `match.txt` gives the numbers

One PNG per view goes under `<out>/<car>/<variant>/`, and one contact sheet per car and variant
(`<out>/<car>/<variant>.png`) lays the grid out with headings across and heights down, so a fault
that only exists from one side stands out against its neighbours.

    scripts/car_goldens.py                          every car, into captures/goldens
    scripts/car_goldens.py bmw_e36 toyota_ae86      just these
    scripts/car_goldens.py --out /tmp/new --compare captures/goldens
                                                    render, then report what changed

`--compare` is the golden half: render into a fresh directory against the same grid, and every view
that differs from the reference by more than `--threshold` of its pixels is listed and gets a diff
image beside it (red where the pixels moved). Rendering the previous commit's `.azcar` into one
directory and the current one into another is how to tell a regression from an old fault.

Every render is a few milliseconds, so the whole fleet takes seconds. Needs Pillow.
"""

import argparse
import re
import glob
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

try:
    from PIL import Image, ImageChops, ImageDraw, ImageFilter
except ImportError:
    sys.exit("this needs Pillow: pip install pillow")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CARGO = os.environ.get("CARGO", os.path.expanduser("~/.cargo/bin/cargo"))
AZVIEW = os.path.join(ROOT, "target", "release", "azview")

# Headings round the car, in azview's convention: 0 looks at the nose, 180 at the tail.
YAWS = [0, 45, 90, 135, 180, 225, 270, 315]
# Heights: underneath, level with the sills, chase-camera height, high, and nearly overhead. Not a
# full ±90, because the look-at's up vector degenerates straight above and below.
PITCHES = [-35, 0, 15, 40, 80]
CELL = (400, 280)
DIST = 5.0
LABEL = 14
BG = (200, 200, 210)
# How far lod0's outline is pulled in before asking whether a variant covers it, so the rim that
# decimation always shaves off is not reported as missing geometry. Same idea as silhouette_check.
ERODE = 5


def run_azview(car, out, extra, log=False):
    args = [AZVIEW, car, out, "--dist", str(DIST), "--size", f"{CELL[0]}x{CELL[1]}", "--white",
            *extra]
    r = subprocess.run(args, capture_output=True, text=True)
    ok = r.returncode == 0 and os.path.exists(out)
    return (ok, r.stderr) if log else ok


def hubs(car, scratch):
    """Hub centre and radius of every wheel, from azview's listing."""
    _, listing = run_azview(car, os.path.join(scratch, "probe_hubs.png"), [], log=True)
    found = []
    for line in listing.splitlines():
        m = re.search(r"wheel \d+\s+(\S+)\s+hub \(\s*([-\d.]+),\s*([-\d.]+),\s*([-\d.]+)\) r([\d.]+)",
                      line)
        if m:
            found.append((m.group(1), [float(m.group(i)) for i in (2, 3, 4)], float(m.group(5))))
    return found


def variants(car, scratch):
    """Every level the file carries, then its silhouette if it has one."""
    found = []
    for lod in range(8):
        probe = os.path.join(scratch, f"probe_lod{lod}.png")
        if not run_azview(car, probe, ["--lod", str(lod)]):
            break
        found.append((f"lod{lod}", ["--lod", str(lod)]))
        if lod > 0 and open(probe, "rb").read() == open(
                os.path.join(scratch, "probe_lod0.png"), "rb").read():
            # azview falls back to lod0 for a level the file does not have; identical output means
            # we have walked off the end.
            found.pop()
            break
    if run_azview(car, os.path.join(scratch, "probe_sil.png"), ["--silhouette"]):
        found.append(("sil", ["--silhouette"]))
    return found


def view_name(yaw, pitch):
    return f"y{yaw:03d}_p{pitch:+03d}.png"


def sheet(paths, out):
    w, h = CELL
    im = Image.new("RGB", (w * len(YAWS), (h + LABEL) * len(PITCHES)), (40, 40, 44))
    draw = ImageDraw.Draw(im)
    for r, pitch in enumerate(PITCHES):
        for c, yaw in enumerate(YAWS):
            x, y = c * w, r * (h + LABEL)
            draw.text((x + 4, y + 1), f"yaw {yaw}  pitch {pitch}", fill=(230, 230, 230))
            p = paths.get((yaw, pitch))
            if p and os.path.exists(p):
                im.paste(Image.open(p).convert("RGB"), (x, y + LABEL))
    im.save(out)


def detail_views(car, scratch):
    """(label, azview args) for every close-up on the detail sheet."""
    views = []
    for name, (x, y, z), r in hubs(car, scratch):
        side = 90 if x > 0 else 270
        look = ["--look", f"{x},{y},{z}"]
        d = str(max(r * 4.5, 1.1))
        views.append((f"{name} outboard", [*look, "--yaw", str(side), "--pitch", "5", "--dist", d]))
        # From low in front of the wheel and a little outboard: the angle at which an alloy that
        # overruns its tyre, or a tyre turned inside out, is plainest.
        ahead = 0 if z > 0 else 180
        slant = ahead + (35 if (x > 0) == (ahead == 0) else -35)
        views.append((f"{name} low front",
                      [*look, "--yaw", str(slant % 360), "--pitch", "-8", "--dist", d]))
        # The wheel with nothing else drawn, so the body cannot hide where rim and tyre meet.
        alone = ["--only", name, *look, "--dist", str(max(r * 4.0, 1.0))]
        views.append((f"{name} alone outboard", [*alone, "--yaw", str(side), "--pitch", "0"]))
        views.append((f"{name} alone 3/4",
                      [*alone, "--yaw", str((side + (40 if side == 90 else -40)) % 360),
                       "--pitch", "15"]))
    views += [
        ("cabin left glass", ["--yaw", "270", "--pitch", "12", "--dist", "3.4"]),
        ("cabin right glass", ["--yaw", "90", "--pitch", "12", "--dist", "3.4"]),
        ("cabin windscreen", ["--yaw", "0", "--pitch", "28", "--dist", "3.6"]),
        ("interior only", ["--only", "interior", "--yaw", "240", "--pitch", "30"]),
        ("interior only top", ["--only", "interior", "--yaw", "0", "--pitch", "80"]),
        ("no-cull 3/4", ["--no-cull", "--yaw", "210", "--pitch", "14"]),
        ("no-cull under", ["--no-cull", "--yaw", "120", "--pitch", "-35"]),
        ("no-tex 3/4", ["--no-tex", "--yaw", "210", "--pitch", "14"]),
        ("textured 3/4", ["--yaw", "210", "--pitch", "14"]),
        ("no-tex front", ["--no-tex", "--yaw", "30", "--pitch", "10"]),
        ("textured front", ["--yaw", "30", "--pitch", "10"]),
    ]
    return views


def grid_sheet(cells, out, cols=8):
    """cells: [(label, png path)] laid out left to right, top to bottom."""
    w, h = CELL
    rows = (len(cells) + cols - 1) // cols
    im = Image.new("RGB", (w * cols, (h + LABEL) * rows), (40, 40, 44))
    draw = ImageDraw.Draw(im)
    for i, (label, p) in enumerate(cells):
        x, y = (i % cols) * w, (i // cols) * (h + LABEL)
        draw.text((x + 4, y + 1), label, fill=(230, 230, 230))
        if p and os.path.exists(p):
            im.paste(Image.open(p).convert("RGB"), (x, y + LABEL))
    im.save(out)


def mask(path):
    im = Image.open(path).convert("RGB")
    bg = Image.new("RGB", im.size, BG)
    return ImageChops.difference(im, bg).convert("L").point(lambda v: 255 if v else 0)


def count(m):
    return m.histogram()[255]


def match(ref_png, var_png):
    """Overlay of a variant on lod0, and the fraction of lod0's interior the variant leaves bare."""
    ref, var = mask(ref_png), mask(var_png)
    core = ref.filter(ImageFilter.MinFilter(ERODE))
    missing = ImageChops.subtract(core, var)
    spill = ImageChops.subtract(var, ref)
    im = Image.new("RGB", ref.size, (245, 245, 248))
    im.paste((150, 150, 158), mask=ImageChops.lighter(ref, var))
    im.paste((230, 30, 30), mask=missing)
    im.paste((40, 90, 230), mask=spill)
    return im, count(missing) / max(count(core), 1), count(spill) / max(count(ref), 1)


def render_car(car, out_root, pool):
    name = os.path.basename(car)[: -len(".azcar")]
    car_dir = os.path.join(out_root, name)
    os.makedirs(car_dir, exist_ok=True)
    found = variants(car, car_dir)
    for f in glob.glob(os.path.join(car_dir, "probe_*.png")):
        os.remove(f)
    for variant, extra in found:
        vdir = os.path.join(car_dir, variant)
        os.makedirs(vdir, exist_ok=True)
        jobs = {}
        for pitch in PITCHES:
            for yaw in YAWS:
                p = os.path.join(vdir, view_name(yaw, pitch))
                jobs[(yaw, pitch)] = (p, pool.submit(
                    run_azview, car, p, [*extra, "--yaw", str(yaw), "--pitch", str(pitch)]))
        paths = {k: p for k, (p, fut) in jobs.items() if fut.result()}
        sheet(paths, os.path.join(car_dir, f"{variant}.png"))

    ddir = os.path.join(car_dir, "detail")
    os.makedirs(ddir, exist_ok=True)
    views = detail_views(car, car_dir)
    futs = []
    for i, (label, extra) in enumerate(views):
        p = os.path.join(ddir, f"{i:02d}_{re.sub(r'[^a-z0-9]+', '_', label.lower())}.png")
        futs.append((label, p, pool.submit(run_azview, car, p, extra)))
    grid_sheet([(label, p if fut.result() else None) for label, p, fut in futs],
               os.path.join(car_dir, "detail.png"), cols=8)

    # Every lower variant against lod0, view by view.
    report = []
    for variant, _ in found[1:]:
        rows = []
        worst = (0.0, None)
        total_missing = total_spill = 0.0
        for pitch in PITCHES:
            for yaw in YAWS:
                v = view_name(yaw, pitch)
                im, miss, spill = match(os.path.join(car_dir, "lod0", v),
                                        os.path.join(car_dir, variant, v))
                im.save(os.path.join(car_dir, variant, v[:-4] + "_match.png"))
                total_missing += miss
                total_spill += spill
                if miss > worst[0]:
                    worst = (miss, v)
                rows.append((f"yaw {yaw} pitch {pitch}  miss {miss * 100:.1f}%",
                             os.path.join(car_dir, variant, v[:-4] + "_match.png")))
        n = len(PITCHES) * len(YAWS)
        report.append(f"{variant:<5} missing {total_missing / n * 100:5.2f}% mean, "
                      f"worst {worst[0] * 100:5.2f}% at {worst[1]}, "
                      f"spill {total_spill / n * 100:5.2f}% mean")
        grid_sheet(rows, os.path.join(car_dir, f"match_{variant}.png"))
    with open(os.path.join(car_dir, "match.txt"), "w") as f:
        f.write("\n".join(report) + "\n")
    for f in glob.glob(os.path.join(car_dir, "probe_*.png")):
        os.remove(f)
    return name, [v for v, _ in found], report


def compare(new_root, ref_root, threshold):
    changed, missing = [], []
    for new in sorted(glob.glob(os.path.join(new_root, "*", "*", "*.png"))):
        if new.endswith(("_diff.png", "_match.png")) or os.path.basename(new).startswith("probe"):
            continue
        rel = os.path.relpath(new, new_root)
        ref = os.path.join(ref_root, rel)
        if not os.path.exists(ref):
            missing.append(rel)
            continue
        a, b = Image.open(new).convert("RGB"), Image.open(ref).convert("RGB")
        if a.size != b.size:
            changed.append((rel, 1.0))
            continue
        diff = ImageChops.difference(a, b).convert("L").point(lambda v: 255 if v > 24 else 0)
        frac = sum(1 for v in diff.getdata() if v) / (a.size[0] * a.size[1])
        if frac > threshold:
            changed.append((rel, frac))
            red = Image.new("RGB", a.size, (255, 0, 0))
            Image.composite(red, a, diff).save(new[: -len(".png")] + "_diff.png")
    for rel, frac in changed:
        print(f"  changed  {rel}  {frac * 100:.2f}% of pixels")
    for rel in missing:
        print(f"  new      {rel}  (no reference)")
    print(f"{len(changed)} changed, {len(missing)} without a reference")
    return 1 if changed else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cars", nargs="*", help="car names or .azcar paths (default: every car)")
    ap.add_argument("--out", default=os.path.join(ROOT, "captures", "goldens"))
    ap.add_argument("--compare", metavar="REF", help="reference directory to diff against")
    ap.add_argument("--threshold", type=float, default=0.002,
                    help="fraction of pixels that may change before a view is reported")
    args = ap.parse_args()

    build = subprocess.run([CARGO, "build", "--release", "-q", "-p", "anglezero-asset",
                            "--bin", "azview"], cwd=ROOT)
    if build.returncode != 0:
        sys.exit("azview did not build")

    if args.cars:
        cars = [c if c.endswith(".azcar") else os.path.join(ROOT, "assets", "compiled",
                                                              f"{c}.azcar") for c in args.cars]
    else:
        cars = sorted(glob.glob(os.path.join(ROOT, "assets", "compiled", "*.azcar")))

    with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
        for car in cars:
            name, found, report = render_car(car, args.out, pool)
            print(f"  {name:<24} {' '.join(found)}")
            for line in report:
                print(f"      {line}")
    print(f"{len(cars)} cars -> {args.out}")

    if args.compare:
        sys.exit(compare(args.out, args.compare, args.threshold))


if __name__ == "__main__":
    main()
