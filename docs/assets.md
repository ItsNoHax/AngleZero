# Assets

XMB presentation and music. Cars have their own pipeline; see [Cars](cars.md).

## Layout

```
assets/
  ICON0.png, PIC1.png   XMB icon and background
  PIC0.png              XMB info panel, drawn over PIC1
  ICON1.PMF             XMB animated icon (generated, committed)
  ICON1_source.mp4      animated icon source
  SND0.AT3              XMB music, looping (generated, committed)
  SND0_source.wav       music source
  configs/*.toml        car configs (committed)
  compiled/*.azcar      compiled cars (committed)
  source/*.glb          car source models (not in git)
```

## XMB slots

Configured in `Psp.toml` and packed into the EBOOT by `cargo psp`.

| Slot | File | Format |
|---|---|---|
| `ICON0` | `assets/ICON0.png` | 144 × 80 PNG, 24-bit RGB |
| `PIC1` | `assets/PIC1.png` | 480 × 272 PNG, 24-bit RGB |
| `ICON1` | `assets/ICON1.PMF` | 144 × 80 H.264 PMF, 29.97 fps, 4.8 s seamless loop |
| `PIC0` | `assets/PIC0.png` | 310 × 180 PNG, 32-bit RGBA |
| `SND0` | `assets/SND0.AT3` | ATRAC3 LP4 (66 kbps), 44.1 kHz stereo, RIFF |

ICON0 and PIC1 must have no alpha channel; some firmwares and packers reject it. PIC0 keeps its
alpha: it is an overlay, and the slot is meant to be transparent. To convert RGBA art:

```bash
magick in.png -background black -alpha remove -alpha off -define png:color-type=2 out.png
```

To list what a built EBOOT actually contains:

```bash
python3 - target/mipsel-sony-psp/release/angle-zero.EBOOT.PBP <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
o = struct.unpack('<8I', d[8:40])
for i, n in enumerate(['PARAM.SFO','ICON0.PNG','ICON1.PMF','PIC0.PNG','PIC1.PNG','SND0.AT3','DATA.PSP','DATA.PSAR']):
    size = (o[i+1] if i+1 < 8 else len(d)) - o[i]
    if size:
        print(f'{n:<12}{size:>9} bytes')
PY
```

## Animated icon

The XMB plays `ICON1.PMF` in place of `ICON0` while the game is selected. `ICON1_source.mp4` is
its source: 144 frames at 29.97 fps, two passes of a 2.4 s animation, so it loops without a seam.

```bash
python3 scripts/make_pmf.py assets/ICON1_source.mp4 assets/ICON1.PMF
```

The script needs `ffmpeg` with `libx264`. ffmpeg can read PMF but cannot write it, so the script
re-encodes the video and writes the container itself; its docstring lists the format rules. It is
ported unchanged from SSB64PSP, whose icons it produced animate on a PSP Slim with firmware 6.61.
ffmpeg decoding the result does not prove the XMB will play it: the console's decoder is stricter,
and only a physical PSP shows the animation.

```bash
ffprobe -v error -count_frames -show_entries stream=profile,level,nb_read_frames assets/ICON1.PMF
```

## Music

`SND0_source.wav` is a 20 s seamless loop (144 BPM, Am–F–C–G), 44.1 kHz 16-bit stereo. Do not
fade or trim the ends.

```bash
scripts/encode_music.sh
```

The script encodes with [`pspbuild`](https://github.com/ItsNoHax/pspbuild), a workspace
dependency, through `tools/anglezero-eboot`. It needs nothing installed beyond cargo. It refuses to
write a file that pspbuild's validator finds anything wrong with, warnings included, or one that
does not loop.

### XMB requirements

ffmpeg can decode ATRAC3 but not encode it. A playable, looping `SND0.AT3` must have:

| Property | Required |
|---|---|
| Container | RIFF/WAVE: `fmt `, `fact`, `smpl`, `data` |
| Mode | LP4: 66144 bps, block align 192, joint stereo (LP2 is unproven) |
| `bands_coded` per frame | 3 bands (first frame byte `A2`); a frame coding four (`A3`) makes the XMB silent |
| Loop | `fact` = (loop length, 1024) and a `smpl` loop from 1024, forever. Without both, the XMB plays the track once and stops; `smpl` alone plays nothing |
| Length | at most 55 s |

pspbuild low-passes the source at 15.5 kHz, since the fourth band, where anything higher would go,
is never coded. The loop is exactly the source's length, so the source must be a seamless loop.
Its [AUDIO.md](https://github.com/ItsNoHax/pspbuild/blob/main/docs/AUDIO.md) has the evidence for
each rule.

Check a file, or the copy inside a built EBOOT:

```bash
cargo run --release -q -p anglezero-eboot -- target/mipsel-sony-psp/release/angle-zero.EBOOT.PBP /tmp/EBOOT.PBP
```

Its output includes `VALID: SND0.AT3 ...` and, if anything is off, a `WARNING: SND0.AT3:` line.

### Debugging playback

A `devtools` build tests the file with the console's own decoder at boot and writes
`ms0:/ANGLEZERO/ATRAC.TXT` (`src/psp/atractest.rs`). Healthy output:

```
sceAtracSetDataAndGetID 2 (0x00000002)
bitrate rc 0 value 66
decode rc 0 (0x00000000) samples 955
first frame peak 25084
```

A file can pass this and still be silent in the XMB. Comparing byte-for-byte against a `SND0.AT3`
from a game whose music plays is the most reliable check.

## Art direction

Night touge: moonlit ridge road, sodium lamp pools, red tail-light smear on the apex.

| Use | Colour |
|---|---|
| Sky | `#04060e` → `#132247` |
| Moon | `#e8efff` |
| Sodium lamps | `#ffc478` |
| Tail lights | `#ff3a3a` |
| Accent | `#ff783c` |

Wordmark in a bold condensed grotesque with added tracking; "SEKIRA PASS" subtitle at 6.5 px on the
icon.
