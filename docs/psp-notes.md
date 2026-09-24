# PSP hardware notes

Pitfalls that PPSSPP does not reproduce or that fail silently. All SDK references are to rust-psp
0.3.13.

## `sceGumLookAt` does nothing

`gum_look_at` shadows its `&mut` output with a local, so the caller's matrix is never written and
the view stays identity:

```rust
let mut mat = gum_mult_matrix(mat, &t);   // new local, not the caller's matrix
gum_translate(&mut mat, &ieye);
```

**Workaround:** `src/math.rs` builds the view matrix (tested in `tests/matrix.rs`) and uploads it
with `sceGumLoadMatrix`. The matrix must be 16-byte aligned, or the VFPU `lv.q` faults.

## Gum context is created lazily

rust-psp creates its VFPU matrix context only inside `sceGumLoadIdentity` and `sceGumLoadMatrix`.
Every other `sceGum*` call hits `unreachable` first, which surfaces as a bare `break` instruction
rather than a panic. `psp_main` calls `sceGumLoadIdentity` once during setup.

## `sceGumPushMatrix` / `sceGumPopMatrix` are mismatched

Push advances the stack pointer then saves; pop retreats then loads. What is popped is never what
was pushed. It only appears to work once an earlier draw has synced the right matrix into the slot
below.

**Workaround:** `draw_one_car` in `src/psp/render.rs` rebuilds the full transform before each mesh
and does not use the matrix stack.

**Symptom:** the asset is valid offline but the car renders as a few stray pixels, depending on
mesh order.

## The GE reads vertex data immediately

`sceGumDrawArray` queues a pointer, and in `GuContextType::Direct` each draw ends in
`send_command_i_stall`, which starts the GE on it at once. So:

- Vertex data must outlive the frame. A stack local, or one static buffer reused across draws, is a
  use-after-free that PPSSPP tolerates and hardware does not.
- Vertex data must already be in memory, not in the data cache, when the draw is issued. There is no
  later point at which a cache writeback is safe.

**Workaround:** all per-frame geometry comes from the bump arena in `src/psp/scratch.rs`, which
lives for the whole frame and returns **uncached** pointers (as `sceGuStart` does for the display
list). Cost: ~0.3 ms/frame. Static GE data (meshes, font, minimap) is written once at boot and
flushed with `sceKernelDcacheWritebackAll`.

**Symptoms:** truncated text, sprites at wild coordinates, flickering geometry — intermittently and
only on hardware.

## Performance

CPU time (simulation + display-list build) in PPSSPP over a full-throttle descent. GE
rasterisation is not included.

| Build | Typical | Worst | Budget (30 fps) |
|---|---|---|---|
| debug | ~7 ms | 7.7 ms | 33 ms |
| release | ~1.1 ms | 9.7 ms | 33 ms |

The worst case is the fixed-timestep accumulator catching up after a slow frame; it is capped at
40 substeps.

## Memory

Nothing is allocated per frame. Effect pools are fixed-size ring buffers and dynamic vertices come
from the frame arena.

| Region | Release | `devtools` |
|---|---|---|
| Car arena (`src/psp/car.rs`) | 2 slots × 1.25 MB = 2.5 MB | 5 slots = 6.25 MB |
| Display list | 1 MB | 1 MB |

The PSP has 24 MB of user memory.
