---
name: psp-deploy
description: Install this project's builds onto the real PSP over USB mass storage, and pull on-device captures back off it. Use whenever asked to push, deploy, install, copy, or flash a build to the PSP or the memory stick, or to fetch frames and counters the console recorded.
---

# Deploying to the console

## Finding the stick

The PSP has to be in USB mode (XMB → Settings → USB Connection) with the cable in. It then automounts
as removable storage. `scripts/psp_pull.sh` already has the detection logic and honours `PSP_MOUNT`;
reuse it rather than hardcoding a path.

To confirm what mounted is actually the console and not some other USB disk:

```bash
lsblk -o NAME,SIZE,FSTYPE,VENDOR,MODEL,MOUNTPOINT | grep -i 'psp\|MODEL'
```

The real thing reports `SONY` / `"PSP" Type A` and has `MEMSTICK.IND`, `PSP/`, and `SEPLUGINS/` at
its root. **Check this before copying anything.** The stick in use is 120 GB, which looks nothing like
the 2–32 GB people expect of a Memory Stick, so size is not the test — vendor and `MEMSTICK.IND` are.
Writing a build onto somebody's external drive because it was the only removable device is the
failure mode this check exists to prevent.

## The two slots

Both already exist on the stick, and the names are the established convention — match them, do not
invent new ones:

| Slot | Build | Why |
| --- | --- | --- |
| `PSP/GAME/AngleZero/EBOOT.PBP` | `cargo psp --release` | What actually gets played. ~765 KB. |
| `PSP/GAME/AngleZeroDev/EBOOT.PBP` | `cargo psp --release --features devtools` | START toggles the counter overlay, SELECT saves a frame burst. ~932 KB. |

"Push the new builds" means **both**. The `EBOOT.PBP` is the only *build* artifact in each folder —
`Psp.toml` bundles the icon, background and music into the PBP itself — but the cars are separate
files and have to go on too.

## The cars

```bash
STICK="${PSP_MOUNT:?point this at the stick psp_pull.sh found}"
mkdir -p "$STICK/PSP/GAME/AngleZero/CARS"
cp assets/compiled/*.azcar "$STICK/PSP/GAME/AngleZero/CARS/"
```

One directory under the release slot, read by **both** builds: the path is absolute in
`src/psp/car.rs`, so `AngleZeroDev` reads the same files and there is never a second copy to keep in
step. The game loads every `.azcar` it finds there, so adding a car to the console is copying a file
— no rebuild, and the title screen offers it with L/R.

Push them whenever `anglezero-asset convert` has run, which is not the same occasion as a code
change. A stale car on the stick is the failure that looks like a rendering regression: the binary is
new, the model is not, and nothing on screen says so.

Without them the game boots to a title screen reading `car asset not found on the memory stick` and
no car. That is the message to expect on a stick that has only ever had builds copied to it.

There is a ceiling, and it is the arena rather than the twelve slots behind it. Cars are loaded
into a fixed 6 MB arena (`ARENA_BYTES` in `src/psp/car.rs`); the seven current ones come to 5.45 MB,
leaving room for about one more. A car that does not fit is refused with `not enough room for
every car on the stick` rather than silently dropped, and the rest still load — so the symptom is
one car missing from the title screen's rotation and a message under the car's name, not a build
that fails to boot. If that happens, either raise the arena, lower a triangle budget, or take a car
off.

Both builds write to the same `target/mipsel-sony-psp/release/angle-zero.EBOOT.PBP`, so build and
copy one, then build and copy the other. Doing both builds first silently installs the same binary
twice.

Guard the clean one before it goes on. `devtools` being off by default is a promise the build makes,
not one it checks:

```bash
for m in "NO CULL" "NO DEPTH" "ms0:/ANGLEZERO/" "ATRAC.TXT" "TRACE.TXT" "SCRIPT.TXT"; do
    strings -a target/mipsel-sony-psp/release/angle-zero.prx | grep -qF "$m" && echo "LEAKED: $m"
done
```

`scripts/release.sh` runs this same check plus the tests, and is the right tool when the goal is a
distributable archive rather than a copy onto the stick.

Finish with `sync`. vfat writes sit in the page cache and the console gets unplugged.

## Never push the harness build

`--features harness` ignores the controller entirely and drives itself from a script file. It is for
the headless glitch hunt (see the `psp-glitch` skill), and on hardware it would look like a game that
does not respond to input. It implies `devtools`, so the guard above catches it.

## Pulling captures back

```bash
scripts/psp_pull.sh
```

Converts `ms0:/ANGLEZERO/*.BMP` to PNG in `captures/`, prints the per-shot counters, and summarises
each 600-frame trace — including gaps, which is the part worth reading. `SCF` (refused vertex-arena
allocations) being non-zero means draws were silently dropped.

The stick usually has captures sitting on it from an earlier session. They are evidence: copy them
off, do not clear them to tidy up.
