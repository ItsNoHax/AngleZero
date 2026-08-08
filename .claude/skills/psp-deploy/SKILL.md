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
| `PSP/GAME/AngleZero/EBOOT.PBP` | `cargo psp --release` | What actually gets played. ~756 KB. |
| `PSP/GAME/AngleZeroDev/EBOOT.PBP` | `cargo psp --release --features devtools` | START toggles the counter overlay, SELECT saves a frame burst. ~958 KB. |

"Push the new builds" means **both**. Only the `EBOOT.PBP` goes in each folder — `Psp.toml` bundles
the icon, background and music into the PBP itself, so there is nothing else to copy.

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
