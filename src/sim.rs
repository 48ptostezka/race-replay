//! Patrol state and position at time t (design doc, section 4), plus who is at a location.

use crate::geo::LatLon;
use crate::input::Hint;
use crate::model::{Interval, Outcome, Patrol, Race};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Before its first arrival.
    Waiting,
    /// At site `k` since `since`.
    AtSite { k: usize, since: f64 },
    /// On the leg from site `k` to `k + 1`, `frac` of the walking time done.
    Walking { k: usize, frac: f64 },
    /// Reached the finish before the limit.
    Finished,
    /// Reached the finish at or after the limit, or has no later times.
    Disqualified,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub kind: Kind,
    /// The location the patrol is at, for every state except walking.
    pub location: Option<usize>,
    /// Arrival at that location (sorting key of its cluster).
    pub since: f64,
    pub pos: LatLon,
    /// The current segment uses an estimated time.
    pub estimated: bool,
}

/// The interval whose span [arrive, next) contains t, if any.
pub fn interval_at(p: &Patrol, t: f64) -> Option<&Interval> {
    let i = p.intervals.partition_point(|iv| iv.arrive <= t);
    if i == 0 {
        return None;
    }
    let iv = &p.intervals[i - 1];
    (t < iv.next).then_some(iv)
}

pub fn state_at(race: &Race, cat: usize, pi: usize, t: f64) -> State {
    let c = &race.categories[cat];
    let p = &c.patrols[pi];
    let located = |k: usize, kind: Kind, since: f64, estimated: bool| {
        let loc = c.site_loc[k];
        State { kind, location: Some(loc), since, pos: race.locations[loc].pos, estimated }
    };
    if t < p.arrivals[0] {
        return located(0, Kind::Waiting, p.arrivals[0], p.estimated[0]);
    }
    if t >= p.arrivals[p.last] {
        let kind = match p.outcome {
            Outcome::Finished { .. } => Kind::Finished,
            Outcome::Late { .. } | Outcome::NoLaterTimes => Kind::Disqualified,
        };
        return located(p.last, kind, p.arrivals[p.last], p.estimated[p.last]);
    }
    match interval_at(p, t) {
        Some(iv) if t < iv.leave => located(iv.k, Kind::AtSite { k: iv.k, since: iv.arrive }, iv.arrive, iv.estimated),
        Some(iv) => {
            let frac = if iv.next > iv.leave { (t - iv.leave) / (iv.next - iv.leave) } else { 1.0 };
            State {
                kind: Kind::Walking { k: iv.k, frac },
                location: None,
                since: iv.leave,
                pos: c.legs[iv.k].at(frac),
                estimated: iv.estimated,
            }
        }
        // only for zero-length intervals at t exactly; treat as arrived at the later site
        None => {
            let k = p.arrivals.partition_point(|&a| a <= t).saturating_sub(1);
            located(k, Kind::AtSite { k, since: p.arrivals[k] }, p.arrivals[k], p.estimated[k])
        }
    }
}

/// A patrol at a location at time t.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Occupant {
    pub cat: usize,
    pub patrol: usize,
    pub state: State,
}

/// Patrols at location `loc` at time t, in arrival order, then category, then number.
/// `visible[cat]` filters hidden categories.
pub fn occupants(race: &Race, loc: usize, t: f64, visible: &[bool]) -> Vec<Occupant> {
    let mut out = Vec::new();
    for (ci, c) in race.categories.iter().enumerate() {
        if !visible.get(ci).copied().unwrap_or(true) || !c.site_loc.contains(&loc) {
            continue;
        }
        for pi in 0..c.patrols.len() {
            let st = state_at(race, ci, pi, t);
            if st.location == Some(loc) {
                out.push(Occupant { cat: ci, patrol: pi, state: st });
            }
        }
    }
    out.sort_by(|a, b| {
        a.state.since.total_cmp(&b.state.since).then(a.cat.cmp(&b.cat)).then(
            number_key(&race.categories[a.cat].patrols[a.patrol].id)
                .cmp(&number_key(&race.categories[b.cat].patrols[b.patrol].id)),
        )
    });
    out
}

/// Sort key for patrol ids: numbers numerically, everything else after them.
pub fn number_key(id: &str) -> (u64, String) {
    match id.trim().parse::<u64>() {
        Ok(v) => (v, String::new()),
        Err(_) => (u64::MAX, id.to_string()),
    }
}

