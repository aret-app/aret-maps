//! Elevation profile, climbing and walking time of a route line (aret-maps#5).
//!
//! - **Profile:** the line sampled every [`STEP_M`] metres on the elevation model.
//! - **Ascent/descent:** from the profile smoothed over [`SMOOTHING_M`], counting a change only once
//!   it exceeds [`THRESHOLD_M`] in one direction. Where a path crosses a steep slope, a few metres of
//!   sideways error in the mapped line make the sampled height wobble up and down the slope, and those
//!   wobbles would add up to fake climbing. Calibrated on the pilot region (2026-11-27) against 1,812
//!   routes with a mapped `ascent` and 53 with a mapped `duration` (mostly CAI sign times):
//!
//!   | smoothing / threshold | ascent vs mapped (median, median abs) | time vs mapped |
//!   |---|---|---|
//!   | 90 m / 5 m | −10%, 16% | −10%, 11% |
//!   | 60 m / 3 m | −7%, 14% | −7%, 11% |
//!   | **30 m / 3 m** | **−3%, 14%** | **−5%, 8%** |
//!   | none / 3 m | −1%, 13% | −3%, 7% |
//!
//!   30 m still removes the shortest wobble while staying within a few percent of both references
//!   (which carry some of the same noise themselves).
//! - **Walking time:** DIN 33466, the rule used by the Alpine clubs: 4 km/h on the flat, 300 m/h
//!   up, 500 m/h down; the larger of the distance time and the climbing time counts fully, the
//!   other half. An estimate for an average hiker without breaks, never a promise.

use geo::{Distance, Geodesic, Point};

use crate::dem::Dem;

/// Distance between profile samples, in metres: one TINITALY cell.
pub const STEP_M: f64 = 10.0;
/// Length of the moving average applied before counting climbing, in metres (see the calibration
/// in the module docs).
pub const SMOOTHING_M: f64 = 30.0;
/// Smallest change in one direction that counts as climbing, in metres.
pub const THRESHOLD_M: f64 = 3.0;
/// Share of samples that must have an elevation; otherwise the line gets none.
const MIN_COVERAGE: f64 = 0.95;

/// Elevation of one line, in the line's direction.
#[derive(Debug, Clone, PartialEq)]
pub struct LineElevation {
    /// (distance from the start in metres, elevation in metres), every [`STEP_M`].
    pub profile: Vec<(f64, f64)>,
    /// Metres climbed.
    pub ascent_m: f64,
    /// Metres descended.
    pub descent_m: f64,
}

impl LineElevation {
    /// Elevation of the first sample.
    pub fn start(&self) -> f64 {
        self.profile.first().map_or(0.0, |&(_, ele)| ele)
    }

    /// Elevation of the last sample.
    pub fn end(&self) -> f64 {
        self.profile.last().map_or(0.0, |&(_, ele)| ele)
    }

    /// Lowest and highest elevation.
    pub fn range(&self) -> (f64, f64) {
        self.profile
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), &(_, ele)| {
                (low.min(ele), high.max(ele))
            })
    }

    /// Length along the profile, in metres.
    pub fn length_m(&self) -> f64 {
        self.profile.last().map_or(0.0, |&(distance, _)| distance)
    }

    /// The same line walked the other way.
    pub fn reversed(&self) -> Self {
        let length = self.length_m();
        Self {
            profile: self
                .profile
                .iter()
                .rev()
                .map(|&(distance, ele)| (length - distance, ele))
                .collect(),
            ascent_m: self.descent_m,
            descent_m: self.ascent_m,
        }
    }
}

/// Samples `line` (longitude, latitude) on `dem`; `None` if under 95% of it has elevation data
/// (the line leaves the area covered by the model).
pub fn line_elevation(line: &[(f64, f64)], dem: &Dem) -> Option<LineElevation> {
    let points = resample(line, STEP_M);
    let total = points.len();
    let profile: Vec<(f64, f64)> = points
        .into_iter()
        .filter_map(|(distance, point)| dem.elevation(point).map(|ele| (distance, ele)))
        .collect();
    if total == 0 || (profile.len() as f64) < MIN_COVERAGE * total as f64 {
        return None;
    }
    let elevations: Vec<f64> = profile.iter().map(|&(_, ele)| ele).collect();
    let window = (SMOOTHING_M / STEP_M).round() as usize;
    let (ascent_m, descent_m) = climb(&moving_average(&elevations, window), THRESHOLD_M);
    Some(LineElevation {
        profile,
        ascent_m,
        descent_m,
    })
}

/// Points every `step` metres along `line` (plus its last point), with their distance from the
/// start on the WGS 84 ellipsoid. Within a segment, positions are interpolated linearly: segments
/// of hiking paths are short, so the difference from the true geodesic is millimetres.
pub fn resample(line: &[(f64, f64)], step: f64) -> Vec<(f64, (f64, f64))> {
    let Some(&first) = line.first() else {
        return Vec::new();
    };
    let mut samples = vec![(0.0, first)];
    let (mut walked, mut next_sample) = (0.0, step);
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = Geodesic.distance(Point::from(a), Point::from(b));
        while length > 0.0 && next_sample <= walked + length {
            let t = (next_sample - walked) / length;
            samples.push((next_sample, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)));
            next_sample += step;
        }
        walked += length;
    }
    if let Some(&last) = line.last()
        && samples
            .last()
            .is_some_and(|&(distance, _)| distance < walked)
    {
        samples.push((walked, last));
    }
    samples
}

