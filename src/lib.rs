//! Angle Zero game core.
//!
//! Deliberately free of any PSP SDK dependency so the whole simulation — track generation,
//! vehicle physics, scoring, camera and screen flow — can be unit-tested on the host with
//! plain `cargo test`. The PSP binary in `main.rs` is a thin input + rendering shell over this.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod camera;
pub mod effects;
pub mod game;
pub mod hud;
pub mod math;
pub mod mesh;
pub mod save;
pub mod scoring;
pub mod track;
pub mod vehicle;