/// Hints and solutions a patrol has taken by time t. A hint counts 20 min after the
/// arrival at its site and a solution 40 min after it (or when the patrol left the site,
/// if that was earlier). Sites the patrol never reached do not count.
pub fn taken_by(race: &Race, cat: usize, pi: usize, t: f64) -> (u32, u32) {
    let s = &race.settings;
    let p = &race.categories[cat].patrols[pi];
    let (mut hints, mut solutions) = (0, 0);
    for (k, &rec) in p.records.iter().enumerate() {
        if k == 0 || !matches!(rec, Hint::Hint | Hint::Solution) {
            continue;
        }
        let Some(&arrive) = p.arrivals.get(k) else { continue };
        let leave = p.intervals.get(k).map_or(f64::INFINITY, |iv| iv.leave);
        if t >= (arrive + s.hint_after).min(leave) {
            hints += 1;
        }
        if rec == Hint::Solution && t >= (arrive + s.solution_after).min(leave) {
            solutions += 1;
        }
    }
    (hints, solutions)
}

/// Distance the patrol still has to walk to the finish along its route at time t, metres.
pub fn remaining_m(race: &Race, cat: usize, pi: usize, t: f64) -> f64 {
    let c = &race.categories[cat];
    let p = &c.patrols[pi];
    let to = |k: usize| c.legs[..k].iter().map(|l| l.length_m).sum::<f64>();
    let done = match state_at(race, cat, pi, t).kind {
        Kind::Waiting => 0.0,
        Kind::AtSite { k, .. } => to(k),
        Kind::Walking { k, frac } => to(k) + frac * c.legs[k].length_m,
        Kind::Finished | Kind::Disqualified => to(p.last),
    };
    to(c.legs.len()) - done
}

/// A patrol's position in its category: places `first`..=`last` (equal when not tied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Rank { first: usize, last: usize },
    Disqualified,
}

impl Standing {
    /// "4." or "4.–5." or "diskv."
    pub fn label(&self) -> String {
        match *self {
            Standing::Rank { first, last } if first == last => format!("{first}."),
            Standing::Rank { first, last } => format!("{first}.–{last}."),
            Standing::Disqualified => "diskv.".to_string(),
        }
    }
}

/// Current position at time t: patrols in the finish first, in the order of the final
/// placement, then everyone else by the distance left to the finish (equal distance =
/// shared place). A disqualified patrol has no position from the moment it is disqualified.
pub fn standing(race: &Race, cat: usize, pi: usize, t: f64) -> Standing {
    let c = &race.categories[cat];
    let key = |q: usize| -> Option<(u8, i64, u32, u32)> {
        let p = &c.patrols[q];
        match (state_at(race, cat, q, t).kind, &p.outcome) {
            (Kind::Disqualified, _) => None,
            (Kind::Finished, Outcome::Finished { race_min, .. }) => Some((0, *race_min, p.solutions, p.hints)),
            _ => Some((1, remaining_m(race, cat, q, t).round() as i64, 0, 0)),
        }
    };
    let Some(mine) = key(pi) else { return Standing::Disqualified };
    let keys: Vec<_> = (0..c.patrols.len()).filter_map(key).collect();
    let better = keys.iter().filter(|k| **k < mine).count();
    let same = keys.iter().filter(|k| **k == mine).count();
    Standing::Rank { first: better + 1, last: better + same }
}

/// The final position: the placement, or "diskv.".
pub fn final_standing(race: &Race, cat: usize, pi: usize) -> Standing {
    match &race.categories[cat].patrols[pi].outcome {
        Outcome::Finished { place: Some(p), .. } => Standing::Rank { first: p.first, last: p.last },
        _ => Standing::Disqualified,
    }
}

