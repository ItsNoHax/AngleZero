//! Screen flow and the fixed-timestep loop.
//!
//! This is the piece the PSP shell drives: hand it the track, the buttons and a frame delta, and
//! it advances everything else. Physics runs on its own fixed 1/120 s clock underneath, so the
//! game behaves identically whether the renderer is managing 60 fps or 20.

use crate::camera::Camera;
use crate::effects::Effects;
use crate::math::{atan2, clamp, cos, min, sin};
use crate::save::Record;
use crate::scoring::Scoring;
use crate::track::{Track, BAY_NODE, BAY_SIDE};
use crate::vehicle::{Input, Vehicle, FIXED_DT, MAX_FRAME_DT, MAX_SUBSTEPS};

/// Node the run starts from. The first two nodes are spline lead-in.
pub const START_NODE: usize = 2;
/// Fraction of the centreline that counts as finishing.
pub const FINISH_PROGRESS: f32 = 0.985;
/// Beyond this lateral offset there is no tarmac left to mark.
pub const SKID_LAT: f32 = 6.2;
/// How long a toast stays up, and how long it spends fading out.
pub const TOAST_HOLD: f32 = 1.1;
pub const TOAST_FADE: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Title,
    Run,
    /// The run, suspended mid-descent with the menu up. Everything keeps its state — this is the
    /// same run, waiting — so `Continue` is a phase change and nothing else.
    Paused,
    Results,
}

/// The pause menu's three entries, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseChoice {
    Continue,
    Restart,
    ChangeCar,
}

impl PauseChoice {
    pub const ALL: [PauseChoice; 3] = [
        PauseChoice::Continue,
        PauseChoice::Restart,
        PauseChoice::ChangeCar,
    ];

    /// Moves `delta` entries down the list, wrapping. Wrapping rather than stopping at the ends
    /// because there are three of them: the far entry is one press away either way round.
    pub fn step(self, delta: i32) -> PauseChoice {
        let n = Self::ALL.len() as i32;
        let i = Self::ALL.iter().position(|&c| c == self).unwrap_or(0) as i32;
        Self::ALL[(((i + delta) % n + n) % n) as usize]
    }
}

/// The messages that flash up mid-run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toast {
    Go,
    ComboUp(u32),
    WallTap,
    WrongWay,
    BackOnTrack,
}

/// PSP controller state for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Buttons {
    pub cross: bool,
    pub circle: bool,
    pub square: bool,
    pub triangle: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// START, which opens the pause menu and closes it again. Never a driving input.
    pub start: bool,
    /// Analog nub, -1.0 (right) to +1.0 (left). Ignored when the d-pad is used.
    pub analog_x: f32,
}

/// What the results screen shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunResult {
    pub time: f32,
    pub score: f32,
    pub best_combo: u32,
}

pub struct Game {
    pub phase: Phase,
    /// How many cars the shell managed to load, which is how many the title screen offers. One
    /// unless something set it: the core has no way to find out for itself, and no business
    /// knowing where cars come from.
    pub car_count: usize,
    pub vehicle: Vehicle,
    pub scoring: Scoring,
    pub camera: Camera,
    pub effects: Effects,
    pub run_time: f32,
    pub result: RunResult,

    /// Best time, score and combo across every run, loaded and stored by the shell.
    pub record: Record,
    /// Set when `record` has improved and needs writing back.
    record_dirty: bool,

    /// Which pause-menu entry is highlighted. Only meaningful in `Phase::Paused`.
    pub pause_choice: PauseChoice,

    pub toast: Option<Toast>,
    pub toast_timer: f32,
    /// Throttle applied on the last substep, which the rev counter reads.
    last_throttle: f32,
    /// Whether the brake or handbrake was applied, which the tail lamps read.
    last_braking: bool,
    /// Counts guard-rail impacts. The shell watches it to fire the thud once per hit.
    impacts: u32,

    /// Last frame's buttons, for edge detection.
    prev: Buttons,
    /// Leftover time not yet consumed by a fixed substep.
    accumulator: f32,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub const fn new() -> Self {
        Game {
            phase: Phase::Title,
            car_count: 1,
            vehicle: Vehicle::new(),
            scoring: Scoring::new(),
            camera: Camera::new(),
            effects: Effects::new(),
            run_time: 0.0,
            result: RunResult {
                time: 0.0,
                score: 0.0,
                best_combo: 1,
            },
            record: Record {
                best_time_cs: 0,
                best_score: 0,
                best_combo: 0,
            },
            record_dirty: false,
            pause_choice: PauseChoice::Continue,
            toast: None,
            toast_timer: 0.0,
            last_throttle: 0.0,
            last_braking: false,
            impacts: 0,
            prev: Buttons {
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
            },
            accumulator: 0.0,
        }
    }

