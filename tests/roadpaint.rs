//! The road's paint: two lanes, a centre line that turns to double yellow for the bends, and the
//! warnings painted in the lane before them. These pin where each of them may go.

use angle_zero::roadpaint::{
    cracks, patches, surface, tone, Glyph, Paint, EDGE_IN, EDGE_OUT, LANE_IN, YELLOW_LEAD,
};
use angle_zero::track::{Track, TARMAC_HALF_WIDTH};

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

#[test]
fn the_lanes_fit_on_the_tarmac() {
    assert!(EDGE_OUT < TARMAC_HALF_WIDTH);
    assert!(LANE_IN < EDGE_IN);
    // Two lanes of a realistic mountain-road width.
    let lane = EDGE_IN - LANE_IN;
    assert!((3.0..=3.5).contains(&lane), "lane {lane} m");
}

#[test]
fn every_bend_is_double_yellow_from_before_it_to_after_it() {
    let t = track();
    let p = Paint::new(&t);
    assert!(p.bends().len() >= 10);
    for b in p.bends() {
        let apex = t.nodes[b.apex as usize].s;
        assert!(p.yellow_at(apex));
        assert!(p.yellow_at(b.start_s - YELLOW_LEAD + 1.0));
        assert!(b.start_s <= apex && apex <= b.end_s);
    }
    for &(s, _) in &p.apexes[..p.apex_count] {
        assert!(p.yellow_at(s), "apex at {s} is not in a yellow stretch");
    }
    // No two bends are the same stretch of road.
    for w in p.bends().windows(2) {
        assert!(w[1].start_s > w[0].start_s);
    }
}

#[test]
fn no_dash_runs_into_the_yellow() {
    let t = track();
    let p = Paint::new(&t);
    let mut s = 0.0;
    while s < t.length {
        if p.dash_starts_at(s) {
            for k in 0..=10 {
                assert!(!p.yellow_at(s + k as f32 * 0.5));
            }
        }
        s += 10.0;
    }
}

#[test]
fn speed_bars_come_before_their_bend_and_close_up() {
    let t = track();
    let p = Paint::new(&t);
    let mut bars = Vec::new();
    p.bars(|s| bars.push(s));
    // Bends on this pass come close together, so the bars before some are crowded out.
    assert!(bars.len() > 40, "{} bars", bars.len());
    for b in p.bends() {
        let mine: Vec<f32> = bars.iter().copied().filter(|&s| s < b.start_s && s > b.start_s - 40.0).collect();
        if mine.len() >= 3 {
            let gaps: Vec<f32> = mine.windows(2).map(|w| w[1] - w[0]).collect();
            assert!(gaps.windows(2).all(|g| g[1] <= g[0] + 1e-3), "{gaps:?}");
        }
    }
    // None inside a bend.
    for &s in &bars {
        assert!(p.bends().iter().all(|b| s < b.start_s || s > b.end_s));
    }
}

#[test]
fn road_text_is_in_our_lane_and_reads_nearest_first() {
    let t = track();
    let p = Paint::new(&t);
    let mut texts = Vec::new();
    p.texts(|x| texts.push(x));
    assert!(texts.iter().any(|x| x.glyph == Glyph::Kyu));
    for x in &texts {
        assert!(x.u - x.width / 2.0 >= -EDGE_IN && x.u + x.width / 2.0 <= -LANE_IN + 1e-3, "{x:?}");
    }
    for w in texts.windows(4) {
        if w[0].glyph == Glyph::Kyu {
            assert_eq!([w[1].glyph, w[2].glyph, w[3].glyph], [Glyph::Ka, Glyph::Bar, Glyph::Bu]);
            assert!(w[0].s < w[1].s && w[1].s < w[2].s && w[2].s < w[3].s);
        }
    }
}

#[test]
fn studs_only_where_the_line_is_yellow() {
    let t = track();
    let p = Paint::new(&t);
    let mut n = 0;
    p.studs(|s| {
        n += 1;
        assert!(p.yellow_at(s));
    });
    assert!(n > 100);
}

#[test]
fn patches_and_cracks_stay_inside_the_edge_lines() {
    let t = track();
    patches(&t, |x| {
        assert!(x.u0 < x.u1 && x.u0 >= -EDGE_IN && x.u1 <= EDGE_IN);
        assert!(x.u0 >= 0.0 || x.u1 <= 0.0, "a patch straddles the centre line");
    });
    cracks(&t, |pts| {
        for &(_, u) in pts {
            assert!(u.abs() < EDGE_IN);
        }
    });
}

#[test]
fn sections_differ_but_only_slightly() {
    let tones: Vec<f32> = (0..20).map(|i| tone(i as f32 * 210.0 + 5.0)).collect();
    assert!(tones.iter().all(|&k| (0.88..=1.12).contains(&k)));
    assert!(tones.windows(2).any(|w| (w[0] - w[1]).abs() > 0.02));
}

#[test]
fn surface_points_sit_on_the_road() {
    let t = track();
    for i in (10..t.nodes.len() - 10).step_by(37) {
        let n = &t.nodes[i];
        let p = surface(&t, n.s, 0.0, 0.0);
        assert!((p.y - n.p.y - 0.03).abs() < 0.02);
        assert!(p.horizontal_distance(n.p) < 0.05);
    }
}

#[test]
fn text_and_bars_do_not_overlap() {
    let t = track();
    let p = Paint::new(&t);
    let mut bars = Vec::new();
    p.bars(|s| bars.push(s));
    p.texts(|x| {
        for &b in &bars {
            assert!(b + 0.45 < x.s || b > x.s + x.length, "bar at {b} under {x:?}");
        }
    });
}

#[test]
fn cones_are_few_and_on_the_shoulders() {
    use angle_zero::roadpaint::{cones, SHOULDER_IN};
    let t = track();
    let p = Paint::new(&t);
    let mut all = Vec::new();
    cones(&t, &p, |s, u| all.push((s, u)));
    assert!(all.len() >= 8 && all.len() <= 25, "{} cones", all.len());
    for &(s, u) in &all {
        assert!(u.abs() >= SHOULDER_IN && u.abs() <= TARMAC_HALF_WIDTH + 0.6, "cone in a lane at {s} {u}");
        assert!(s > 0.0 && s < t.length);
    }
    // In a few spots, not spread along the pass: most cones have another within 5 m.
    let clustered = all.iter().filter(|a| all.iter().any(|b| b != *a && (a.0 - b.0).abs() < 5.0)).count();
    assert!(clustered * 10 >= all.len() * 9);
}
