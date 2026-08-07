# PSP hardware notes

Things the hardware does that no emulator here reproduces, each of which cost a day.

## Two traps worth knowing about

Both of these cost real debugging time, and neither fails loudly.

**`sceGumLookAt` does nothing in rust-psp 0.3.13.** Its helper `gum_look_at` shadows its own
`&mut` output parameter with a local:

```rust
let mut mat = gum_mult_matrix(mat, &t);   // new local, not the caller's matrix
gum_translate(&mut mat, &ieye);
```

so the caller's matrix is never written and the view matrix stays identity. The world still draws —
it is rendered with an identity model matrix — but everything is positioned as though the camera
sat at the world origin, and anything with a model transform (the car) lands somewhere else
entirely or off-screen. `src/math.rs` builds the view matrix instead, checked by `tests/matrix.rs`,
and uploads it with `sceGumLoadMatrix`. That matrix must be 16-byte aligned or the VFPU's `lv.q`
faults.

Related: rust-psp creates its VFPU matrix context lazily, but only inside `sceGumLoadIdentity` and
`sceGumLoadMatrix`. Every other `sceGum*` entry point calls `get_context_unchecked`, which hits an
`unreachable` — surfacing as a bare break instruction, not a panic message. `psp_main` touches
`sceGumLoadIdentity` once during setup so later code can start with `sceGumMatrixMode`.

**The GE reads vertex data by pointer, and sooner than you think.** `sceGumDrawArray` only queues
the pointer, so building vertices in a stack local — or reusing one static buffer for several draws
in a frame — is a use-after-free that PPSSPP often survives and hardware will not. Everything
dynamic goes through the bump arena in `src/psp/scratch.rs`, which lives for the whole frame.

Lifetime is only half of it. In `GuContextType::Direct` the hardware does **not** wait for
`sceGuFinish`: every `sceGumDrawArray` ends in `send_command_i_stall`, which advances the display
list's stall address and kicks the GE into executing that draw immediately. So there is no safe
point at which to write the data cache back — by the time the frame ends, the GE has already read
every buffer the frame referenced, while the writes were still sitting in cache.

The arena therefore hands out **uncached** pointers, the same trick `sceGuStart` uses for the
display list itself, so the data is in memory before the draw pointing at it is ever issued. It
costs about 0.3 ms a frame and cannot be got wrong later by a call site that forgets to flush.
Statics the GE reads (the meshes, the font texture, the projected minimap) are written once at boot
and flushed with `sceKernelDcacheWritebackAll` afterwards.

Getting this wrong does not fail cleanly: it reads as text losing its last few characters, sprites
appearing at wild coordinates, and geometry flickering — intermittently, and only on hardware.

## Performance

Measured in PPSSPP with the emulated microsecond clock, over a full-throttle descent. This covers
the CPU side — simulation plus building the display list — and not GE rasterisation, which is a
separate unit and the thing this cannot measure from here.

| Build | Typical frame | Worst seen | Budget at 30 fps |
|---|---|---|---|
| debug | ~7 ms | 7.7 ms | 33 ms |
| release | ~1.1 ms | 9.7 ms | 33 ms |

The worst case is not a startup transient — it persists with the first ninety frames excluded. It
is the fixed-timestep accumulator catching up after a slow frame, which is capped at 40 substeps
and so cannot run away.

Static allocation is ~3.4 MB of `.bss`, against the PSP's 24 MB. Nothing is allocated per frame:
the effect pools are fixed-size ring buffers and every dynamic vertex comes from a frame-lived
arena with a known ceiling.