    /// Maps the controller to driving inputs.
    pub fn drive_input(b: &Buttons) -> Input {
        // The d-pad is all-or-nothing; the nub only speaks when the d-pad is quiet.
        let steer_in = if b.left || b.right {
            (if b.left { 1.0 } else { 0.0 }) - (if b.right { 1.0 } else { 0.0 })
        } else {
            clamp(b.analog_x, -1.0, 1.0)
        };

        Input {
            throttle: if b.cross || b.up { 1.0 } else { 0.0 },
            brake: b.square || b.down,
            handbrake: b.circle,
            steer_in,
        }
    }

    /// Puts the car in the emergency pull-off and shows the title screen.
    pub fn enter_title(&mut self, track: &Track) {
        self.camera.front_view = false;
        let n = &track.nodes[BAY_NODE];
        let road_heading = atan2(n.dir.x, n.dir.z);
        // Parked on the apron, then nudged forward and back toward the road.
        let pad_x = n.p.x + n.nrm.x * BAY_SIDE * 11.5;
        let pad_z = n.p.z + n.nrm.z * BAY_SIDE * 11.5;
        let x = pad_x + sin(road_heading) * 2.5 - n.nrm.x * BAY_SIDE * 1.2;
        let z = pad_z + cos(road_heading) * 2.5 - n.nrm.z * BAY_SIDE * 1.2;

        // On the paving, which is the road's own surface carried outwards — not on the shelf
        // underneath it, which would sink the car a quarter of a metre into its own car park.
        let lateral = 11.5 - 1.2;
        let y = n.p.y + crate::track::bay_apron_offset(lateral);
        self.vehicle
            .place_at(track, x, y, z, road_heading + BAY_SIDE * 0.16, BAY_NODE);
        self.phase = Phase::Title;
        self.camera.orbit_angle = 0.0;
        self.toast = None;
        self.toast_timer = 0.0;
        self.scoring.reset();
        self.run_time = 0.0;
    }

    /// Moves to the next or previous car, wrapping.
    ///
    /// Only the index changes. What that car is, how it is drawn and what its wheels measure are
    /// all the asset's business — the simulation drives whatever is in the slot, which is what
    /// makes a new car a new file.
    pub fn select_car(&mut self, delta: i32) {
        if self.car_count < 2 {
            return;
        }
        let n = self.car_count as i32;
        self.vehicle.model = (((self.vehicle.model as i32 + delta) % n + n) % n) as usize;
    }

    /// Starts a fresh descent from the top of the road.
    pub fn start_run(&mut self, track: &Track) {
        self.vehicle.place_at_node(track, START_NODE);
        self.scoring.reset();
        self.effects.reset();
        self.run_time = 0.0;
        self.accumulator = 0.0;
        self.phase = Phase::Run;
        self.camera.front_view = false;
        self.camera.snap_behind(&self.vehicle.state);
        self.show(Toast::Go);
    }

    /// Suspends the run and opens the menu, always on `Continue` — the entry that undoes the
    /// press, so a mistaken START costs one more press and nothing else.
    pub fn pause(&mut self) {
        self.phase = Phase::Paused;
        self.pause_choice = PauseChoice::Continue;
        self.camera.front_view = false;
    }

    fn show(&mut self, toast: Toast) {
        self.toast = Some(toast);
        self.toast_timer = TOAST_HOLD;
    }

    /// Toast alpha, 1.0 while held then fading to 0.0.
    pub fn toast_opacity(&self) -> f32 {
        clamp(self.toast_timer / TOAST_FADE, 0.0, 1.0)
    }

    /// Throttle from the last substep, for the rev counter.
    #[inline]
    pub fn throttle_hint(&self) -> f32 {
        self.last_throttle
    }

    /// Whether the brake or handbrake is on, for the tail lamps.
    #[inline]
    pub fn braking_hint(&self) -> bool {
        self.last_braking
    }

    /// Running count of guard-rail impacts. The shell compares it against the last value it saw
    /// rather than being handed a flag, so a hit is sounded exactly once however many frames
    /// elapse between the audio thread's buffers.
    #[inline]
    pub fn impact_count(&self) -> u32 {
        self.impacts
    }

    /// Progress down the hill as a whole percentage, for the HUD.
    pub fn descent_percent(&self, track: &Track) -> u32 {
        (track.progress(self.vehicle.locator.last_idx) * 100.0 + 0.5) as u32
    }

