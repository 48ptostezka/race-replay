//! The Leaflet map: cached Mapy.com tiles as they are, and on top the routes, the
//! locations and one numbered dot per patrol, moved every frame (design doc, section 6).
//!
//! Layers, bottom to top: tiles, routes (own pane), control circles, location labels
//! (own pane), start/finish symbols and patrol dots (marker pane). So patrol dots are
//! always above the routes and the labels, whatever is selected.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::geo::{project, unproject};
use crate::input::TileManifest;
use crate::layout::{nudge, ring_slots, DOT_PX};
use crate::leaflet::{self as l, escape, js, latlng, Layer, Map};
use crate::model::{LocKind, Race};
use crate::sim::{number_key, state_at, Kind};
use crate::state::Sel;

pub const COURSE: &str = "#a8126f";
pub const INK: &str = "#1d1f22";
const TILES: &str = "tiles";
const OVERZOOM: u8 = 2;
/// Leaflet panes created for the app (styled in style.css by their class names
/// `leaflet-routes-pane` and `leaflet-labels-pane`).
const ROUTES_PANE: &str = "routes";
const LABELS_PANE: &str = "labels";
/// Marker z-index offsets: location symbols below dots, the selected dot on top.
const Z_LOCATION: i32 = -10_000;
const Z_SELECTED: i32 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Waiting,
    Active,
    Finished,
    Disqualified,
}

/// How a dot is drawn: its state, an estimated time, selected.
type Style = (Look, bool, bool);

struct Dot {
    marker: Layer,
    style: Option<Style>,
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
    map.create_pane(ROUTES_PANE);
    map.create_pane(LABELS_PANE);

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

    // Routes, in their own pane below everything else drawn by the app.
    let mut routes = Vec::new();
    let (mut bs, mut bw, mut bn, mut be) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in &race.categories {
        let pts: Vec<[f64; 2]> = c.route.iter().map(|p| [p.lat, p.lon]).collect();
        for p in &c.route {
            (bs, bn, bw, be) = (bs.min(p.lat), bn.max(p.lat), bw.min(p.lon), be.max(p.lon));
        }
        let casing = l::polyline(
            &js(json!(pts)),
            &js(json!({ "pane": ROUTES_PANE, "color": "#ffffff", "opacity": 0.7, "weight": 6,
                        "interactive": false, "lineJoin": "round" })),
        );
        let line = l::polyline(
            &js(json!(pts)),
            &js(json!({ "pane": ROUTES_PANE, "color": c.color, "opacity": 0.55, "weight": 3,
                        "interactive": false, "lineJoin": "round" })),
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
                &js(json!({ "radius": 11, "color": COURSE, "weight": 2.4, "fillColor": "#ffffff",
                            "fillOpacity": 0.35, "bubblingMouseEvents": false })),
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
                l::marker(&latlng(loc.pos), &marker_options(&icon, Z_LOCATION))
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
        layer.bind_tooltip(
            &label,
            &js(json!({ "permanent": true, "direction": "right", "offset": [22, 0], "className": "loc-label",
                        "interactive": false, "pane": LABELS_PANE })),
        );
        let cb = on_select.clone();
        let click = Closure::<dyn FnMut(JsValue)>::new(move |_| cb(Some(Sel::Location(li))));
        layer.layer_on("click", click.as_ref().unchecked_ref());
        closures.push(click);
        layer.add_to(&map);
    }

    // One numbered dot per patrol.
    let mut dots = Vec::new();
    for (ci, c) in race.categories.iter().enumerate() {
        let mut row = Vec::new();
        for (pi, p) in c.patrols.iter().enumerate() {
            let start = race.locations[c.site_loc[0]].pos;
            let icon = dot_icon(&p.id, &c.color, (Look::Waiting, false, false));
            let marker = l::marker(&latlng(start), &marker_options(&icon, 0));
            let cb = on_select.clone();
            let click = Closure::<dyn FnMut(JsValue)>::new(move |_| cb(Some(Sel::Patrol { cat: ci, patrol: pi })));
            marker.layer_on("click", click.as_ref().unchecked_ref());
            closures.push(click);
            marker.add_to(&map);
            row.push(Dot { marker, style: Some((Look::Waiting, false, false)) });
        }
        dots.push(row);
    }

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
    *holder.borrow_mut() =
        Some(MapView { race, map, dots, routes, shown: vec![true; n], last: None, _closures: closures });
}

/// Marker options with a ready-made icon object (built directly: a JSON round trip
/// would turn the icon into a plain object).
fn marker_options(icon: &JsValue, z_offset: i32) -> JsValue {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"icon".into(), icon);
    let _ = js_sys::Reflect::set(&opts, &"keyboard".into(), &JsValue::FALSE);
    let _ = js_sys::Reflect::set(&opts, &"zIndexOffset".into(), &JsValue::from(z_offset));
    opts.into()
}

