//! Preprocessing on load (design doc, section 3): fill missing times, snap sites to the
//! route, merge shared locations, split every interval into stay and walk, and compute
//! hint totals, outcomes and placements. Everything is computed once; the replay only
//! looks values up (see `sim`). User-facing note texts are Czech.

use std::collections::{BTreeMap, HashMap};

use crate::fmt::{dec, hm};
use crate::geo::{cumulative_m, distance_m, mean, LatLon};
use crate::input::{
    self, norm, parse_csv, parse_gpx, parse_hint, parse_sites, parse_time, CategoryManifest, Hint, Manifest,
};

/// Walking faster than this after the split gets a note.
pub const MAX_SPEED_KMH: f64 = 12.0;
/// Walking slower than this (when the stay is already at its maximum) gets a note.
pub const MIN_SPEED_KMH: f64 = 2.0;
/// A site further than this from its route gets a note.
pub const SNAP_WARN_M: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
}

/// A data note shown under "Poznámky k datům".
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub level: Level,
    pub cat: Option<usize>,
    pub patrol: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub speed_kmh: f64,
    pub hint_after: f64,
    pub solution_after: f64,
    pub merge_radius_m: f64,
}

impl Settings {
    pub fn from_manifest(m: &Manifest) -> Self {
        Self {
            speed_kmh: m.speed_kmh,
            hint_after: m.hint_after_min,
            solution_after: m.solution_after_min,
            merge_radius_m: m.merge_radius_m,
        }
    }

    /// Allowed stay at a site, minutes, for a hint record (min, max).
    pub fn stay_bounds(&self, h: Hint) -> (f64, f64) {
        match h {
            Hint::None => (0.0, self.hint_after),
            Hint::Hint => (self.hint_after, self.solution_after),
            // a totálka is the cipher's solution: the patrol leaves right after it
            Hint::Solution => (self.solution_after, self.solution_after),
            Hint::Unknown => (0.0, f64::INFINITY),
        }
    }

    /// The allowed stay in words, for data notes.
    pub fn stay_text(&self, h: Hint) -> String {
        match h {
            Hint::None => format!("pobyt nejvýš {} min", self.hint_after.round()),
            Hint::Hint => format!("pobyt {}–{} min", self.hint_after.round(), self.solution_after.round()),
            Hint::Solution => format!("pobyt právě {} min", self.solution_after.round()),
            Hint::Unknown => "pobyt libovolně dlouhý".to_string(),
        }
    }
}

/// The route between two consecutive sites.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    pub points: Vec<LatLon>,
    pub cum: Vec<f64>,
    pub length_m: f64,
}

impl Leg {
    pub fn new(points: Vec<LatLon>) -> Self {
        let cum = cumulative_m(&points);
        let length_m = *cum.last().unwrap_or(&0.0);
        Self { points, cum, length_m }
    }

    /// The point at `frac` (0..1) of the leg's length, measured along the track.
    pub fn at(&self, frac: f64) -> LatLon {
        if self.points.len() == 1 || self.length_m <= 0.0 {
            return self.points[0];
        }
        let target = frac.clamp(0.0, 1.0) * self.length_m;
        let i = self.cum.partition_point(|&d| d < target).clamp(1, self.points.len() - 1);
        let (d0, d1) = (self.cum[i - 1], self.cum[i]);
        let t = if d1 > d0 { (target - d0) / (d1 - d0) } else { 0.0 };
        let (p, q) = (self.points[i - 1], self.points[i]);
        LatLon::new(p.lat + (q.lat - p.lat) * t, p.lon + (q.lon - p.lon) * t)
    }
}

/// One interval: arrival at site k -> arrival at site k+1, split into stay and walk.
#[derive(Clone, Debug, PartialEq)]
pub struct Interval {
    pub k: usize,
    pub arrive: f64,
    pub leave: f64,
    pub next: f64,
    pub stay: f64,
    pub walk: f64,
    /// Walking speed after the split; None when there is no walking or no distance.
    pub speed_kmh: Option<f64>,
    /// Either end is an estimated time.
    pub estimated: bool,
    /// The hint record as read (the start's is always Unknown).
    pub record: Hint,
    /// The record actually used for the split (an impossible one becomes Unknown).
    pub used: Hint,
    /// The walking speed is outside the plausible range (a note exists).
    pub speed_note: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// First and last place of a tie group (equal when not tied).
    pub first: usize,
    pub last: usize,
    /// How many patrols of the category are ranked.
    pub ranked: usize,
}