    /// Advances one rendered frame.
    pub fn update(&mut self, track: &Track, buttons: Buttons, dt: f32) {
        let pressed = |now: bool, before: bool| now && !before;
        let cross_edge = pressed(buttons.cross, self.prev.cross);
        let square_edge = pressed(buttons.square, self.prev.square);
        let start_edge = pressed(buttons.start, self.prev.start);
        let right_edge = pressed(buttons.right, self.prev.right);
        let left_edge = pressed(buttons.left, self.prev.left);
        let up_edge = pressed(buttons.up, self.prev.up);
        let down_edge = pressed(buttons.down, self.prev.down);
        self.prev = buttons;

        // A hitch must not be simulated in full, or the car teleports through the scenery.
        let frame_dt = min(dt, MAX_FRAME_DT);

        match self.phase {
            Phase::Title => {
                if cross_edge {
                    self.start_run(track);
                } else {
                    // Picking a car. Only on the title screen, where the car is standing still in
                    // front of the player and swapping it is something to look at rather than
                    // something that happens mid-corner.
                    if right_edge {
                        self.select_car(1);
                    }
                    if left_edge {
                        self.select_car(-1);
                    }
                    self.camera.update_title(&self.vehicle.state, frame_dt);
                }
            }
            Phase::Run => {
                if start_edge {
                    self.pause();
                } else {
                    // Held, not toggled: it is a glance at the front of the car, and letting go has
                    // to put the road back without a second press to remember.
                    self.camera.front_view = buttons.triangle;
                    self.run_substeps(track, buttons, frame_dt);
                    self.camera.update_run(&self.vehicle.state, frame_dt);
                    // Smoke ages per rendered frame rather than per substep.
                    self.effects.update(frame_dt);

                    if track.progress(self.vehicle.locator.last_idx) > FINISH_PROGRESS {
                        self.finish();
                    }
                }
            }
            // Nothing moves: no substeps, no camera, no effects and no clock. The frame is drawn
            // from the state the run was suspended in, which is what makes this a pause rather
            // than a menu the car keeps rolling behind.
            Phase::Paused => {
                if up_edge {
                    self.pause_choice = self.pause_choice.step(-1);
                }
                if down_edge {
                    self.pause_choice = self.pause_choice.step(1);
                }
                // START closes the menu the way it opened it, whatever is highlighted.
                if start_edge {
                    self.phase = Phase::Run;
                } else if cross_edge {
                    match self.pause_choice {
                        PauseChoice::Continue => self.phase = Phase::Run,
                        PauseChoice::Restart => self.start_run(track),
                        PauseChoice::ChangeCar => self.enter_title(track),
                    }
                }
            }
            Phase::Results => {
                if cross_edge {
                    self.start_run(track);
                } else if square_edge {
                    self.enter_title(track);
                } else {
                    self.camera.update_run(&self.vehicle.state, frame_dt);
                }
            }
        }

        // A toast holds its place while the game is paused rather than fading out behind the menu.
        if self.phase != Phase::Paused && self.toast_timer > 0.0 {
            self.toast_timer -= frame_dt;
            if self.toast_timer <= 0.0 {
                self.toast_timer = 0.0;
                self.toast = None;
            }
        }
    }

    fn run_substeps(&mut self, track: &Track, buttons: Buttons, frame_dt: f32) {
        let input = Self::drive_input(&buttons);
        self.last_throttle = input.throttle;
        self.last_braking = input.brake || input.handbrake;
        self.accumulator += frame_dt;

        let mut guard = 0;
        while self.accumulator >= FIXED_DT && guard < MAX_SUBSTEPS {
            guard += 1;
            self.accumulator -= FIXED_DT;

            let outcome = self.vehicle.step(track, input, FIXED_DT);
            self.run_time += FIXED_DT;

            if outcome.respawned {
                self.scoring.on_wall_tap();
                self.show(Toast::BackOnTrack);
                continue;
            }
            if outcome.wall_tap {
                self.camera.add_shake(0.5);
                self.impacts = self.impacts.wrapping_add(1);
                // The combo always dies; it is only worth announcing if there was one.
                if self.scoring.on_wall_tap() {
                    self.show(Toast::WallTap);
                }
            } else if outcome.wrong_way {
                self.show(Toast::WrongWay);
            }

            let speed = crate::math::hypot(self.vehicle.state.vx, self.vehicle.state.vy);
            let event = self.scoring.update(
                self.vehicle.drifting,
                speed,
                self.vehicle.slip_angle,
                FIXED_DT,
            );
            if event.combo_up {
                self.show(Toast::ComboUp(self.scoring.combo));
            }

            if self.vehicle.drifting {
                // Marks only where there is tarmac to mark; smoke happens regardless.
                if crate::math::abs(self.vehicle.query.lat) < SKID_LAT {
                    self.effects
                        .emit_skids(&self.vehicle.state, &self.vehicle.shape, speed);
                }
                self.effects.emit_smoke(&self.vehicle.state);
            }
        }

        // Whatever is left over would otherwise pile up during a long stall.
        if self.accumulator > FIXED_DT * MAX_SUBSTEPS as f32 {
            self.accumulator = 0.0;
        }
    }

    fn finish(&mut self) {
        self.phase = Phase::Results;
        self.result = RunResult {
            time: self.run_time,
            score: self.scoring.score,
            best_combo: self.scoring.best_combo,
        };
        if self.record.merge_run(
            self.result.time,
            self.result.score,
            self.result.best_combo,
        ) {
            self.record_dirty = true;
        }
    }

    /// Whether the stored record needs writing back, clearing the flag. The shell owns the
    /// filesystem, so it asks rather than the core reaching for it.
    pub fn take_record_dirty(&mut self) -> bool {
        let dirty = self.record_dirty;
        self.record_dirty = false;
        dirty
    }
}
