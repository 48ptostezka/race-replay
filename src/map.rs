//! The Leaflet map: cached Mapy.com tiles as they are, and on top the routes, the
//! locations and one dot per patrol, moved every frame (design doc, section 6).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::geo::{project, unproject, LatLon};
use crate::input::TileManifest;
use crate::layout::{nudge, ring_slots};
use crate::leaflet::{self as l, escape, js, latlng, Layer, Map};
use crate::model::{LocKind, Race};
use crate::sim::{number_key, state_at, Kind};
use crate::state::Sel;

pub const COURSE: &str = "#a8126f";
pub const AMBER: &str = "#c98500";
pub const INK: &str = "#1d1f22";
const TILES: &str = "tiles";
const OVERZOOM: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Waiting,
    Active,
    Finished,
    Disqualified,
}

struct Dot {
    edge: Layer,
    dot: Layer,
    look: Option<(Look, bool)>,
}

struct Route {
    casing: Layer,
    line: Layer,
    strong: Option<bool>,
}

pub struct MapView {
    race: Rc<Race>,
    map: Map,
    dots: Vec<Vec<Dot>>,
    routes: Vec<Route>,
    halo: Layer,
    halo_on: bool,
    shown: Vec<bool>,
    last: Option<(f64, Option<Sel>, Vec<bool>)>,
    _closures: Vec<Closure<dyn FnMut(JsValue)>>,
}

pub type Holder = Rc<RefCell<Option<MapView>>>;

/// A dot waiting for its cluster slot: (arrival, category, number key, patrol).
type Member = (f64, usize, (u64, String), usize);
/// A walking dot: (category, patrol, (x, y, normal x, normal y) in pixels).
type Walker = (usize, usize, (f64, f64, f64, f64));

