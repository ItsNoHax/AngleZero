//! Reading a car in pieces.
//!
//! The counting has to be exactly right in two places: the last chunk, which is short because the
//! file ended, and a short read, which is short because something went wrong. They look the same
//! to the caller unless the request never overruns the file.

use angle_zero::stream::Progress;

#[test]
fn a_load_that_has_not_started_has_everything_left() {
    let p = Progress::new(800_000);
    assert_eq!(p.done(), 0);
    assert_eq!(p.size(), 800_000);
    assert_eq!(p.rest(), 800_000);
    assert!(!p.is_complete());
    assert_eq!(p.fraction(), 0.0);
}

#[test]
fn a_car_arrives_in_whole_chunks_and_one_short_one() {
    const CHUNK: usize = 32 * 1024;
    let mut p = Progress::new(CHUNK * 3 + 100);
    let mut reads = std::vec::Vec::new();
    while !p.is_complete() {
        let n = p.next_read(CHUNK);
        reads.push(n);
        p.advance(n);
    }
    assert_eq!(reads, [CHUNK, CHUNK, CHUNK, 100]);
    assert_eq!(p.done(), p.size());
    assert_eq!(p.fraction(), 1.0);
}

#[test]
fn the_request_never_overruns_the_file() {
    // Which is what lets the caller treat "read fewer bytes than asked for" as a fault rather
    // than as the end of the file.
    let mut p = Progress::new(10);
    assert_eq!(p.next_read(4096), 10);
    p.advance(10);
    assert_eq!(p.next_read(4096), 0);
}

#[test]
fn a_hurried_load_asks_for_everything_left() {
    let mut p = Progress::new(900_000);
    p.advance(32 * 1024);
    assert_eq!(p.rest(), 900_000 - 32 * 1024);
    p.advance(p.rest());
    assert!(p.is_complete());
}

#[test]
fn a_load_cannot_be_advanced_past_its_end() {
    let mut p = Progress::new(100);
    p.advance(4096);
    assert_eq!(p.done(), 100);
    assert!(p.is_complete());
    assert_eq!(p.fraction(), 1.0);
}

#[test]
fn an_empty_file_is_already_complete() {
    // Not a car — the format refuses it — but the counting must not divide by zero on the way to
    // saying so.
    let p = Progress::new(0);
    assert!(p.is_complete());
    assert_eq!(p.fraction(), 1.0);
    assert_eq!(p.next_read(4096), 0);
}
