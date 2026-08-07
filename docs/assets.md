# Assets

What the XMB shows for the game, and how the music is made.

## The three slots

`Psp.toml` points at them and `cargo psp` bakes them into the EBOOT:

| Slot | File | Format |
|---|---|---|
| `ICON0` | `assets/ICON0.png` | 144 × 80, 24-bit PNG |
| `PIC1` | `assets/PIC1.png` | 480 × 272, 24-bit PNG |
| `SND0` | `assets/SND0.AT3` | ATRAC3, 66 kbps, 44.1 kHz stereo |

Both PNGs are 24-bit with no alpha channel. The source art is RGBA and every pixel of it is opaque,
but some firmwares and packers reject an alpha channel in these slots, so `assets/` holds converted
copies.

To check what actually ended up inside a built EBOOT rather than trusting the manifest:

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

## Music

`assets/SND0.AT3` is committed, because regenerating it means building two projects. To rebuild it
from `assets/SND0_source.wav`:

```bash
scripts/encode_music.sh
```

That clones and builds [atracdenc](https://github.com/dcherednik/atracdenc) at a pinned revision,
applies `scripts/patches/atracdenc-psp-bands.patch`, low-passes the source, encodes, and then
verifies the result frame by frame before accepting it.

### Why it is not just a one-line ffmpeg call

ffmpeg has **decoders** for ATRAC3 but no encoder, so it cannot produce this file at all. atracdenc
can, but its output is rejected outright by the PSP — the XMB simply plays nothing.

An ATRAC3 frame opens with a six-bit unit id, then two bits of `bands_coded`. Every frame of a
stock PSP `SND0.AT3` carries **2**, meaning three QMF bands. atracdenc always writes **3**, coding
a fourth band above 16.5 kHz, and the console's decoder refuses the stream rather than ignoring the
extra band. The tell is the first byte of every frame: `A2` in a file that plays, `A3` in one that
does not. The patch caps the count; the low-pass at 15.5 kHz then keeps the encoder from spending
bits on a band that is going to be discarded anyway.

Nothing about this is visible from a PC. ffmpeg decodes the rejected file perfectly, the RIFF header
is byte-identical to files that work, and `sceAtrac` accepts it. That is why the encode script
checks every frame and refuses to emit a file the console would ignore:

```
>> 863 frames, all with bands_coded=2
```

The `fact` chunk is also dropped. atracdenc writes a sample count of exactly frames × 1024, leaving
no slack for ATRAC3's decoder delay; Sony's own files either claim fewer samples than they carry or
omit the chunk entirely. The chunk is optional, so the header now matches a file known to play:
`fmt`, then `data`, nothing else.

### Diagnosing a file that will not play

The dev build asks the console's own decoder at boot and writes the answer to
`ms0:/ANGLEZERO/ATRAC.TXT` — see `src/psp/atractest.rs`. A healthy report:

```
sceAtracSetDataAndGetID 2 (0x00000002)
bitrate rc 0 value 66
decode rc 0 (0x00000000) samples 955
first frame peak 25084
```

Note that a file can pass all of this and still be silent in the XMB, which is what happened here.
Comparing against an `SND0.AT3` extracted from a game that does play is worth more than any single
check.