/// Counts of a category at time t: (on course incl. waiting, finished, disqualified).
pub fn counts(race: &Race, cat: usize, t: f64) -> (usize, usize, usize) {
    let mut c = (0, 0, 0);
    for pi in 0..race.categories[cat].patrols.len() {
        match state_at(race, cat, pi, t).kind {
            Kind::Finished => c.1 += 1,
            Kind::Disqualified => c.2 += 1,
            _ => c.0 += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmt::hm;
    use crate::input::parse_time;
    use crate::model::tests::race_m;

    fn t(s: &str) -> f64 {
        parse_time(s).unwrap().unwrap()
    }

    fn idx(r: &Race, id: &str) -> usize {
        r.categories[0].patrols.iter().position(|p| p.id == id).unwrap()
    }

    #[test]
    fn states_at_13_24() {
        let r = race_m();
        let now = t("13:24");
        let st = |id: &str| state_at(&r, 0, idx(&r, id), now);
        assert_eq!(st("1").kind, Kind::Finished);
        assert_eq!(st("16").kind, Kind::Finished);
        // patrol 9 reached M5 at 13:02 and has a hint there: still at M5
        assert_eq!(st("9").kind, Kind::AtSite { k: 5, since: t("13:02") });
        assert_eq!(st("9").location, Some(r.categories[0].site_loc[5]));
        // everyone else in category M is somewhere between the start and the finish
        let (on, fin, dq) = counts(&r, 0, now);
        assert_eq!((on, fin, dq), (16, 5, 0));
    }

    #[test]
    fn before_start_and_after_finish() {
        let r = race_m();
        let p = idx(&r, "11");
        assert_eq!(state_at(&r, 0, p, t("09:00")).kind, Kind::Waiting);
        assert_eq!(state_at(&r, 0, p, t("16:14")).kind, Kind::AtSite { k: 7, since: t("15:43") });
        let fin = state_at(&r, 0, p, t("16:15"));
        assert_eq!(fin.kind, Kind::Disqualified); // 16:15 is the limit
        assert_eq!(fin.location, Some(r.categories[0].site_loc[8]));
        assert_eq!(counts(&r, 0, t("16:15")), (0, 19, 2));
    }

    #[test]
    fn walking_moves_along_the_leg() {
        let r = race_m();
        let p = idx(&r, "14");
        let iv = r.categories[0].patrols[p].intervals[4].clone();
        let mid = (iv.leave + iv.next) / 2.0;
        match state_at(&r, 0, p, mid).kind {
            Kind::Walking { k, frac } => {
                assert_eq!(k, 4);
                assert!((frac - 0.5).abs() < 1e-9);
            }
            k => panic!("{k:?} at {}", hm(mid)),
        }
        // at the moment of arrival the patrol is at the next site
        assert_eq!(state_at(&r, 0, p, iv.next).kind, Kind::AtSite { k: 5, since: iv.next });
    }

    #[test]
    fn hints_and_solutions_taken_so_far() {
        let r = race_m();
        let p = idx(&r, "4"); // records M1..MZ: N N T T - N -, arrivals M1 10:16, M3 12:35
        assert_eq!(taken_by(&r, 0, p, t("10:35")), (0, 0));
        assert_eq!(taken_by(&r, 0, p, t("10:36")), (1, 0)); // 20 min after the arrival at M1
                                                            // M3: totálka; arrival 12:35, the hint at 12:55, the solution at 13:15 (then it leaves)
        let before = taken_by(&r, 0, p, t("12:54"));
        assert_eq!(taken_by(&r, 0, p, t("12:55")), (before.0 + 1, before.1));
        assert_eq!(taken_by(&r, 0, p, t("13:15")), (before.0 + 1, before.1 + 1));
        // at the end the counts equal the totals
        let q = &r.categories[0].patrols[p];
        assert_eq!(taken_by(&r, 0, p, t("23:59")), (q.hints, q.solutions));
        // an impossible hint (patrol 6 at M6, next site after 18 min) counts when it leaves
        let p6 = idx(&r, "6");
        let iv = r.categories[0].patrols[p6].intervals[6].clone();
        assert!(iv.leave < iv.arrive + 20.0);
        let (h_before, _) = taken_by(&r, 0, p6, iv.leave - 0.01);
        assert_eq!(taken_by(&r, 0, p6, iv.leave).0, h_before + 1);
    }

    #[test]
    fn current_position() {
        let r = race_m();
        let st = |id: &str, hhmm: &str| standing(&r, 0, idx(&r, id), t(hhmm)).label();
        // at the start everyone shares the first place
        assert_eq!(st("1", "09:45"), "1.–21.");
        // at 13:24 the five finished patrols lead in their final order
        assert_eq!(st("16", "13:24"), "1.");
        assert_eq!(st("1", "13:24"), "2.");
        assert_eq!(st("8", "13:24"), "5.");
        // patrol 11 has a position during the race and is "diskv." from 16:15
        assert!(st("11", "16:00").ends_with('.') && st("11", "16:00") != "diskv.");
        assert_eq!(st("11", "16:15"), "diskv.");
        assert_eq!(final_standing(&r, 0, idx(&r, "11")).label(), "diskv.");
        assert_eq!(final_standing(&r, 0, idx(&r, "15")).label(), "3.");
        // at the end the current position equals the final one
        for id in ["1", "4", "16", "19"] {
            assert_eq!(st(id, "16:15"), final_standing(&r, 0, idx(&r, id)).label(), "{id}");
        }
        // remaining distance shrinks while walking
        let p = idx(&r, "14");
        let iv = r.categories[0].patrols[p].intervals[4].clone();
        assert!(remaining_m(&r, 0, p, iv.next - 1.0) < remaining_m(&r, 0, p, iv.leave + 1.0));
    }

    #[test]
    fn occupants_sorted_by_arrival() {
        let r = race_m();
        let loc = r.categories[0].site_loc[8];
        let occ = occupants(&r, loc, t("13:30"), &[true]);
        let ids: Vec<&str> = occ.iter().map(|o| r.categories[0].patrols[o.patrol].id.as_str()).collect();
        // finished by 13:30, in order of arrival at Cíl: 16 12:35, 1 12:48, 15 12:56, 2 12:57, 8 13:23, 7 13:27;
        // patrol 10 reached MZ (same spot) at 13:25 and is still there until Cíl 13:36
        assert_eq!(ids, vec!["16", "1", "15", "2", "8", "10", "7"]);
        assert!(occupants(&r, loc, t("13:30"), &[false]).is_empty());
    }
}