/// Build the map into `el` and store it in `holder`. `on_select` receives clicks.
pub fn create(
    el: &web_sys::HtmlElement,
    race: Rc<Race>,
    tiles: Option<&TileManifest>,
    on_select: Rc<dyn Fn(Option<Sel>)>,
    holder: Holder,
) {
    let mut closures: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();
    let max_zoom = tiles.map_or(19, |t| t.max_zoom + OVERZOOM);
    let min_zoom = tiles.map_or(10, |t| t.min_zoom);
    let mut opts = json!({ "preferCanvas": true, "minZoom": min_zoom, "maxZoom": max_zoom });
    if let Some(t) = tiles {
        let [[s, w], [n, e]] = t.bounds;
        let (ph, pw) = ((n - s) * 0.15, (e - w) * 0.15);
        opts["maxBounds"] = json!([[s - ph, w - pw], [n + ph, e + pw]]);
        opts["maxBoundsViscosity"] = json!(0.8);
    }
    let map = l::new_map(el, &js(opts));

    // Tiles from the local folder, one base layer per cached map set.
    if let Some(t) = tiles {
        let base = js_sys::Object::new();
        let mut default = None;
        for (name, info) in &t.mapsets {
            let layer = l::tile_layer(
                &format!("{TILES}/{name}/{{z}}/{{x}}/{{y}}.{}", info.ext),
                &js(json!({
                    "minZoom": t.min_zoom, "maxZoom": t.max_zoom + OVERZOOM,
                    "minNativeZoom": t.min_zoom, "maxNativeZoom": t.max_zoom,
                    "bounds": t.bounds, "tileSize": 256, "noWrap": true,
                    "attribution": t.attribution_html,
                })),
            );
            let _ = js_sys::Reflect::set(&base, &JsValue::from_str(mapset_label(name)), &layer);
            if *name == t.default_mapset || default.is_none() {
                default = Some(layer);
            }
        }
        if let Some(layer) = default {
            layer.add_to(&map);
        }
        if t.mapsets.len() > 1 {
            l::control_layers(&base, &JsValue::UNDEFINED, &js(json!({ "collapsed": true }))).add_to(&map);
        }
    }
    l::control_scale(&js(json!({ "imperial": false, "position": "bottomright" }))).add_to(&map);

    // Routes.
    let mut routes = Vec::new();
    let (mut bs, mut bw, mut bn, mut be) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in &race.categories {
        let pts: Vec<[f64; 2]> = c.route.iter().map(|p| [p.lat, p.lon]).collect();
        for p in &c.route {
            (bs, bn, bw, be) = (bs.min(p.lat), bn.max(p.lat), bw.min(p.lon), be.max(p.lon));
        }
        let casing = l::polyline(
            &js(json!(pts)),
            &js(json!({ "color": "#ffffff", "opacity": 0.7, "weight": 6, "interactive": false, "lineJoin": "round" })),
        );
        let line = l::polyline(
            &js(json!(pts)),
            &js(json!({ "color": c.color, "opacity": 0.55, "weight": 3, "interactive": false, "lineJoin": "round" })),
        );
        casing.add_to(&map);
        line.add_to(&map);
        routes.push(Route { casing, line, strong: None });
    }

    // Locations: orienteering control circles, start triangle, finish double circle.
    for (li, loc) in race.locations.iter().enumerate() {
        let layer = match loc.kind {
            LocKind::Control => l::circle_marker(
                &latlng(loc.pos),
                &js(
                    json!({ "radius": 11, "color": COURSE, "weight": 2.4, "fillColor": "#ffffff", "fillOpacity": 0.35, "bubblingMouseEvents": false }),
                ),
            ),
            LocKind::Start | LocKind::Finish => {
                let svg = if loc.kind == LocKind::Start {
                    format!("<svg width='30' height='30' viewBox='0 0 30 30'><polygon points='15,2 27,23 3,23' fill='rgba(255,255,255,0.35)' stroke='{COURSE}' stroke-width='2.6'/></svg>")
                } else {
                    format!("<svg width='30' height='30' viewBox='0 0 30 30'><circle cx='15' cy='15' r='12' fill='rgba(255,255,255,0.35)' stroke='{COURSE}' stroke-width='2.4'/><circle cx='15' cy='15' r='7.5' fill='none' stroke='{COURSE}' stroke-width='2.4'/></svg>")
                };
                let icon = l::div_icon(&js(
                    json!({ "html": svg, "className": "loc-icon", "iconSize": [30, 30], "iconAnchor": [15, 15] }),
                ));
                let opts = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&opts, &"icon".into(), &icon);
                let _ = js_sys::Reflect::set(&opts, &"keyboard".into(), &JsValue::FALSE);
                l::marker(&latlng(loc.pos), &opts)
            }
        };
        let label: String = loc
            .label
            .iter()
            .map(|(c, name)| {
                let color = c.map_or(COURSE, |c| race.categories[c].color.as_str());
                format!("<span style=\"color:{}\">{}</span>", escape(color), escape(name))
            })
            .collect::<Vec<_>>()
            .join("<span class=\"sep\"> · </span>");
        layer.bind_tooltip(&label, &js(json!({ "permanent": true, "direction": "right", "offset": [22, 0], "className": "loc-label", "interactive": false })));
        let cb = on_select.clone();
        let click = Closure::<dyn FnMut(JsValue)>::new(move |_| cb(Some(Sel::Location(li))));
        layer.layer_on("click", click.as_ref().unchecked_ref());
        closures.push(click);
        layer.add_to(&map);
    }

    // One dot per patrol (with a thin dark edge underneath, for light colours).
    let mut dots = Vec::new();
    for (ci, c) in race.categories.iter().enumerate() {
        let mut row = Vec::new();
        for (pi, _) in c.patrols.iter().enumerate() {
            let start = race.locations[c.site_loc[0]].pos;
            let edge = l::circle_marker(
                &latlng(start),
                &js(
                    json!({ "radius": 8.5, "stroke": false, "fillColor": INK, "fillOpacity": 0.5, "interactive": false }),
                ),
            );
            let dot = l::circle_marker(
                &latlng(start),
                &js(
                    json!({ "radius": 6.5, "color": "#ffffff", "weight": 1.5, "fillColor": c.color, "fillOpacity": 1, "bubblingMouseEvents": false }),
                ),
            );
            let cb = on_select.clone();
            let click = Closure::<dyn FnMut(JsValue)>::new(move |_| cb(Some(Sel::Patrol { cat: ci, patrol: pi })));
            dot.layer_on("click", click.as_ref().unchecked_ref());
            closures.push(click);
            edge.add_to(&map);
            dot.add_to(&map);
            row.push(Dot { edge, dot, look: None });
        }
        dots.push(row);
    }

    // Halo and name tag of the selected patrol.
    let halo = l::circle_marker(
        &latlng(race.locations[0].pos),
        &js(json!({ "radius": 14, "weight": 1.5, "fillOpacity": 0.18, "interactive": false })),
    );
    halo.bind_tooltip(
        "",
        &js(json!({ "permanent": true, "direction": "top", "offset": [0, -12], "className": "sel-tag" })),
    );

    // A click on the empty map closes the panel.
    let cb = on_select.clone();
    let click = Closure::<dyn FnMut(JsValue)>::new(move |_| cb(None));
    map.map_on("click", click.as_ref().unchecked_ref());
    closures.push(click);

    // Clusters are laid out in pixels: redo them after zooming.
    let h = holder.clone();
    let zoom = Closure::<dyn FnMut(JsValue)>::new(move |_| {
        if let Ok(mut g) = h.try_borrow_mut() {
            if let Some(view) = g.as_mut() {
                view.relayout();
            }
        }
    });
    map.map_on("zoomend", zoom.as_ref().unchecked_ref());
    closures.push(zoom);

    if bs <= bn {
        map.fit_bounds(&js(json!([[bs, bw], [bn, be]])), &js(json!({ "padding": [30, 30] })));
    }
    let n = race.categories.len();
    *holder.borrow_mut() = Some(MapView {
        race,
        map,
        dots,
        routes,
        halo,
        halo_on: false,
        shown: vec![true; n],
        last: None,
        _closures: closures,
    });
}

