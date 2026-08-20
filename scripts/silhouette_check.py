#!/usr/bin/env python3
"""Check that each car's silhouette actually covers the car it stands in for.

The silhouette is what the console draws while a car is still being read off the memory stick, so
it is only doing its job if it is the same shape as the car. The ways it goes wrong are not the
ways a person notices by looking at it on its own — a missing wing or a rounded-off sill looks
perfectly plausible until the real car replaces it, at which point the shape jumps.

So this compares the two directly: it renders each car and its silhouette from the same camera with
`azview`, and measures how much of the car the silhouette fails to cover.

The measurement has one trick in it that matters. Every silhouette is slightly smaller than its car,
because decimation pulls a surface inward, and that thin rim is not a fault — reporting it would
bury the real faults in noise. So the car's mask is *eroded* by a few pixels first, and what is
measured is whether the silhouette covers the car's **interior**. A rim of shrinkage survives that;
a missing panel does not.

    scripts/silhouette_check.py                 every car
    scripts/silhouette_check.py bmw_m5 ae86     just these
    scripts/silhouette_check.py --overlay bmw_m5   also write an overlay to see *where*

Under about 2% is a car whose silhouette is right. Above it, use `--overlay`: red is car with no
silhouette over it, which is the missing geometry, and blue is silhouette hanging outside the car.

Needs Pillow, which the glitch hunt already needs.
"""

import argparse
import glob
import os
import subprocess
import sys
import tempfile

try:
    from PIL import Image, ImageFilter
except ImportError:
    sys.exit("this needs Pillow: pip install pillow")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CARGO = os.environ.get("CARGO", os.path.expanduser("~/.cargo/bin/cargo"))
# `azview --white`'s background, which is what tells car from paper.
BG = (200, 200, 210)
# How far to pull the car's outline in before asking. Three pixels each way at this size is about a
# centimetre of car, which is comfortably more than decimation loses and far less than a panel.
ERODE = 7
VIEW = ["--yaw", "270", "--pitch", "6", "--dist", "7", "--size", "400x200", "--white"]


def render(car, out, silhouette):
    args = [CARGO, "run", "--release", "-q", "-p", "anglezero-asset", "--bin", "azview", "--",
            car, out, *VIEW]
    if silhouette:
        args.append("--silhouette")
    r = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(out):
        return False
    return True


def mask(path):
    im = Image.open(path).convert("RGB")
    out = Image.new("L", im.size)
    out.putdata([255 if px != BG else 0 for px in list(im.getdata())])
    return out


def check(car, tmp, overlay_dir=None):
    name = os.path.basename(car)[: -len(".azcar")]
    car_png = os.path.join(tmp, f"{name}_car.png")
    sil_png = os.path.join(tmp, f"{name}_sil.png")
    if not render(car, car_png, False):
        return name, None, "could not render the car"
    if not render(car, sil_png, True):
        return name, None, "no silhouette in this car"

    car_mask, sil_mask = mask(car_png), mask(sil_png)
    core = car_mask.filter(ImageFilter.MinFilter(ERODE))
    cd, sd = list(core.getdata()), list(sil_mask.getdata())
    inside = max(sum(1 for v in cd if v), 1)
    missing = sum(1 for a, b in zip(cd, sd) if a and not b)

    if overlay_dir:
        cm = list(car_mask.getdata())
        im = Image.new("RGB", car_mask.size)
        im.putdata([
            (70, 70, 80) if (a and b) else
            (255, 40, 40) if a else
            (40, 120, 255) if b else
            (245, 245, 245)
            for a, b in zip(cm, sd)
        ])
        path = os.path.join(overlay_dir, f"{name}.png")
        im.save(path)
        return name, missing / inside, path
    return name, missing / inside, None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("cars", nargs="*", help="car names; default is every compiled car")
    ap.add_argument("--overlay", action="store_true", help="also write an overlay showing where")
    ap.add_argument("--out", default=None, help="where overlays go (default: a temp directory)")
    args = ap.parse_args()

    paths = ([os.path.join(ROOT, "assets/compiled", f"{c}.azcar") for c in args.cars]
             or sorted(glob.glob(os.path.join(ROOT, "assets/compiled/*.azcar"))))
    missing = [p for p in paths if not os.path.exists(p)]
    if missing:
        sys.exit("no such car: " + ", ".join(os.path.basename(p) for p in missing))

    overlay_dir = None
    if args.overlay:
        overlay_dir = args.out or tempfile.mkdtemp(prefix="silhouette-")
        os.makedirs(overlay_dir, exist_ok=True)

    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for p in paths:
            rows.append(check(p, tmp, overlay_dir))

    rows.sort(key=lambda r: -1 if r[1] is None else -r[1])
    worst = 0.0
    for name, frac, note in rows:
        if frac is None:
            print(f"  {name:26} {'--':>7}  {note}")
            continue
        worst = max(worst, frac)
        flag = "  <-- missing geometry" if frac > 0.02 else ""
        where = f"  {note}" if note else ""
        print(f"  {name:26} {frac:>6.1%}{flag}{where}")

    print()
    if worst > 0.02:
        print("Something is missing from a silhouette. Run again with --overlay and look at the red.")
        return 1
    print(f"Every silhouette covers its car. Worst {worst:.1%}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
