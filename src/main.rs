//! PSP binary shell. All game logic lives in the `angle_zero` library crate so it stays
//! host-testable; this file only exists to drive it from the PSP SDK.
//!
//! On any target other than the PSP this compiles down to an empty `main`, which is what lets
//! `cargo test` build the workspace without the `psp` crate.

#![cfg_attr(target_os = "psp", no_std)]
#![cfg_attr(target_os = "psp", no_main)]

#[cfg(target_os = "psp")]
mod psp;

#[cfg(not(target_os = "psp"))]
fn main() {
    eprintln!("angle-zero is a PSP binary; build it with `cargo psp`.");
}