fn mapset_label(name: &str) -> &str {
    match name {
        "outdoor" => "Turistická",
        "basic" => "Základní",
        "winter" => "Zimní",
        "aerial" => "Letecká",
        "names-overlay" => "Popisky",
        other => other,
    }
}

impl MapView {
    fn relayout(&mut self) {
        if let Some((t, sel, visible)) = self.last.clone() {
            self.update(t, sel, &visible);
        }
    }

    /// Move every dot to time t.
    pub fn update(&mut self, t: f64, sel: Option<Sel>, visible: &[bool]) {
        self.last = Some((t, sel, visible.to_vec()));
        let race = self.race.clone();
        let zoom = self.map.get_zoom();

        // Show or hide whole categories.
        for (ci, shown) in self.shown.iter_mut().enumerate() {
            let want = visible.get(ci).copied().unwrap_or(true);
            if want != *shown {
                let r = &self.routes[ci];
                for d in &self.dots[ci] {
                    if want {
                        d.edge.add_to(&self.map);
                        d.dot.add_to(&self.map);
                    } else {
                        d.edge.remove();
                        d.dot.remove();
                    }
                }
                if want {
                    r.casing.add_to(&self.map);
                    r.line.add_to(&self.map);
                } else {
                    r.casing.remove();
                    r.line.remove();
                }
                *shown = want;
            }
        }

        // States; group dots by location, collect walking dots.
        let mut groups: HashMap<usize, Vec<Member>> = HashMap::new();
        let mut walkers: Vec<Walker> = Vec::new();
        let mut looks: Vec<Vec<(Look, bool)>> = Vec::new();
        for (ci, c) in race.categories.iter().enumerate() {
            let mut row = Vec::with_capacity(c.patrols.len());
            for pi in 0..c.patrols.len() {
                let st = state_at(&race, ci, pi, t);
                let look = match st.kind {
                    Kind::Waiting => Look::Waiting,
                    Kind::AtSite { .. } | Kind::Walking { .. } => Look::Active,
                    Kind::Finished => Look::Finished,
                    Kind::Disqualified => Look::Disqualified,
                };
                row.push((look, st.estimated));
                if !self.shown[ci] {
                    continue;
                }
                match (st.location, st.kind) {
                    (Some(loc), _) => {
                        groups.entry(loc).or_default().push((st.since, ci, number_key(&c.patrols[pi].id), pi))
                    }
                    (None, Kind::Walking { k, frac }) => {
                        let (x, y) = project(st.pos, zoom);
                        let leg = &c.legs[k];
                        let (ax, ay) = project(leg.at((frac - 0.01).max(0.0)), zoom);
                        let (bx, by) = project(leg.at((frac + 0.01).min(1.0)), zoom);
                        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                        let (nx, ny) = if len > 1e-6 { (-(by - ay) / len, (bx - ax) / len) } else { (0.0, 1.0) };
                        walkers.push((ci, pi, (x, y, nx, ny)));
                    }
                    _ => {}
                }
            }
            looks.push(row);
        }
        let mut positions: HashMap<(usize, usize), LatLon> = HashMap::new();
        let mut place = |ci: usize, pi: usize, p: LatLon| {
            positions.insert((ci, pi), p);
            let ll = latlng(p);
            let d = &self.dots[ci][pi];
            d.edge.set_lat_lng(&ll);
            d.dot.set_lat_lng(&ll);
        };
        for (loc, mut members) in groups {
            members.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            let (cx, cy) = project(race.locations[loc].pos, zoom);
            for ((_, ci, _, pi), (ox, oy)) in members.iter().zip(ring_slots(members.len())) {
                place(*ci, *pi, unproject(cx + ox, cy + oy, zoom));
            }
        }
        let mut pts: Vec<(f64, f64, f64, f64)> = walkers.iter().map(|w| w.2).collect();
        nudge(&mut pts);
        for ((ci, pi, _), (x, y, _, _)) in walkers.iter().zip(pts) {
            place(*ci, *pi, unproject(x, y, zoom));
        }

        // Styles, only where they changed.
        for (ci, row) in looks.iter().enumerate() {
            let color = race.categories[ci].color.as_str();
            for (pi, &look) in row.iter().enumerate() {
                let d = &mut self.dots[ci][pi];
                if d.look == Some(look) {
                    continue;
                }
                let (kind, est) = look;
                let (dot, edge) = dot_style(kind, est, color);
                d.dot.set_style(&js(dot));
                d.edge.set_style(&js(json!({ "fillOpacity": edge })));
                d.look = Some(look);
            }
        }

        // The selected patrol's route is drawn stronger.
        let strong_cat = match sel {
            Some(Sel::Patrol { cat, .. }) => Some(cat),
            _ => None,
        };
        for (ci, r) in self.routes.iter_mut().enumerate() {
            let strong = strong_cat == Some(ci);
            if r.strong != Some(strong) {
                r.line.set_style(&js(if strong {
                    json!({ "opacity": 0.9, "weight": 4.5 })
                } else {
                    json!({ "opacity": 0.55, "weight": 3 })
                }));
                if strong {
                    r.casing.bring_to_front();
                    r.line.bring_to_front();
                }
                r.strong = Some(strong);
            }
        }

        // Halo and name tag.
        match sel {
            Some(Sel::Patrol { cat, patrol }) if self.shown.get(cat).copied().unwrap_or(false) => {
                let c = &race.categories[cat];
                let pos = positions.get(&(cat, patrol)).copied().unwrap_or_else(|| state_at(&race, cat, patrol, t).pos);
                let d = &self.dots[cat][patrol];
                self.halo.set_lat_lng(&latlng(pos));
                self.halo.set_style(&js(json!({ "color": c.color, "fillColor": c.color })));
                self.halo.set_tooltip_content(&format!("{} {}", escape(&c.id), escape(&c.patrols[patrol].id)));
                if !self.halo_on {
                    self.halo.add_to(&self.map);
                    self.halo_on = true;
                }
                d.edge.bring_to_front();
                d.dot.bring_to_front();
            }
            _ => {
                if self.halo_on {
                    self.halo.remove();
                    self.halo_on = false;
                }
            }
        }
    }
}

