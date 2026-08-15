//! Scratch: searches offline for an input script that puts the S15 into a fast, wide slide, so the
//! emulator only has to be run once. Delete when the screenshot is taken.

use angle_zero::azcar::Car;
use angle_zero::game::{Buttons, Game};
use angle_zero::track::{Track, NODE_COUNT};

const NONE: Buttons = Buttons {
    cross: false,
    circle: false,
    square: false,
    triangle: false,
    up: false,
    down: false,
    left: false,
    right: false,
    start: false,
    analog_x: 0.0,
};

const DT: f32 = 1.0 / 60.0;

#[derive(Clone, Copy)]
struct Step(u32, Buttons);

fn buttons_at(steps: &[Step], frame: u32) -> Buttons {
    let mut b = NONE;
    for s in steps {
        if s.0 <= frame {
            b = s.1;
        }
    }
    b
}

/// The script letters `psp/harness.rs` reads, for pasting straight into SCRIPT.TXT.
fn letters(b: Buttons) -> String {
    let mut s = String::new();
    if b.cross {
        s.push('x');
    }
    if b.circle {
        s.push('o');
    }
    if b.left {
        s.push('l');
    }
    if b.right {
        s.push('r');
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

fn curvature(t: &Track, i: usize) -> f32 {
    let a = t.nodes[i].dir;
    let b = t.nodes[(i + 10).min(NODE_COUNT - 1)].dir;
    a.x * b.z - a.z * b.x
}

/// A candidate: enter at `kph`, yank the handbrake with `turn` held for `hb` frames, then power out.
fn script(turn: Buttons, hb: u32) -> Vec<Step> {
    let mut s = vec![Step(0, NONE), Step(1, Buttons { cross: true, ..NONE })];
    let entry = 24;
    if hb > 0 {
        s.push(Step(
            entry,
            Buttons {
                circle: true,
                ..turn
            },
        ));
        s.push(Step(
            entry + hb,
            Buttons {
                cross: true,
                ..turn
            },
        ));
    } else {
        s.push(Step(
            entry,
            Buttons {
                cross: true,
                ..turn
            },
        ));
    }
    s
}

#[test]
fn find_a_drift() {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);

    let bytes = std::fs::read("assets/compiled/nissan_s15.azcar").expect("compile the cars first");
    let car = Car::parse(&bytes).expect("parse");

    let mut corners: Vec<(usize, f32)> = (200..NODE_COUNT - 300)
        .step_by(4)
        .map(|i| (i, curvature(&t, i)))
        .collect();
    corners.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
    let mut picked: Vec<(usize, f32)> = Vec::new();
    for c in corners {
        if picked.iter().all(|&(p, _)| (p as i32 - c.0 as i32).abs() > 80) {
            picked.push(c);
        }
        if picked.len() == 8 {
            break;
        }
    }

    let left = Buttons { left: true, ..NONE };
    let right = Buttons {
        right: true,
        ..NONE
    };

    // (score, node, kph, turn-name, hb, frame, slip, speed, lat)
    let mut best: Option<(f32, usize, u32, &'static str, u32, u32, f32, f32, f32)> = None;

    for &(node, curve) in &picked {
        for kph in [70u32, 90, 110, 130] {
            for (tname, turn) in [("l", left), ("r", right)] {
                for hb in [0u32, 12, 24, 40, 60] {
                    let steps = script(turn, hb);
                    let mut g = Box::new(Game::new());
                    g.enter_title(&t);
                    g.vehicle.shape = car.shape();
                    g.vehicle.handling = car.handling();

                    for frame in 0..200u32 {
                        if frame == 3 {
                            g.vehicle.place_at_node(&t, node);
                            g.vehicle.state.vx = kph as f32 / 3.6;
                        }
                        g.update(&t, buttons_at(&steps, frame), DT);
                        if frame < 30 {
                            continue;
                        }
                        let speed =
                            (g.vehicle.state.vx.powi(2) + g.vehicle.state.vy.powi(2)).sqrt();
                        let lat = g.vehicle.query.lat.abs();
                        // One frame is all a screenshot needs: angle, speed, and still on tarmac
                        // with room around the car.
                        if !g.vehicle.drifting || speed < 16.0 || lat > 3.2 {
                            continue;
                        }
                        let score = g.vehicle.slip_angle * speed;
                        if best.map_or(true, |b| score > b.0) {
                            best = Some((
                                score,
                                node,
                                kph,
                                tname,
                                hb,
                                frame,
                                g.vehicle.slip_angle,
                                speed,
                                g.vehicle.query.lat,
                            ));
                        }
                    }
                }
            }
        }
        println!("corner at node {node} curvature {curve:+.3}");
    }

    let (_, node, kph, tname, hb, frame, slip, speed, lat) = best.expect("nothing drifted");
    println!(
        "\nBEST: node {node}, {kph} kph, steer {tname}, handbrake {hb} frames\n\
         frame {frame}: slip {slip:.2} rad, {:.0} km/h, lat {lat:+.2}",
        speed * 3.6
    );

    let steps = script(if tname == "l" { left } else { right }, hb);
    println!("\nSCRIPT.TXT body:");
    for s in &steps {
        println!("{} {}", s.0, letters(s.1));
    }
    println!("place 3 {node} {kph}");

    // What the frames either side look like, so the burst window covers the good ones.
    let mut g = Box::new(Game::new());
    g.enter_title(&t);
    g.vehicle.shape = car.shape();
    g.vehicle.handling = car.handling();
    for f in 0..(frame + 40) {
        if f == 3 {
            g.vehicle.place_at_node(&t, node);
            g.vehicle.state.vx = kph as f32 / 3.6;
        }
        g.update(&t, buttons_at(&steps, f), DT);
        if f + 25 >= frame && f <= frame + 30 {
            let speed = (g.vehicle.state.vx.powi(2) + g.vehicle.state.vy.powi(2)).sqrt();
            println!(
                "frame {f:3} {:5.1} km/h slip {:5.2} lat {:6.2} combo x{} drifting {}",
                speed * 3.6,
                g.vehicle.slip_angle,
                g.vehicle.query.lat,
                g.scoring.combo,
                g.vehicle.drifting
            );
        }
    }
}
