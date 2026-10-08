//! Screen-space layout of dots: rings around a location, and walking dots pushed apart.

use std::f64::consts::PI;

/// Pixel offsets for `n` dots around a location. Ring r (1-based) has radius
/// 20 + 15 (r − 1) px and holds as many dots as fit 16 px apart: 7, 13, 19, …
/// Slot 0 is at the top, then clockwise.
pub fn ring_slots(n: usize) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(n);
    let mut ring = 0;
    while out.len() < n {
        let r = 20.0 + 15.0 * ring as f64;
        let cap = (2.0 * PI * r / 16.0).floor() as usize;
        let take = cap.min(n - out.len());
        for i in 0..take {
            let a = -PI / 2.0 + 2.0 * PI * i as f64 / cap as f64;
            out.push((r * a.cos(), r * a.sin()));
        }
        ring += 1;
    }
    out
}

/// Walking dots closer than 6 px on screen are pushed apart across their legs.
/// `pts` holds (x, y, normal_x, normal_y) in pixels; positions are changed in place.
pub fn nudge(pts: &mut [(f64, f64, f64, f64)]) {
    const MIN: f64 = 6.0;
    const PUSH: f64 = 4.5;
    for i in 0..pts.len() {
        for j in i + 1..pts.len() {
            let (dx, dy) = (pts[i].0 - pts[j].0, pts[i].1 - pts[j].1);
            if (dx * dx + dy * dy).sqrt() < MIN {
                let (_, _, nx, ny) = pts[i];
                pts[i].0 += nx * PUSH;
                pts[i].1 += ny * PUSH;
                let (_, _, nx, ny) = pts[j];
                pts[j].0 -= nx * PUSH;
                pts[j].1 -= ny * PUSH;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings() {
        let s = ring_slots(11);
        assert_eq!(s.len(), 11);
        let r = |p: (f64, f64)| (p.0 * p.0 + p.1 * p.1).sqrt();
        assert!(s[..7].iter().all(|&p| (r(p) - 20.0).abs() < 1e-9));
        assert!(s[7..].iter().all(|&p| (r(p) - 35.0).abs() < 1e-9));
        assert!((s[0].0).abs() < 1e-9 && s[0].1 < 0.0); // first slot at the top
        assert_eq!(ring_slots(7 + 13 + 1).iter().filter(|&&p| (r(p) - 50.0).abs() < 1e-9).count(), 1);
        // neighbours on ring 1 are at least 16 px apart
        let d = ((s[0].0 - s[1].0).powi(2) + (s[0].1 - s[1].1).powi(2)).sqrt();
        assert!(d >= 16.0, "{d}");
    }

    #[test]
    fn nudge_separates_overlaps() {
        let mut p = vec![(100.0, 100.0, 0.0, 1.0), (101.0, 100.0, 0.0, 1.0), (200.0, 200.0, 1.0, 0.0)];
        nudge(&mut p);
        assert!((p[0].1 - p[1].1).abs() >= 9.0);
        assert_eq!((p[2].0, p[2].1), (200.0, 200.0));
    }
}