/// Dot style and the opacity of its dark edge (design doc, section 4).
fn dot_style(kind: Look, est: bool, color: &str) -> (serde_json::Value, f64) {
    let (mut s, edge) = match kind {
        Look::Waiting => {
            (json!({ "fillColor": color, "fillOpacity": 0.4, "color": "#ffffff", "opacity": 0.6, "weight": 1.5 }), 0.2)
        }
        Look::Active => {
            (json!({ "fillColor": color, "fillOpacity": 1, "color": "#ffffff", "opacity": 1, "weight": 1.5 }), 0.5)
        }
        Look::Finished => (
            json!({ "fillColor": color, "fillOpacity": 0.45, "color": "#ffffff", "opacity": 0.7, "weight": 1.5 }),
            0.22,
        ),
        Look::Disqualified => {
            (json!({ "fillColor": "#ffffff", "fillOpacity": 1, "color": color, "opacity": 1, "weight": 2.4 }), 0.35)
        }
    };
    s["dashArray"] = serde_json::Value::Null;
    if est {
        if kind == Look::Disqualified {
            s["dashArray"] = json!("3 2");
        } else {
            s["color"] = json!(AMBER);
            s["opacity"] = json!(1);
            s["weight"] = json!(2);
            s["dashArray"] = json!("3 2");
        }
    }
    (s, edge)
}
