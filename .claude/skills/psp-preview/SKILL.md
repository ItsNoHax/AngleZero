---
name: psp-preview
description: Build this PSP homebrew project and capture what it renders as an image, using PPSSPPHeadless (no window, no X server), optionally with a button held. Use whenever asked to see, show, view, preview, or screenshot the PSP screen or emulator output, or to verify a rendering or input change visually.
---

# Previewing PSP output headlessly

Captures the emulated PSP framebuffer to an image without opening a window, so rendering changes can
be verified visually.

## Prerequisites

A `PPSSPPHeadless` binary is required. Resolve it once per session, honouring an override so this
does not assume where the emulator was built:

```bash
HEADLESS="${PPSSPP_HEADLESS:-$HOME/.local/src/ppsspp/build-headless/PPSSPPHeadless}"
```

The examples below all use `$HEADLESS`. If it is missing, stop and point the user at the
"Headless screenshots → One-time setup" section of `README.md` (a ~2 GB checkout and a 15–30 minute
build — never start it without asking first).

## How capture actually works

`--screenshot-save` is **pull-based**. Headless writes the file only when the emulated program calls
`sceIoDevctl("emulator:", 0x20, ...)` (`EMULATOR_DEVCTL__EMIT_SCREENSHOT`) — that is the
`emit_screenshot()` helper in `src/main.rs`, called every 60 frames. Headless captures **nothing** on
its `--timeout` path. So if no BMP appears, check that the program still emits, before suspecting the
emulator.

Each emit overwrites the file, so the saved image is whatever state the *last* emit saw.

## Idle capture

1. Build from the project root:

   ```bash
   cargo psp
   ```

   Headless runs the `.prx`, not the `.pbp`: `target/mipsel-sony-psp/debug/angle-zero.prx`. The
   `rust-lld` `abicalls` / `discarded section` warnings are expected and harmless (upstream
   rust-psp#203); a build ending in `Saved to ".../EBOOT.PBP"` succeeded.

2. Capture, writing temp files to the scratchpad rather than the repo:

   ```bash
   "$HEADLESS" \
       --graphics=software \
       --screenshot-save=<scratchpad>/psp.bmp \
       --timeout=5 \
       target/mipsel-sony-psp/debug/angle-zero.prx
   ```

   `--timeout` is **required** — the main loop never exits, so headless would otherwise run forever.
   `--graphics=software` is deterministic across runs, unlike the GPU backends, so unchanged code
   gives byte-identical captures. Keep it unless specifically testing a hardware backend.

3. Convert and view. The BMP is 512×272 (framebuffer stride) with 32 px of unused padding on the
   right, so crop to the visible area:

   ```bash
   ffmpeg -y -i <scratchpad>/psp.bmp -vf crop=480:272:0:0 -update 1 <scratchpad>/psp.png
   ```

   `-update 1` is required or ffmpeg treats the output as an image sequence and errors. Then `Read`
   the PNG, and `SendUserFile` with `display: "render"` when the user should see it too.

## Capture with a button held

Headless has no input device, but `--debugger=<port>` starts the WebSocket debugger, and
`input.buttons.send` feeds `__CtrlUpdateButtons` — the same HLE path real input uses — so the guest
genuinely sees the press. `scripts/psp_input.py` wraps it (no third-party deps):

```bash
"$HEADLESS" \
    --graphics=software --debugger=9333 \
    --screenshot-save=<scratchpad>/cross.bmp --timeout=10 \
    target/mipsel-sony-psp/debug/angle-zero.prx &
HL=$!
sleep 3
python3 scripts/psp_input.py 9333 cross &
PY=$!
wait $HL; kill $PY 2>/dev/null
```

Timing is the part that bites:

- **Hold the button through headless's `--timeout`.** Releasing early lets a later idle frame
  overwrite the held capture, and the result looks exactly like the press never registered.
- `--debugger` implies `startBreak`; the core stays halted until `cpu.resume`. The script sends it.
- Sleep a few seconds before connecting so the debugger is listening, and give `--timeout` enough
  headroom (~10 s) for the software renderer to reach an emit while held.

Pass `--verbose` to the script to see every debugger response; failed requests come back as
`{"event":"error",...}` and are printed even without it.

## Interpreting what you see

PSP clear colours are **ABGR**, not ARGB — `0xff4040e0` is R=0xe0 (red), and `0xff302010` is R=0x10
B=0x30 (dark navy). Check byte order before concluding a colour is wrong.

A completely black frame usually means the GU pipeline is misconfigured rather than that nothing was
drawn. `sceGuClear` rasterises through the transform pipeline, so it silently produces nothing if
`sceGuOffset` / `sceGuViewport` / `sceGuScissor` were not set up during init.

## Interactive fallback

For anything needing live human input, run the GUI build instead:

```bash
flatpak run org.ppsspp.PPSSPP "$PWD/target/mipsel-sony-psp/debug/EBOOT.PBP"
```