/// The icon of a patrol dot: the patrol number on the category colour.
fn dot_icon(id: &str, color: &str, (look, est, selected): Style) -> JsValue {
    let mut class = String::from("pdot");
    class.push_str(match look {
        Look::Waiting => " waiting",
        Look::Active => "",
        Look::Finished => " finished",
        Look::Disqualified => " dq",
    });
    if est {
        class.push_str(" est");
    }
    if selected {
        class.push_str(" sel");
    }
    if id.chars().count() > 2 {
        class.push_str(" long");
    }
    let html = format!("<span style=\"--c:{};--t:{}\">{}</span>", escape(color), text_on(color), escape(id));
    let size = DOT_PX;
    l::div_icon(&js(json!({
        "html": html, "className": class,
        "iconSize": [size, size], "iconAnchor": [size / 2.0, size / 2.0],
    })))
}

/// White or dark text, whichever reads better on `color` (`#rrggbb`): white unless its
/// contrast with the colour is under 3:1 (as on yellow).
pub fn text_on(color: &str) -> &'static str {
    let hex = color.trim().trim_start_matches('#');
    let channel = |i: usize| {
        let v = hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0) as f64 / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    if hex.len() != 6 {
        return "#ffffff";
    }
    let lum = 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
    if 1.05 / (lum + 0.05) >= 3.0 {
        "#ffffff"
    } else {
        INK
    }
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
        let selected = match sel {
            Some(Sel::Patrol { cat, patrol }) => Some((cat, patrol)),
            _ => None,
        };

        // Show or hide whole categories.
        for (ci, shown) in self.shown.iter_mut().enumerate() {
            let want = visible.get(ci).copied().unwrap_or(true);
            if want != *shown {
                let r = &self.routes[ci];
                for d in self.dots[ci].iter_mut() {
                    if want {
                        d.marker.add_to(&self.map);
                    } else {
                        d.marker.remove();
                    }
                    d.style = None; // restyle after showing again
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
        let mut styles: Vec<Vec<Style>> = Vec::new();
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
                row.push((look, st.estimated, selected == Some((ci, pi))));
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
            styles.push(row);
        }
        for (loc, mut members) in groups {
            members.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            let (cx, cy) = project(race.locations[loc].pos, zoom);
            for ((_, ci, _, pi), (ox, oy)) in members.iter().zip(ring_slots(members.len())) {
                self.dots[*ci][*pi].marker.set_lat_lng(&latlng(unproject(cx + ox, cy + oy, zoom)));
            }
        }
        let mut pts: Vec<(f64, f64, f64, f64)> = walkers.iter().map(|w| w.2).collect();
        nudge(&mut pts);
        for ((ci, pi, _), (x, y, _, _)) in walkers.iter().zip(pts) {
            self.dots[*ci][*pi].marker.set_lat_lng(&latlng(unproject(x, y, zoom)));
        }

        // Icons, only where the style changed; the selected dot is drawn on top.
        for (ci, row) in styles.iter().enumerate() {
            if !self.shown[ci] {
                continue;
            }
            let c = &race.categories[ci];
            for (pi, &style) in row.iter().enumerate() {
                let d = &mut self.dots[ci][pi];
                if d.style == Some(style) {
                    continue;
                }
                d.marker.set_icon(&dot_icon(&c.patrols[pi].id, &c.color, style));
                d.marker.set_z_index_offset(if style.2 { Z_SELECTED } else { 0 });
                d.style = Some(style);
            }
        }

        // The selected patrol's route is drawn stronger (it stays in the routes pane).
        let strong_cat = selected.map(|(c, _)| c);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_colour_on_category_colours() {
        assert_eq!(text_on("#2f9e44"), "#ffffff"); // green
        assert_eq!(text_on("#d6392f"), "#ffffff"); // red
        assert_eq!(text_on("#e0b000"), INK); // yellow
        assert_eq!(text_on("#ffffff"), INK);
        assert_eq!(text_on("red"), "#ffffff"); // not #rrggbb: default
    }
}
