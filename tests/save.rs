//! Persisted records.
//!
//! The file lives on a memory stick that can be pulled mid-write, so decoding treats every byte
//! as untrusted: a corrupt or truncated record has to read as "no record yet" rather than as
//! garbage best times.

use angle_zero::save::{Record, RECORD_BYTES};

#[test]
fn a_fresh_record_holds_nothing() {
    let r = Record::default();
    assert!(!r.has_time());
    assert_eq!(r.best_score, 0);
    assert_eq!(r.best_combo, 0);
}

#[test]
fn a_record_survives_a_round_trip() {
    let r = Record {
        best_time_cs: 13_705,
        best_score: 128_400,
        best_combo: 7,
    };
    let bytes = r.encode();
    assert_eq!(bytes.len(), RECORD_BYTES);
    assert_eq!(Record::decode(&bytes), Some(r));
}

#[test]
fn a_truncated_file_is_rejected() {
    let bytes = Record::default().encode();
    for n in 0..RECORD_BYTES {
        assert_eq!(Record::decode(&bytes[..n]), None, "accepted {n} bytes");
    }
}

#[test]
fn a_file_from_something_else_is_rejected() {
    let mut bytes = Record::default().encode();
    bytes[0] ^= 0xFF;
    assert_eq!(Record::decode(&bytes), None, "accepted a bad magic");
}

#[test]
fn a_record_from_a_future_version_is_rejected() {
    let mut bytes = Record {
        best_time_cs: 100,
        best_score: 1,
        best_combo: 1,
    }
    .encode();
    // Bump the version byte and repair nothing else; a newer layout must not be misread.
    bytes[3] = 0xFE;
    assert_eq!(Record::decode(&bytes), None);
}

#[test]
fn a_corrupted_payload_is_rejected() {
    let r = Record {
        best_time_cs: 9_999,
        best_score: 5_000,
        best_combo: 4,
    };
    let good = r.encode();
    // Flip one bit in each payload byte in turn; the checksum must catch every one.
    for i in 4..RECORD_BYTES - 4 {
        let mut bad = good;
        bad[i] ^= 0x01;
        assert_eq!(Record::decode(&bad), None, "byte {i} slipped past the checksum");
    }
}

#[test]
fn the_first_finish_sets_every_record() {
    let mut r = Record::default();
    assert!(r.merge_run(97.35, 12_500.0, 5));
    assert_eq!(r.best_time_cs, 9_735);
    assert_eq!(r.best_score, 12_500);
    assert_eq!(r.best_combo, 5);
    assert!(r.has_time());
}

#[test]
fn only_improvements_are_kept() {
    let mut r = Record::default();
    r.merge_run(97.35, 12_500.0, 5);

    // Slower, lower scoring, smaller combo: nothing changes.
    assert!(!r.merge_run(120.0, 900.0, 2));
    assert_eq!(r.best_time_cs, 9_735);
    assert_eq!(r.best_score, 12_500);
    assert_eq!(r.best_combo, 5);

    // A quicker run improves only the time.
    assert!(r.merge_run(90.0, 100.0, 1));
    assert_eq!(r.best_time_cs, 9_000);
    assert_eq!(r.best_score, 12_500);
    assert_eq!(r.best_combo, 5);

    // A better score improves only the score.
    assert!(r.merge_run(200.0, 30_000.0, 1));
    assert_eq!(r.best_time_cs, 9_000);
    assert_eq!(r.best_score, 30_000);

    // And a bigger combo only the combo.
    assert!(r.merge_run(200.0, 1.0, 9));
    assert_eq!(r.best_combo, 9);
}

#[test]
fn each_record_improves_independently_of_the_others() {
    // A blistering time on a scrappy run must not wipe out a hard-won score.
    let mut r = Record::default();
    r.merge_run(150.0, 90_000.0, 9);
    r.merge_run(60.0, 0.0, 1);
    assert_eq!(r.best_time_cs, 6_000);
    assert_eq!(r.best_score, 90_000);
    assert_eq!(r.best_combo, 9);
}

#[test]
fn a_nonsensical_run_is_ignored_rather_than_stored() {
    let mut r = Record::default();
    // A zero or negative time is not a finish, and must not become an unbeatable record.
    assert!(!r.merge_run(0.0, 500.0, 3));
    assert!(!r.has_time());
    // The score and combo from it are not credited either.
    assert_eq!(r.best_score, 0);
}

#[test]
fn records_survive_a_round_trip_after_merging() {
    let mut r = Record::default();
    r.merge_run(83.21, 44_000.0, 6);
    assert_eq!(Record::decode(&r.encode()), Some(r));
}

// --- integration with the game loop -------------------------------------------------------

use angle_zero::game::{Buttons, Game, Phase};
use angle_zero::track::{Track, NODE_COUNT};

const NONE: Buttons = Buttons {
    cross: false, circle: false, square: false, triangle: false,
    up: false, down: false, left: false, right: false, analog_x: 0.0,
};

/// Runs a descent from near the finish so it completes quickly.
fn finish_a_run(g: &mut Game, track: &Track) {
    g.update(track, Buttons { cross: true, ..NONE }, 1.0 / 60.0);
    g.vehicle.place_at_node(track, NODE_COUNT - 30);
    g.vehicle.state.vx = 20.0;
    for _ in 0..600 {
        g.update(track, Buttons { cross: true, ..NONE }, 1.0 / 60.0);
        if g.phase == Phase::Results {
            return;
        }
    }
    panic!("run never finished");
}

#[test]
fn finishing_a_run_updates_the_record_and_asks_to_be_saved() {
    let mut track = Box::new(Track::EMPTY);
    Track::generate(&mut track);
    let mut g = Box::new(Game::new());
    g.enter_title(&track);

    assert!(!g.take_record_dirty());
    finish_a_run(&mut g, &track);

    assert!(g.record.has_time(), "a finished run should set a best time");
    assert!(g.take_record_dirty(), "the record should be flagged for saving");
    // The flag clears once taken, so the shell writes once rather than every frame.
    assert!(!g.take_record_dirty());
}

#[test]
fn a_worse_second_run_does_not_ask_to_be_saved_again() {
    let mut track = Box::new(Track::EMPTY);
    Track::generate(&mut track);
    let mut g = Box::new(Game::new());
    g.enter_title(&track);

    finish_a_run(&mut g, &track);
    g.take_record_dirty();
    let best = g.record;

    // Pretend a much better run already happened, so the next one cannot beat anything.
    g.record.best_time_cs = 1;
    g.record.best_score = u32::MAX;
    g.record.best_combo = 9;
    g.start_run(&track);
    finish_a_run(&mut g, &track);
    assert!(!g.take_record_dirty(), "nothing improved, so nothing to write");
    let _ = best;
}
