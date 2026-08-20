#!/usr/bin/env python3
"""Check that every car's lamps are where lamps go.

The lamp detector finds lenses by name where a model names them and by position where it does not,
and the fallback is the part worth checking: it picks whatever emissive geometry is nearest the
front or the back, which on some models is the dashboard lighting, or one merged mesh holding every
lamp on the car at once. When it goes wrong it goes wrong quietly — the car still drives, the lamps
still light, they are just somewhere that is not the corner of the car. The Golf R32 shipped with
its tail lights under the driver's elbow and nobody noticed until they were seen in the dark.

So this asks two questions of the compiled car, both of which have an obviously right answer:

* Is each lamp in the right end of the car? A headlight belongs in the front 30% and a tail light
  in the rear 30%, measured against that car's own length.
* Is each pair mirrored? Left and right should be the same distance from the centreline and the
  same distance along it. A pair that is not mirrored means the detector matched two different
  things and called them a pair.

    scripts/lights_check.py

Fix what it finds with `[lights.headlight_left]` and friends in the car's config — `node` to point
at the real lens when the model has one, `at` to place it by hand when every lamp is one mesh.
"""

import glob
import os
import struct
import sys

KIND = {0: "headlight", 1: "tail", 2: "brake", 3: "reverse", 4: "indicator"}
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# How far into the car a lamp may sit before it is not at the end of it any more.
END = 0.30
# How far a pair may be from mirroring each other, in metres. Bodywork is not perfectly symmetric —
# the Charger's hidden headlights genuinely differ by 8 cm — so this is loose enough to allow a
# model its own asymmetry and tight enough to catch two unrelated parts called a pair.
PAIR = 0.10


def lamps(path):
    b = open(path, "rb").read()
    lo = struct.unpack_from("<fff", b, 32)
    hi = struct.unpack_from("<fff", b, 44)
    at = struct.unpack_from("<I", b, 104)[0]
    count = struct.unpack_from("<H", b, 108)[0]
    out = []
    for i in range(count):
        o = at + i * 32
        out.append((b[o], struct.unpack_from("<fff", b, o + 4)))
    return lo, hi, out


def check(path):
    name = os.path.basename(path)[: -len(".azcar")]
    lo, hi, found = lamps(path)
    length = hi[2] - lo[2]
    faults = []

    for kind, at in found:
        if kind == 0:
            if at[2] < hi[2] - length * END:
                faults.append(f"{KIND.get(kind, kind)} at z={at[2]:+.2f} on a car whose nose is {hi[2]:+.2f}")
        else:
            if at[2] > lo[2] + length * END:
                faults.append(f"{KIND.get(kind, kind)} at z={at[2]:+.2f} on a car whose tail is {lo[2]:+.2f}")

    for kind in sorted({k for k, _ in found}):
        side = [at for k, at in found if k == kind]
        if len(side) != 2:
            continue
        a, b_ = side
        if abs(a[0] + b_[0]) > PAIR or abs(a[2] - b_[2]) > PAIR:
            faults.append(
                f"{KIND.get(kind, kind)} pair is not mirrored: "
                f"({a[0]:+.2f},{a[2]:+.2f}) and ({b_[0]:+.2f},{b_[2]:+.2f})"
            )

    return name, len(found), faults


def main():
    paths = sorted(glob.glob(os.path.join(ROOT, "assets/compiled/*.azcar")))
    if not paths:
        sys.exit("no compiled cars; run scripts/cars.sh build")

    total = bad = 0
    for p in paths:
        name, count, faults = check(p)
        total += count
        for f in faults:
            bad += 1
            print(f"  {name:24} {f}")
        if count == 0:
            print(f"  {name:24} carries no lamps at all")

    print()
    if bad:
        print(f"{bad} lamp(s) in the wrong place. Name them in the car's [lights] section.")
        return 1
    print(f"All {total} lamps across {len(paths)} cars are at the right end and mirrored.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