/// Centred moving average over `window` samples (shorter at both ends).
pub fn moving_average(values: &[f64], window: usize) -> Vec<f64> {
    let half = window / 2;
    (0..values.len())
        .map(|index| {
            let from = index.saturating_sub(half);
            let to = (index + half + 1).min(values.len());
            values[from..to].iter().sum::<f64>() / (to - from) as f64
        })
        .collect()
}

/// Ascent and descent of `elevations`, counting a change only once it exceeds `threshold` in one
/// direction (hysteresis).
pub fn climb(elevations: &[f64], threshold: f64) -> (f64, f64) {
    let Some(&first) = elevations.first() else {
        return (0.0, 0.0);
    };
    let (mut ascent, mut descent, mut reference) = (0.0, 0.0, first);
    for &ele in &elevations[1..] {
        let change = ele - reference;
        if change >= threshold {
            ascent += change;
            reference = ele;
        } else if change <= -threshold {
            descent -= change;
            reference = ele;
        }
    }
    (ascent, descent)
}

/// DIN 33466 walking time in minutes, rounded to 5 minutes.
pub fn walking_minutes(length_m: f64, ascent_m: f64, descent_m: f64) -> u32 {
    let horizontal_h = length_m / 4000.0;
    let vertical_h = ascent_m / 300.0 + descent_m / 500.0;
    let hours = horizontal_h.max(vertical_h) + horizontal_h.min(vertical_h) / 2.0;
    ((hours * 60.0 / 5.0).round() * 5.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climb_ignores_small_wobbles() {
        // A 0.5 m wobble on a flat path is not climbing
        let flat: Vec<f64> = (0..100)
            .map(|i| 1000.0 + if i % 2 == 0 { 0.5 } else { 0.0 })
            .collect();
        assert_eq!(climb(&flat, THRESHOLD_M), (0.0, 0.0));
        // Up 100, down 40, up 20
        let hill = [500.0, 550.0, 600.0, 580.0, 560.0, 570.0, 580.0];
        assert_eq!(climb(&hill, THRESHOLD_M), (120.0, 40.0));
    }

    #[test]
    fn smoothing_removes_wobble_but_keeps_real_climbs() {
        // A steady 200 m climb over 1 km (every 10 m), with a ±4 m wobble from sample to sample, as
        // when the mapped path zig-zags a few metres across a steep slope
        let wobbly: Vec<f64> = (0..=100)
            .map(|i| 1000.0 + 2.0 * f64::from(i) + if i % 2 == 0 { 4.0 } else { -4.0 })
            .collect();
        let (raw_up, raw_down) = climb(&wobbly, THRESHOLD_M);
        let window = (SMOOTHING_M / STEP_M).round() as usize;
        let (up, down) = climb(&moving_average(&wobbly, window), THRESHOLD_M);
        assert!(
            raw_up > 250.0 && raw_down > 50.0,
            "raw counts the wobble: {raw_up} up, {raw_down} down"
        );
        assert!(
            (up - 200.0).abs() < 25.0,
            "smoothed keeps the real climb: {up}"
        );
        assert!(down < 10.0, "and drops the fake descents: {down}");
    }

    #[test]
    fn walking_time_follows_din_33466() {
        // 12 km on the flat: 3 h
        assert_eq!(walking_minutes(12_000.0, 0.0, 0.0), 180);
        // 4 km with 900 m up: climbing 3 h counts fully, distance 1 h half → 3 h 30
        assert_eq!(walking_minutes(4_000.0, 900.0, 0.0), 210);
        // Same path down: 1 h 48 descending + 1 h / 2 → 2 h 18 → rounded to 2 h 20
        assert_eq!(walking_minutes(4_000.0, 0.0, 900.0), 140);
    }

    #[test]
    fn resamples_every_step_and_keeps_the_end() {
        // 0.001° of latitude ≈ 111 m: samples at 0, 10, …, 110 and the end
        let line = [(10.0, 44.0), (10.0, 44.001)];
        let samples = resample(&line, 10.0);
        assert_eq!(samples.first().map(|s| s.0), Some(0.0));
        assert_eq!(samples[1].0, 10.0);
        let (end_distance, end_point) = *samples.last().expect("samples");
        assert!((end_distance - 111.1).abs() < 0.5, "{end_distance}");
        assert_eq!(end_point, (10.0, 44.001));
        assert_eq!(samples.len(), 13);
    }

    #[test]
    fn reversing_swaps_climbing() {
        let line = LineElevation {
            profile: vec![(0.0, 100.0), (10.0, 110.0), (20.0, 105.0)],
            ascent_m: 10.0,
            descent_m: 5.0,
        };
        let back = line.reversed();
        assert_eq!(
            back.profile,
            vec![(0.0, 105.0), (10.0, 110.0), (20.0, 100.0)]
        );
        assert_eq!((back.ascent_m, back.descent_m), (5.0, 10.0));
        assert_eq!(line.range(), (100.0, 110.0));
    }
}
