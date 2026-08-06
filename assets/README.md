# AngleZero — PSP EBOOT assets

| Slot | File | Format | Status |
|---|---|---|---|
| ICON0 | `ICON0.PNG` | 144 × 80 PNG | ready (see 24-bit note) |
| PIC1 | `PIC1.PNG` | 480 × 272 PNG | ready (see 24-bit note) |
| SND0 | `SND0.AT3` | ATRAC3, 66/105/132 kbps, 44.1 kHz stereo | encode from `SND0_source.wav` |

## 1. Force 24-bit (no alpha) PNG

The generated PNGs are RGBA. Every pixel is opaque, but PSP packers
(and some firmwares) want no alpha channel:

```sh
magick ICON0.png -background black -alpha remove -alpha off -define png:color-type=2 ICON0.PNG
magick PIC1.png  -background black -alpha remove -alpha off -define png:color-type=2 PIC1.PNG
# or: pngcrush -c 2 in.png out.png
```

## 2. Encode SND0.AT3

`SND0_source.wav` is a 20.00 s seamless loop (144 BPM, 12 bars, Am–F–C–G),
44.1 kHz / 16-bit / stereo — the exact format ATRAC3 encoders expect.

```sh
# Official Sony encoder (Windows / wine)
at3tool -e -br 66 SND0_source.wav SND0.AT3

# ffmpeg alternative
ffmpeg -i SND0_source.wav -c:a atrac3 -b:a 66k -ar 44100 -ac 2 SND0.AT3
```

Notes:
- 66 kbps is the safest bitrate for XMB background music; 105 kbps if size allows.
- SND0 must be ≤ ~55 s; 20 s is fine and loops cleanly (delay/reverb tails wrap
  around the loop point by design, so there is no seam).
- Trim to an exact sample count if your packer complains — do not fade the ends,
  the loop is already continuous.

## 3. Pack

```sh
pack-pbp EBOOT.PBP PARAM.SFO ICON0.PNG NULL NULL PIC1.PNG SND0.AT3 DATA.PSP NULL
```

Slot order: `PARAM.SFO, ICON0.PNG, ICON1.PMF, PIC0.PNG, PIC1.PNG, SND0.AT3, DATA.PSP, DATA.PSAR`.

## Art direction

Night touge, moonlit ridge road, sodium lamp pools, red taillight smear on the
apex line. Palette: `#04060e` → `#132247` sky, `#e8efff` moon, `#ffc478` lamps,
`#ff3a3a` taillights, `#ff783c` accent rule. Wordmark set in bold condensed
grotesque, +tracking; subtitle "SEKIRA PASS" at 6.5 px on the icon (smallest
legible size at XMB scale).
