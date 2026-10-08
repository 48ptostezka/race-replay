//! Coordinates, distances and the Web Mercator pixel projection Leaflet uses.

use std::f64::consts::PI;

/// A point in degrees (WGS84).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

/// Great-circle distance in metres.
pub fn distance_m(a: LatLon, b: LatLon) -> f64 {
    const R: f64 = 6_371_008.8;
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dp = p2 - p1;
    let dl = (b.lon - a.lon).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().min(1.0).asin()
}

/// Cumulative distance along the points, metres (same length as `pts`).
pub fn cumulative_m(pts: &[LatLon]) -> Vec<f64> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            acc += distance_m(pts[i - 1], *p);
        }
        out.push(acc);
    }
    out
}

/// Mean of points (fine for points a few metres apart).
pub fn mean(pts: &[LatLon]) -> LatLon {
    let n = pts.len().max(1) as f64;
    LatLon::new(pts.iter().map(|p| p.lat).sum::<f64>() / n, pts.iter().map(|p| p.lon).sum::<f64>() / n)
}

/// Web Mercator: point -> world pixel coordinates at `zoom` (256 px tiles), as Leaflet.
pub fn project(p: LatLon, zoom: f64) -> (f64, f64) {
    let scale = 256.0 * 2f64.powf(zoom);
    let lat = p.lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let x = (p.lon + 180.0) / 360.0 * scale;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0 * scale;
    (x, y)
}

/// Inverse of [`project`].
pub fn unproject(x: f64, y: f64, zoom: f64) -> LatLon {
    let scale = 256.0 * 2f64.powf(zoom);
    let lon = x / scale * 360.0 - 180.0;
    let n = PI - 2.0 * PI * y / scale;
    let lat = (0.5 * (n.exp() - (-n).exp())).atan().to_degrees();
    LatLon::new(lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips() {
        let p = LatLon::new(49.2345, 16.6789);
        for z in [10.0, 15.0, 17.5] {
            let (x, y) = project(p, z);
            let q = unproject(x, y, z);
            assert!((q.lat - p.lat).abs() < 1e-9 && (q.lon - p.lon).abs() < 1e-9);
        }
        // one pixel at zoom 17 is about 0.78 m at this latitude
        let (x, y) = project(p, 17.0);
        let d = distance_m(p, unproject(x + 1.0, y, 17.0));
        assert!((d - 0.78).abs() < 0.05, "{d}");
    }
}