impl Place {
    /// "3. místo" or "4.–5. místo"
    pub fn label(&self) -> String {
        if self.first == self.last {
            format!("{}. místo", self.first)
        } else {
            format!("{}.–{}. místo", self.first, self.last)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Reached the finish before the limit.
    Finished { at: f64, race_min: i64, place: Option<Place> },
    /// Reached the finish at or after the limit: disqualified.
    Late { at: f64 },
    /// No times after `last`: disqualified.
    NoLaterTimes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Patrol {
    pub id: String,
    /// Arrival times 0..=last, made non-decreasing.
    pub arrivals: Vec<f64>,
    pub estimated: Vec<bool>,
    /// Index of the last site with a time.
    pub last: usize,
    /// Hint record per site (all sites; start and finish are always Unknown).
    pub records: Vec<Hint>,
    pub intervals: Vec<Interval>,
    /// Hints taken over the whole race (a totálka includes its hint).
    pub hints: u32,
    pub solutions: u32,
    /// Sites with an unknown record (between the start and the finish).
    pub unknown: u32,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub color: String,
    pub limit: Option<f64>,
    /// Assumed walking speed of this category, km/h.
    pub speed_kmh: f64,
    pub sites: Vec<String>,
    pub site_pos: Vec<LatLon>,
    /// Optional explicit shared-location name per site (`misto` column).
    pub site_misto: Vec<Option<String>>,
    /// Location index of every site (filled when locations are merged).
    pub site_loc: Vec<usize>,
    pub legs: Vec<Leg>,
    pub route: Vec<LatLon>,
    pub patrols: Vec<Patrol>,
}

impl Category {
    pub fn finish(&self) -> usize {
        self.sites.len() - 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocKind {
    Start,
    Control,
    Finish,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub pos: LatLon,
    /// (category, site index), sorted.
    pub members: Vec<(usize, usize)>,
    /// Label parts: (category or None for a name shared by several categories, name).
    pub label: Vec<(Option<usize>, String)>,
    pub kind: LocKind,
}

impl Location {
    pub fn label_text(&self) -> String {
        self.label.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(" · ")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    pub title: String,
    pub settings: Settings,
    pub categories: Vec<Category>,
    pub locations: Vec<Location>,
    pub notes: Vec<Note>,
    pub t0: f64,
    pub t1: f64,
}

/// The raw texts of one category's files (an Err holds why a file could not be fetched).
pub struct CategoryInput {
    pub manifest: CategoryManifest,
    pub times: Result<String, String>,
    pub hints: Option<Result<String, String>>,
    pub route: Result<String, String>,
    pub sites: Result<String, String>,
}

struct Notes(Vec<Note>);

impl Notes {
    fn add(&mut self, level: Level, cat: Option<usize>, patrol: Option<usize>, text: String) {
        self.0.push(Note { level, cat, patrol, text });
    }
}

/// Build the whole race. Fails only when no category can be shown.
pub fn build(manifest: &Manifest, inputs: Vec<CategoryInput>) -> Result<Race, String> {
    let settings = Settings::from_manifest(manifest);
    let mut notes = Notes(Vec::new());
    let mut categories = Vec::new();
    for input in inputs {
        let ci = categories.len();
        if let Some(c) = build_category(ci, input, &settings, &mut notes) {
            categories.push(c);
        }
    }
    if categories.is_empty() {
        let why: Vec<String> = notes.0.iter().filter(|n| n.level == Level::Error).map(|n| n.text.clone()).collect();
        return Err(format!("Žádnou kategorii nelze zobrazit. {}", why.join(" ")));
    }
    let locations = merge_locations(&mut categories, &settings, &mut notes);
    let t0 = categories.iter().flat_map(|c| c.patrols.iter().map(|p| p.arrivals[0])).fold(f64::INFINITY, f64::min);
    let t1 =
        categories.iter().flat_map(|c| c.patrols.iter().map(|p| p.arrivals[p.last])).fold(f64::NEG_INFINITY, f64::max);
    let (t0, t1) = if t0.is_finite() { (t0.floor(), t1.ceil().max(t0.floor() + 1.0)) } else { (0.0, 1.0) };
    let mut notes = notes.0;
    notes.sort_by_key(|n| (n.level, n.cat, n.patrol));
    Ok(Race { title: manifest.title.clone(), settings, categories, locations, notes, t0, t1 })
}

fn build_category(ci: usize, input: CategoryInput, s: &Settings, notes: &mut Notes) -> Option<Category> {
    let m = &input.manifest;
    let cid = m.id.clone();
    macro_rules! fail {
        ($($arg:tt)*) => {{
            notes.add(Level::Error, Some(ci), None, format!("Kategorie {cid}: {} Kategorie se nezobrazí.", format!($($arg)*)));
            return None;
        }};
    }

    // Files.
    let times_text = match input.times {
        Ok(t) => t,
        Err(e) => fail!("soubor s časy {} nelze načíst ({e}).", m.times),
    };
    let table = match parse_csv(&times_text) {
        Ok(t) if t.header.len() >= 3 => t,
        Ok(_) => fail!("soubor s časy musí mít sloupec s hlídkou a aspoň dvě stanoviště."),
        Err(e) => fail!("soubor s časy: {e}"),
    };
    let sites: Vec<String> = table.header[1..].to_vec();
    let n = sites.len();
    let site_rows = match input.sites.and_then(|t| parse_sites(&t)) {
        Ok(r) => r,
        Err(e) => fail!("stanoviště: {e}"),
    };
    let mut site_pos = Vec::with_capacity(n);
    let mut site_misto = Vec::with_capacity(n);
    let mut missing = Vec::new();
    for name in &sites {
        match site_rows.iter().find(|r| norm(&r.name) == norm(name)) {
            Some(r) => {
                site_pos.push(r.pos);
                site_misto.push(r.misto.clone());
            }
            None => missing.push(name.clone()),
        }
    }
    if !missing.is_empty() {
        fail!("v souboru se stanovišti chybí {}.", missing.join(", "));
    }
    for r in &site_rows {
        if !sites.iter().any(|sn| norm(sn) == norm(&r.name)) {
            notes.add(
                Level::Warn,
                Some(ci),
                None,
                format!("Kategorie {cid}: stanoviště {} není v souboru s časy, ignoruje se.", r.name),
            );
        }
    }
    let route = match input.route.and_then(|t| parse_gpx(&t)) {
        Ok(r) => r,
        Err(e) => fail!("trasa: {e}"),
    };
    let limit = match &m.limit {
        None => None,
        Some(l) => match parse_time(l) {
            Ok(v) => v,
            Err(()) => {
                notes.add(
                    Level::Warn,
                    Some(ci),
                    None,
                    format!("Kategorie {cid}: limit „{l}“ není čas, diskvalifikace za limit se nepočítá."),
                );
                None
            }
        },
    };

    let speed_kmh = match m.speed_kmh {
        None => s.speed_kmh,
        Some(v) if v > 0.0 => v,
        Some(v) => {
            notes.add(
                Level::Warn,
                Some(ci),
                None,
                format!("Kategorie {cid}: rychlost {v} km/h není kladná, použije se {} km/h.", dec(s.speed_kmh, 1)),
            );
            s.speed_kmh
        }
    };
    let s = &Settings { speed_kmh, ..s.clone() };

    // Times.
    struct Raw {
        id: String,
        t: Vec<Option<f64>>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (ri, row) in table.rows.iter().enumerate() {
        let mut id = row.first().cloned().unwrap_or_default();
        if id.is_empty() {
            id = format!("řádek {}", ri + 2);
        }
        if seen.contains_key(&id) {
            notes.add(Level::Error, Some(ci), None, format!("Kategorie {cid}: hlídka {id} je v souboru s časy dvakrát; druhý výskyt se zobrazí jako „{id} (řádek {})“.", ri + 2));
            id = format!("{id} (řádek {})", ri + 2);
        }
        seen.insert(id.clone(), raws.len());
        let mut t = Vec::with_capacity(n);
        for (k, site) in sites.iter().enumerate() {
            let cell = row.get(k + 1).map(String::as_str).unwrap_or("");
            match parse_time(cell) {
                Ok(v) => t.push(v),
                Err(()) => {
                    notes.add(
                        Level::Error,
                        Some(ci),
                        None,
                        format!("Hlídka {cid} {id}: „{cell}“ u {site} není čas, bere se jako chybějící."),
                    );
                    t.push(None);
                }
            }
        }
        if t.iter().all(Option::is_none) {
            notes.add(Level::Warn, Some(ci), None, format!("Hlídka {cid} {id}: nemá žádné časy, nezobrazí se."));
            seen.remove(&id);
            continue;
        }
        raws.push(Raw { id, t });
    }
    if raws.is_empty() {
        fail!("žádná hlídka nemá platný čas.");
    }

    // Typical leg durations for filling gaps.
    let mut med = vec![f64::NAN; n - 1];
    for (k, m_) in med.iter_mut().enumerate() {
        let mut g: Vec<f64> = raws
            .iter()
            .filter_map(|r| match (r.t[k], r.t[k + 1]) {
                (Some(a), Some(b)) if b > a => Some(b - a),
                _ => None,
            })
            .collect();
        if !g.is_empty() {
            g.sort_by(|a, b| a.total_cmp(b));
            *m_ = if g.len() % 2 == 1 { g[g.len() / 2] } else { (g[g.len() / 2 - 1] + g[g.len() / 2]) / 2.0 };
        }
    }
    let mut start_freq: BTreeMap<i64, usize> = BTreeMap::new();
    for r in &raws {
        if let Some(v) = r.t[0] {
            *start_freq.entry((v * 60.0).round() as i64).or_default() += 1;
        }
    }
    let common_start = start_freq.iter().max_by_key(|(_, c)| **c).map(|(v, _)| *v as f64 / 60.0);

    // Hints.
    let mut records: Vec<Vec<Hint>> = vec![vec![Hint::Unknown; n]; raws.len()];
    match &input.hints {
        None => notes.add(
            Level::Info,
            Some(ci),
            None,
            format!("Kategorie {cid} nemá soubor s nápovědami; všechny záznamy jsou neznámé."),
        ),
        Some(Err(e)) => notes.add(
            Level::Warn,
            Some(ci),
            None,
            format!("Kategorie {cid}: soubor s nápovědami nelze načíst ({e}); všechny záznamy jsou neznámé."),
        ),
        Some(Ok(text)) => match parse_csv(text) {
            Err(e) => notes.add(
                Level::Warn,
                Some(ci),
                None,
                format!("Kategorie {cid}: soubor s nápovědami: {e} Všechny záznamy jsou neznámé."),
            ),
            Ok(h) => read_hints(
                ci,
                &cid,
                &h,
                &sites,
                &raws.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
                &mut records,
                notes,
            ),
        },
    }

    // Snap sites to the route and cut it into legs.
    let (cuts, offsets) = snap_sites(&route, &site_pos);
    for (k, off) in offsets.iter().enumerate() {
        if *off > SNAP_WARN_M {
            notes.add(
                Level::Warn,
                Some(ci),
                None,
                format!("Kategorie {cid}: stanoviště {} je {} m od trasy.", sites[k], off.round()),
            );
        }
    }
    let legs: Vec<Leg> = (0..n - 1).map(|k| Leg::new(route[cuts[k]..=cuts[k + 1]].to_vec())).collect();
    let lengths_km: Vec<f64> = legs.iter().map(|l| l.length_m / 1000.0).collect();

    // Patrols.
    let mut patrols = Vec::with_capacity(raws.len());
    for (pi, r) in raws.iter().enumerate() {
        let pid = &r.id;
        let mut t = r.t.clone();
        let mut est = vec![false; n];
        if t[0].is_none() {
            let first = t.iter().flatten().next().copied().unwrap_or(0.0);
            let v = common_start.map_or(first, |c| c.min(first));
            t[0] = Some(v);
            est[0] = true;
            notes.add(
                Level::Info,
                Some(ci),
                Some(pi),
                format!("Hlídka {cid} {pid}: chybí čas startu, použit společný start {}.", hm(v)),
            );
        }
        let last = (0..n).rev().find(|&k| t[k].is_some()).unwrap_or(0);
        // fill gaps in proportion to the typical leg durations
        let known: Vec<usize> = (0..=last).filter(|&k| t[k].is_some()).collect();
        for w in known.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b == a + 1 {
                continue;
            }
            let (ta, tb) = (t[a].unwrap(), t[b].unwrap());
            let mut weights: Vec<f64> = med[a..b].to_vec();
            if weights.iter().any(|x| !x.is_finite()) || weights.iter().sum::<f64>() <= 0.0 {
                weights = vec![1.0; b - a];
            }
            let total: f64 = weights.iter().sum();
            let mut acc = 0.0;
            for k in a + 1..b {
                acc += weights[k - a - 1];
                let v = ta + (tb - ta) * acc / total;
                t[k] = Some(v);
                est[k] = true;
                notes.add(
                    Level::Info,
                    Some(ci),
                    Some(pi),
                    format!("Hlídka {cid} {pid}: chybí čas u {}, odhad {}.", sites[k], hm(v)),
                );
            }
        }
        // order: keep times as read, move with the running maximum
        let mut arrivals = Vec::with_capacity(last + 1);
        for k in 0..=last {
            let v = t[k].unwrap();
            if k > 0 && v < arrivals[k - 1] {
                notes.add(
                    Level::Error,
                    Some(ci),
                    Some(pi),
                    format!(
                        "Hlídka {cid} {pid}: {} ({}) je dřív než {} ({}); pro pohyb se použije pozdější čas.",
                        sites[k],
                        hm(v),
                        sites[k - 1],
                        hm(arrivals[k - 1])
                    ),
                );
            }
            arrivals.push(if k > 0 { v.max(arrivals[k - 1]) } else { v });
        }
        est.truncate(last + 1);
        let recs = &records[pi];
        let intervals = split_patrol(&arrivals, &est, recs, &lengths_km, s, &sites, |level, text| {
            notes.add(level, Some(ci), Some(pi), format!("Hlídka {cid} {pid}: {text}"));
        });

        // totals: a totálka also counts its hint
        let mut hints = 0;
        let mut solutions = 0;
        let mut unknown = 0;
        for (k, h) in recs.iter().enumerate() {
            if k == 0 || k == n - 1 {
                continue;
            }
            match h {
                Hint::Hint => hints += 1,
                Hint::Solution => {
                    hints += 1;
                    solutions += 1
                }
                Hint::Unknown if k <= last => unknown += 1,
                _ => {}
            }
        }
        let outcome = if last == n - 1 {
            let at = arrivals[last];
            if limit.is_some_and(|l| at >= l - 1e-9) {
                Outcome::Late { at }
            } else {
                Outcome::Finished { at, race_min: (at - arrivals[0]).round() as i64, place: None }
            }
        } else {
            notes.add(
                Level::Warn,
                Some(ci),
                Some(pi),
                format!(
                    "Hlídka {cid} {pid}: po {} nejsou další časy – diskvalifikována (chybí další časy).",
                    sites[last]
                ),
            );
            Outcome::NoLaterTimes
        };
        patrols.push(Patrol {
            id: pid.clone(),
            arrivals,
            estimated: est,
            last,
            records: recs.clone(),
            intervals,
            hints,
            solutions,
            unknown,
            outcome,
        });
    }
    assign_places(ci, &cid, &mut patrols, notes);

    Some(Category {
        id: m.id.clone(),
        name: m.name.clone(),
        color: m.color.clone(),
        limit,
        speed_kmh,
        sites,
        site_pos,
        site_misto,
        site_loc: Vec::new(),
        legs,
        route,
        patrols,
    })
}

fn read_hints(
    ci: usize,
    cid: &str,
    h: &input::Table,
    sites: &[String],
    ids: &[String],
    records: &mut [Vec<Hint>],
    notes: &mut Notes,
) {
    let n = sites.len();
    let mut cols: Vec<(usize, usize)> = Vec::new(); // (column, site)
    for (j, name) in h.header.iter().enumerate().skip(1) {
        match sites.iter().position(|s| norm(s) == norm(name)) {
            Some(k) => cols.push((j, k)),
            None => notes.add(
                Level::Warn,
                Some(ci),
                None,
                format!("Kategorie {cid}: sloupec „{name}“ v nápovědách neodpovídá žádnému stanovišti, ignoruje se."),
            ),
        }
    }
    let mut found = vec![false; ids.len()];
    let mut start_values = false;
    for row in &h.rows {
        let Some(id) = row.first() else { continue };
        let Some(pi) = ids.iter().position(|x| norm(x) == norm(id)) else {
            if !id.is_empty() {
                notes.add(
                    Level::Warn,
                    Some(ci),
                    None,
                    format!("Kategorie {cid}: hlídka {id} je v nápovědách, ale ne v souboru s časy."),
                );
            }
            continue;
        };
        found[pi] = true;
        for &(j, k) in &cols {
            let cell = row.get(j).map(String::as_str).unwrap_or("");
            let v = match parse_hint(cell) {
                Some(v) => v,
                None => {
                    notes.add(
                        Level::Warn,
                        Some(ci),
                        Some(pi),
                        format!(
                            "Hlídka {cid} {}: neznámá hodnota „{cell}“ u {}, bere se jako neznámý záznam.",
                            ids[pi], sites[k]
                        ),
                    );
                    Hint::Unknown
                }
            };
            if k == 0 {
                start_values |= v != Hint::Unknown;
            } else if k < n - 1 {
                records[pi][k] = v;
            }
        }
    }
    if start_values {
        notes.add(
            Level::Info,
            Some(ci),
            None,
            format!(
                "Kategorie {cid}: záznamy u startu ({}) se ignorují – na startu nejsou nápovědy ani totálky.",
                sites[0]
            ),
        );
    }
    for (pi, f) in found.iter().enumerate() {
        if !f {
            notes.add(
                Level::Warn,
                Some(ci),
                Some(pi),
                format!("Hlídka {cid} {} chybí v souboru s nápovědami; všechny její záznamy jsou neznámé.", ids[pi]),
            );
        }
    }
    let hint_sites: Vec<usize> = cols.iter().map(|&(_, k)| k).filter(|&k| k > 0 && k < n - 1).collect();
    for (pi, f) in found.iter().enumerate() {
        if !f {
            continue;
        }
        let empty: Vec<&str> =
            hint_sites.iter().filter(|&&k| records[pi][k] == Hint::Unknown).map(|&k| sites[k].as_str()).collect();
        if !empty.is_empty() {
            notes.add(
                Level::Info,
                Some(ci),
                Some(pi),
                format!("Hlídka {cid} {}: chybí záznam o nápovědě u {}.", ids[pi], empty.join(", ")),
            );
        }
    }
}

/// Cut points on the route for every site, in race order, minimising the total distance
/// (dynamic programming), so routes passing a spot twice are cut in the right place.
/// Returns the point index per site and each site's distance from it in metres.
pub fn snap_sites(route: &[LatLon], sites: &[LatLon]) -> (Vec<usize>, Vec<f64>) {
    let m = route.len();
    let d: Vec<Vec<f64>> = sites.iter().map(|s| route.iter().map(|p| distance_m(*s, *p)).collect()).collect();
    let mut dp = d[0].clone();
    let mut backs: Vec<Vec<usize>> = Vec::with_capacity(sites.len());
    for row in d.iter().skip(1) {
        let mut best = f64::INFINITY;
        let mut arg = 0;
        let mut back = vec![0; m];
        let mut next = vec![0.0; m];
        for i in 0..m {
            if dp[i] < best {
                best = dp[i];
                arg = i;
            }
            back[i] = arg;
            next[i] = row[i] + best;
        }
        backs.push(back);
        dp = next;
    }
    let mut idx = (0..m).min_by(|&a, &b| dp[a].total_cmp(&dp[b])).unwrap_or(0);
    let mut cuts = vec![idx];
    for back in backs.iter().rev() {
        idx = back[idx];
        cuts.push(idx);
    }
    cuts.reverse();
    let offsets = cuts.iter().enumerate().map(|(k, &i)| d[k][i]).collect();
    (cuts, offsets)
}

/// The agreed stay/walk split of one patrol (design doc, step 6).
/// `note(level, text)` receives notes about impossible hints and odd speeds.
pub fn split_patrol(
    arrivals: &[f64],
    est: &[bool],
    records: &[Hint],
    lengths_km: &[f64],
    s: &Settings,
    sites: &[String],
    mut note: impl FnMut(Level, String),
) -> Vec<Interval> {
    let mut out = Vec::new();
    for k in 0..arrivals.len().saturating_sub(1) {
        let (arrive, next) = (arrivals[k], arrivals[k + 1]);
        let gap = next - arrive;
        let estimated = est[k] || est[k + 1];
        let record = if k == 0 { Hint::Unknown } else { records[k] };
        let mut used = record;
        let (mut lo, mut hi) = s.stay_bounds(record);
        if k > 0 && gap < lo {
            let what = if record == Hint::Hint { "nápověda" } else { "totálka" };
            note(
                if estimated { Level::Warn } else { Level::Error },
                format!(
                    "{what} u {}, ale na {} dorazila už za {} min ({what} je možná nejdřív po {} min); záznam se pro výpočet nepoužije.",
                    sites[k],
                    sites[k + 1],
                    gap.round(),
                    lo.round()
                ),
            );
            used = Hint::Unknown;
            (lo, hi) = (0.0, f64::INFINITY);
        }
        let len = lengths_km[k];
        let (stay, want_over_max) = if len < 0.001 {
            (gap, false) // zero-length leg: all stay, no hint range
        } else {
            let expected = len / s.speed_kmh * 60.0;
            let want = gap - expected;
            let top = hi.min(gap);
            (want.max(lo).min(top), want > top + 1e-9)
        };
        let walk = gap - stay;
        let speed_kmh = if walk > 1e-9 && len >= 0.001 { Some(len / (walk / 60.0)) } else { None };
        let mut speed_note = false;
        if !estimated && gap > 0.0 && len >= 0.001 {
            let tag = format!("přesun {} → {}", sites[k], sites[k + 1]);
            let why = if k == 0 {
                "start, bez nápověd".to_string()
            } else {
                format!("{}, {}", used.label(), s.stay_text(used))
            };
            match speed_kmh {
                Some(v) if v > MAX_SPEED_KMH => {
                    speed_note = true;
                    note(Level::Warn, format!("{tag} vychází na {} km/h ({why}).", dec(v, 1)));
                }
                None => {
                    speed_note = true;
                    note(Level::Warn, format!("{tag}: na chůzi nezbývá žádný čas ({why})."));
                }
                Some(v) if v < MIN_SPEED_KMH && want_over_max => {
                    speed_note = true;
                    note(Level::Warn, format!("{tag} vychází jen na {} km/h ({why}).", dec(v, 1)));
                }
                _ => {}
            }
        }
        out.push(Interval {
            k,
            arrive,
            leave: arrive + stay,
            next,
            stay,
            walk,
            speed_kmh,
            estimated,
            record,
            used,
            speed_note,
        });
    }
    out
}

/// Placement in the category: shorter race time, then fewer solutions, then fewer hints;
/// a full tie is a shared place. Disqualified patrols are not ranked.
fn assign_places(ci: usize, cid: &str, patrols: &mut [Patrol], notes: &mut Notes) {
    let keys: Vec<(usize, (i64, u32, u32))> = patrols
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match p.outcome {
            Outcome::Finished { race_min, .. } => Some((i, (race_min, p.solutions, p.hints))),
            _ => None,
        })
        .collect();
    let ranked = keys.len();
    for &(i, key) in &keys {
        let better = keys.iter().filter(|(_, k)| *k < key).count();
        let same = keys.iter().filter(|(_, k)| *k == key).count();
        if let Outcome::Finished { place, .. } = &mut patrols[i].outcome {
            *place = Some(Place { first: better + 1, last: better + same, ranked });
        }
        let ties: Vec<&str> =
            keys.iter().filter(|(j, k)| *j != i && k.0 == key.0).map(|(j, _)| patrols[*j].id.as_str()).collect();
        if patrols[i].unknown > 0 && !ties.is_empty() {
            notes.add(
                Level::Info,
                Some(ci),
                Some(i),
                format!(
                    "Hlídka {cid} {}: má stejný čas jako {} {}; chybějící záznamy o nápovědě se pro pořadí počítají jako bez nápovědy.",
                    patrols[i].id,
                    if ties.len() == 1 { "hlídka" } else { "hlídky" },
                    ties.join(", ")
                ),
            );
        }
    }
}

/// Merge sites of all categories into physical locations (design doc, step 5).
fn merge_locations(cats: &mut [Category], s: &Settings, notes: &mut Notes) -> Vec<Location> {
    struct S {
        cat: usize,
        k: usize,
        pos: LatLon,
        misto: Option<String>,
    }
    let mut all = Vec::new();
    for (ci, c) in cats.iter().enumerate() {
        for k in 0..c.sites.len() {
            all.push(S { cat: ci, k, pos: c.site_pos[k], misto: c.site_misto[k].clone() });
        }
    }
    let n = all.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut j = i;
        while p[j] != r {
            let nx = p[j];
            p[j] = r;
            j = nx;
        }
        r
    }
    for i in 0..n {
        for j in i + 1..n {
            let same_name = matches!((&all[i].misto, &all[j].misto), (Some(a), Some(b)) if norm(a) == norm(b));
            if same_name || distance_m(all[i].pos, all[j].pos) < s.merge_radius_m {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[b] = a;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    groups.sort_by_key(|g| g.iter().map(|&i| (all[i].cat, all[i].k)).min());
    for c in cats.iter_mut() {
        c.site_loc = vec![0; c.sites.len()];
    }
    let mut locs = Vec::new();
    for g in groups {
        let li = locs.len();
        let mut members: Vec<(usize, usize)> = g.iter().map(|&i| (all[i].cat, all[i].k)).collect();
        members.sort();
        for &(c, k) in &members {
            cats[c].site_loc[k] = li;
        }
        let pos = mean(&g.iter().map(|&i| all[i].pos).collect::<Vec<_>>());
        let spread = g
            .iter()
            .flat_map(|&i| g.iter().map(move |&j| (i, j)))
            .map(|(i, j)| distance_m(all[i].pos, all[j].pos))
            .fold(0.0, f64::max);
        // label: per-category names first, names shared by several categories last
        let mut by_name: Vec<(String, Vec<usize>)> = Vec::new();
        for &(c, k) in &members {
            let name = cats[c].sites[k].clone();
            match by_name.iter_mut().find(|(n, _)| norm(n) == norm(&name)) {
                Some((_, cs)) => {
                    if !cs.contains(&c) {
                        cs.push(c)
                    }
                }
                None => by_name.push((name, vec![c])),
            }
        }
        let mut label: Vec<(Option<usize>, String)> =
            by_name.iter().filter(|(_, cs)| cs.len() == 1).map(|(n, cs)| (Some(cs[0]), n.clone())).collect();
        label.extend(by_name.iter().filter(|(_, cs)| cs.len() > 1).map(|(n, _)| (None, n.clone())));
        let kind = if members.iter().any(|&(c, k)| k == cats[c].finish()) {
            LocKind::Finish
        } else if members.iter().any(|&(_, k)| k == 0) {
            LocKind::Start
        } else {
            LocKind::Control
        };
        let loc = Location { pos, members, label, kind };
        if spread > 2.0 * s.merge_radius_m {
            notes.add(
                Level::Warn,
                None,
                None,
                format!(
                    "Místo {} spojuje stanoviště vzdálená až {} m od sebe; zkontrolujte souřadnice.",
                    loc.label_text(),
                    spread.round()
                ),
            );
        }
        locs.push(loc);
    }
    locs
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::input::parse_manifest;

    pub const RACE_TOML: &str = include_str!("../testdata/race.toml");
    pub const M_TIMES: &str = include_str!("../testdata/m_casy.csv");
    pub const M_HINTS: &str = include_str!("../testdata/m_napovedy.csv");
    pub const M_GPX: &str = include_str!("../testdata/trasa_m.gpx");
    pub const M_SITES: &str = include_str!("../testdata/m_stanoviste.csv");

    pub fn race_m() -> Race {
        let m = parse_manifest(RACE_TOML).unwrap();
        let input = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok(M_TIMES.into()),
            hints: Some(Ok(M_HINTS.into())),
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        build(&m, vec![input]).unwrap()
    }

    fn t(s: &str) -> f64 {
        parse_time(s).unwrap().unwrap()
    }

    fn patrol<'a>(r: &'a Race, id: &str) -> &'a Patrol {
        r.categories[0].patrols.iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn loads_category_m() {
        let r = race_m();
        let c = &r.categories[0];
        assert_eq!(c.patrols.len(), 21);
        assert_eq!(c.sites.len(), 9);
        assert_eq!(r.t0, t("09:45"));
        assert_eq!(r.t1, t("16:15"));
        // MZ and Cíl share a spot: one location; start is its own
        assert_eq!(c.site_loc[7], c.site_loc[8]);
        assert_eq!(r.locations.len(), 8);
        assert_eq!(r.locations[c.site_loc[8]].kind, LocKind::Finish);
        assert_eq!(r.locations[c.site_loc[0]].kind, LocKind::Start);
        assert_eq!(r.locations[c.site_loc[8]].label_text(), "MZ · Cíl");
        // MZ -> Cíl has length 0
        assert!(c.legs[7].length_m < 1.0);
    }

    #[test]
    fn fills_patrol_12_mz() {
        let r = race_m();
        let p = patrol(&r, "12");
        assert!(p.estimated[7]);
        // 15:34 -> 16:15 split by the median leg times 32 and 5 min, as race_analysis.py
        assert_eq!(hm(p.arrivals[7]), "16:10");
        assert!(r.notes.iter().any(|n| n.text.contains("chybí čas u MZ, odhad 16:10")));
    }

    #[test]
    fn totals_count_the_hint_before_a_solution() {
        let r = race_m();
        // patrol 4: N N T T - N -  -> hints 5, solutions 2
        let p = patrol(&r, "4");
        assert_eq!((p.hints, p.solutions, p.unknown), (5, 2, 0));
        let p = patrol(&r, "1");
        assert_eq!((p.hints, p.solutions), (0, 0));
        // patrol 12: MZ record empty -> one unknown
        assert_eq!(patrol(&r, "12").unknown, 1);
    }

    #[test]
    fn outcomes_and_places() {
        let r = race_m();
        assert_eq!(patrol(&r, "11").outcome, Outcome::Late { at: t("16:15") });
        assert!(matches!(patrol(&r, "12").outcome, Outcome::Late { .. }));
        let place = |id: &str| match &patrol(&r, id).outcome {
            Outcome::Finished { place: Some(p), .. } => p.label(),
            o => panic!("{id}: {o:?}"),
        };
        assert_eq!(place("16"), "1. místo"); // 170 min
        assert_eq!(place("1"), "2. místo"); // 183 min
        assert_eq!(place("15"), "3. místo"); // 191 min
        assert_eq!(place("2"), "4. místo"); // 192 min
        assert_eq!(place("4"), "18. místo"); // 359 min
        assert_eq!(place("19"), "19. místo"); // 361 min, the slowest that is not disqualified
        if let Outcome::Finished { place: Some(p), .. } = &patrol(&r, "16").outcome {
            assert_eq!(p.ranked, 19);
        }
        // a finish exactly at the limit is a real time: disqualified, no special note
        assert!(!r.notes.iter().any(|n| n.text.contains("přesně čas limitu")));
    }

    #[test]
    fn ties_break_on_solutions_then_hints() {
        let s = Settings { speed_kmh: 5.0, hint_after: 20.0, solution_after: 40.0, merge_radius_m: 15.0 };
        let mk = |id: &str, race_min: i64, h: u32, so: u32| Patrol {
            id: id.into(),
            arrivals: vec![0.0],
            estimated: vec![false],
            last: 0,
            records: vec![],
            intervals: vec![],
            hints: h,
            solutions: so,
            unknown: 0,
            outcome: Outcome::Finished { at: 0.0, race_min, place: None },
        };
        let mut ps =
            vec![mk("a", 100, 3, 1), mk("b", 100, 2, 1), mk("c", 100, 5, 0), mk("d", 90, 9, 9), mk("e", 100, 2, 1)];
        let mut notes = Notes(Vec::new());
        assign_places(0, "M", &mut ps, &mut notes);
        let label = |i: usize| match &ps[i].outcome {
            Outcome::Finished { place: Some(p), .. } => p.label(),
            _ => unreachable!(),
        };
        assert_eq!(label(3), "1. místo");
        assert_eq!(label(2), "2. místo"); // fewer solutions wins over fewer hints
        assert_eq!(label(1), "3.–4. místo");
        assert_eq!(label(4), "3.–4. místo");
        assert_eq!(label(0), "5. místo");
        let _ = s;
    }

    #[test]
    fn split_matches_the_agreed_rule() {
        // patrol 14 with the leg lengths used in the mockup
        let s = Settings { speed_kmh: 5.0, hint_after: 20.0, solution_after: 40.0, merge_radius_m: 15.0 };
        let a: Vec<f64> = ["09:45", "10:16", "10:38", "11:37", "12:46", "13:33", "13:56", "14:20", "14:25"]
            .iter()
            .map(|x| t(x))
            .collect();
        let rec = [
            Hint::Unknown,
            Hint::None,
            Hint::Hint,
            Hint::Hint,
            Hint::None,
            Hint::None,
            Hint::None,
            Hint::None,
            Hint::Unknown,
        ];
        let len = [1.0, 1.0, 2.4, 0.75, 1.4, 1.2, 0.4, 0.0];
        let sites: Vec<String> =
            ["M0", "M1", "M2", "M3", "M4", "M5", "M6", "MZ", "Cíl"].iter().map(|x| x.to_string()).collect();
        let mut notes = Vec::new();
        let iv = split_patrol(&a, &[false; 9], &rec, &len, &s, &sites, |l, tx| notes.push((l, tx)));
        assert_eq!(iv.len(), 8);
        assert!((iv[0].stay - 19.0).abs() < 1e-9); // start: unknown range
        assert_eq!(hm(iv[4].leave), "13:06"); // M4: no hint -> stay capped at 20 min
        assert!((iv[4].walk - 27.0).abs() < 1e-9);
        assert_eq!(dec(iv[4].speed_kmh.unwrap(), 1), "3,1");
        assert!((iv[3].stay - 40.0).abs() < 1e-9); // M3: hint -> at most 40
        assert_eq!(dec(iv[3].speed_kmh.unwrap(), 1), "1,6");
        assert!(iv[3].speed_note);
        assert_eq!(iv[7].stay, 5.0); // MZ -> Cíl has no length: all stay
        assert_eq!(iv[7].speed_kmh, None);
        assert!(notes.iter().any(|(_, tx)| tx.contains("přesun M3 → M4 vychází jen na 1,6 km/h")));
    }

    #[test]
    fn solution_means_exactly_40_minutes() {
        let s = Settings { speed_kmh: 5.0, hint_after: 20.0, solution_after: 40.0, merge_radius_m: 15.0 };
        let sites: Vec<String> = ["A", "B", "C", "D"].iter().map(|x| x.to_string()).collect();
        let rec = [Hint::Unknown, Hint::Solution, Hint::Solution, Hint::Unknown];
        // 1 km takes 12 min at 5 km/h. B: an 80 min interval, C: a 45 min interval.
        let a = [0.0, 10.0, 90.0, 135.0];
        let mut notes = Vec::new();
        let iv = split_patrol(&a, &[false; 4], &rec, &[0.5, 1.0, 1.0], &s, &sites, |l, tx| notes.push((l, tx)));
        // exactly 40 min at B even though the walk would need only 12: the walk takes the rest
        assert_eq!((iv[1].stay, iv[1].walk), (40.0, 40.0));
        assert!(notes
            .iter()
            .any(|(_, tx)| tx.contains("přesun B → C vychází jen na 1,5 km/h (totálka, pobyt právě 40 min)")));
        // exactly 40 min at C too, so only 5 min are left for 1 km (12 km/h, still no note)
        assert_eq!((iv[2].stay, iv[2].walk), (40.0, 5.0));
        assert!(!notes.iter().any(|(_, tx)| tx.contains("přesun C → D")));
    }

    #[test]
    fn speed_is_set_per_category() {
        let mut m = parse_manifest(RACE_TOML).unwrap();
        let input = |m: &Manifest| CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok(M_TIMES.into()),
            hints: Some(Ok(M_HINTS.into())),
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        let base = build(&m, vec![input(&m)]).unwrap();
        assert_eq!(base.categories[0].speed_kmh, 5.0);
        m.categories[0].speed_kmh = Some(3.0);
        let slow = build(&m, vec![input(&m)]).unwrap();
        assert_eq!(slow.categories[0].speed_kmh, 3.0);
        // patrol 1, M1 -> M2 with no hint: a slower assumed speed leaves less time for the stay
        let (b, w) = (&base.categories[0].patrols[0].intervals[1], &slow.categories[0].patrols[0].intervals[1]);
        assert!(w.stay < b.stay, "{} {}", w.stay, b.stay);
        // the race-wide value is the default when a category has none
        m.categories[0].speed_kmh = None;
        m.speed_kmh = 4.0;
        assert_eq!(build(&m, vec![input(&m)]).unwrap().categories[0].speed_kmh, 4.0);
    }

    #[test]
    fn impossible_hint_becomes_unknown() {
        let r = race_m();
        let p = patrol(&r, "6");
        let iv = &p.intervals[6]; // M6 -> MZ, 18 min with a hint
        assert_eq!(iv.record, Hint::Hint);
        assert_eq!(iv.used, Hint::Unknown);
        assert!(r
            .notes
            .iter()
            .any(|n| n.level == Level::Error && n.text.contains("nápověda u M6, ale na MZ dorazila už za 18 min")));
    }

    #[test]
    fn start_records_are_ignored() {
        let m = parse_manifest(RACE_TOML).unwrap();
        let hints = "Číslo hlídky,M0,M1\n1,nápověda,totálka\n";
        let input = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok("Číslo hlídky,M0,M1,MZ,Cíl\n1,09:45,10:30,11:00,11:05\n".into()),
            hints: Some(Ok(hints.into())),
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        let r = build(&m, vec![input]).unwrap();
        let p = &r.categories[0].patrols[0];
        assert_eq!(p.records[0], Hint::Unknown);
        assert_eq!((p.hints, p.solutions), (1, 1));
        assert!(r.notes.iter().any(|n| n.text.contains("záznamy u startu (M0) se ignorují")));
        // the start stay may be long without any hint: 45 min interval
        assert_eq!(p.intervals[0].record, Hint::Unknown);
    }

    #[test]
    fn snapping_handles_out_and_back() {
        // A -> B -> back past A -> C ; sites A, B, C must cut at 0, 2, 6 (not at 4 for A)
        let route: Vec<LatLon> =
            [(0.0, 0.0), (0.0, 0.001), (0.0, 0.002), (0.0, 0.001), (0.0, 0.0), (0.0005, 0.0), (0.001, 0.0)]
                .iter()
                .map(|&(a, b)| LatLon::new(49.0 + a, 16.0 + b))
                .collect();
        let sites = vec![route[0], route[2], route[6]];
        let (cuts, off) = snap_sites(&route, &sites);
        assert_eq!(cuts, vec![0, 2, 6]);
        assert!(off.iter().all(|&d| d < 0.01));
    }

    #[test]
    fn shared_locations_merge_across_categories() {
        let m = parse_manifest(&format!(
            "{RACE_TOML}\n[[category]]\nid = \"S\"\nname = \"Kategorie S\"\ncolor = \"#e0b000\"\nlimit = \"16:15\"\n\
                 times = \"s.csv\"\nroute = \"s.gpx\"\nsites = \"s.csv\"\n"
        ))
        .unwrap();
        let m_input = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok(M_TIMES.into()),
            hints: Some(Ok(M_HINTS.into())),
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        // S shares the start, M3 (5 m away) and the finish
        let s_sites = "stanoviste,lat,lon\nS0,49.2283375,16.6976417\nS1,49.2348300,16.6743700\nS2,49.240000,16.680000\nSZ,49.2460150,16.6760264\nCíl,49.2460150,16.6760264\n";
        let s_input = CategoryInput {
            manifest: m.categories[1].clone(),
            times: Ok("č,S0,S1,S2,SZ,Cíl\n1,09:50,10:40,11:30,12:10,12:12\n".into()),
            hints: None,
            route: Ok(M_GPX.into()),
            sites: Ok(s_sites.into()),
        };
        let r = build(&m, vec![m_input, s_input]).unwrap();
        let (cm, cs) = (&r.categories[0], &r.categories[1]);
        assert_eq!(cm.site_loc[0], cs.site_loc[0]);
        assert_eq!(cm.site_loc[3], cs.site_loc[1]);
        assert_eq!(r.locations[cm.site_loc[3]].label_text(), "M3 · S1");
        assert_eq!(r.locations[cm.site_loc[8]].label_text(), "MZ · SZ · Cíl");
        assert_eq!(r.locations[cm.site_loc[8]].label[2], (None, "Cíl".to_string()));
        assert_eq!(r.locations.len(), 8 + 1); // only S2 is new
        assert!(r.notes.iter().any(|n| n.text.contains("Kategorie S nemá soubor s nápovědami")));
    }

    #[test]
    fn broken_category_is_skipped_not_fatal() {
        let m = parse_manifest(RACE_TOML).unwrap();
        let good = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok(M_TIMES.into()),
            hints: Some(Ok(M_HINTS.into())),
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        let bad = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Ok(M_TIMES.into()),
            hints: None,
            route: Ok(M_GPX.into()),
            sites: Ok("stanoviste,lat,lon\nM0,49.2,16.6\n".into()),
        };
        let r = build(&m, vec![good, bad]).unwrap();
        assert_eq!(r.categories.len(), 1);
        assert!(r.notes.iter().any(|n| n.level == Level::Error && n.text.contains("chybí M1")));
        let only_bad = CategoryInput {
            manifest: m.categories[0].clone(),
            times: Err("HTTP 404".into()),
            hints: None,
            route: Ok(M_GPX.into()),
            sites: Ok(M_SITES.into()),
        };
        assert!(build(&m, vec![only_bad]).is_err());
    }
}

#[cfg(test)]
mod dump {
    /// Prints leg lengths and the split, for comparing with race_analysis.py.
    #[test]
    #[ignore]
    fn dump_split() {
        let r = super::tests::race_m();
        let c = &r.categories[0];
        let legs: Vec<String> = (0..c.legs.len())
            .map(|k| format!("{}-{}={:.6}", c.sites[k], c.sites[k + 1], c.legs[k].length_m / 1000.0))
            .collect();
        println!("LEGS {}", legs.join(","));
        for p in &c.patrols {
            for iv in &p.intervals {
                println!("SPLIT {},{},{:.3},{:.3}", p.id, c.sites[iv.k], iv.stay, iv.walk);
            }
        }
    }
}
