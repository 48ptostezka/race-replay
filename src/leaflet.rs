//! The Leaflet calls the app uses, bound to the global `L` from `leaflet.js`
//! (loaded by index.html before the app starts).

use wasm_bindgen::prelude::*;

use crate::geo::LatLon;

#[wasm_bindgen]
extern "C" {
    /// `L.Map`
    pub type Map;

    #[wasm_bindgen(js_namespace = L, js_name = map)]
    pub fn new_map(element: &web_sys::HtmlElement, options: &JsValue) -> Map;

    #[wasm_bindgen(method, js_name = fitBounds)]
    pub fn fit_bounds(this: &Map, bounds: &JsValue, options: &JsValue);

    #[wasm_bindgen(method, js_name = getZoom)]
    pub fn get_zoom(this: &Map) -> f64;

    #[wasm_bindgen(method, js_name = on)]
    pub fn map_on(this: &Map, event: &str, handler: &js_sys::Function);

    /// Creates a pane; its element gets the class `leaflet-<name>-pane`.
    #[wasm_bindgen(method, js_name = createPane)]
    pub fn create_pane(this: &Map, name: &str) -> web_sys::HtmlElement;

    /// Any `L.Layer` (tile layer, polyline, circle marker, marker).
    pub type Layer;

    #[wasm_bindgen(method, js_name = addTo)]
    pub fn add_to(this: &Layer, map: &Map) -> Layer;

    #[wasm_bindgen(method)]
    pub fn remove(this: &Layer) -> Layer;

    #[wasm_bindgen(method, js_name = on)]
    pub fn layer_on(this: &Layer, event: &str, handler: &js_sys::Function) -> Layer;

    #[wasm_bindgen(method, js_name = bindTooltip)]
    pub fn bind_tooltip(this: &Layer, content: &str, options: &JsValue) -> Layer;

    #[wasm_bindgen(method, js_name = setLatLng)]
    pub fn set_lat_lng(this: &Layer, latlng: &JsValue) -> Layer;

    #[wasm_bindgen(method, js_name = setStyle)]
    pub fn set_style(this: &Layer, style: &JsValue) -> Layer;

    #[wasm_bindgen(method, js_name = bringToFront)]
    pub fn bring_to_front(this: &Layer) -> Layer;

    /// `L.Marker.setIcon`
    #[wasm_bindgen(method, js_name = setIcon)]
    pub fn set_icon(this: &Layer, icon: &JsValue) -> Layer;

    /// `L.Marker.setZIndexOffset`
    #[wasm_bindgen(method, js_name = setZIndexOffset)]
    pub fn set_z_index_offset(this: &Layer, offset: i32) -> Layer;

    #[wasm_bindgen(js_namespace = L, js_name = tileLayer)]
    pub fn tile_layer(url_template: &str, options: &JsValue) -> Layer;

    #[wasm_bindgen(js_namespace = L, js_name = polyline)]
    pub fn polyline(latlngs: &JsValue, options: &JsValue) -> Layer;

    #[wasm_bindgen(js_namespace = L, js_name = circleMarker)]
    pub fn circle_marker(latlng: &JsValue, options: &JsValue) -> Layer;

    #[wasm_bindgen(js_namespace = L, js_name = marker)]
    pub fn marker(latlng: &JsValue, options: &JsValue) -> Layer;

    #[wasm_bindgen(js_namespace = L, js_name = divIcon)]
    pub fn div_icon(options: &JsValue) -> JsValue;

    /// `L.Control`
    pub type Control;

    #[wasm_bindgen(method, js_name = addTo)]
    pub fn add_to(this: &Control, map: &Map) -> Control;

    #[wasm_bindgen(js_namespace = ["L", "control"], js_name = layers)]
    pub fn control_layers(base_layers: &JsValue, overlays: &JsValue, options: &JsValue) -> Control;

    #[wasm_bindgen(js_namespace = ["L", "control"], js_name = scale)]
    pub fn control_scale(options: &JsValue) -> Control;
}

/// Is Leaflet loaded? (Without it every call above would throw.)
pub fn available() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("L")).map(|v| !v.is_undefined()).unwrap_or(false)
}

/// A plain JS object/array from JSON, for Leaflet options.
pub fn js(value: serde_json::Value) -> JsValue {
    js_sys::JSON::parse(&value.to_string()).expect("serde_json always produces valid JSON")
}

/// `[lat, lon]` without going through JSON (called every frame).
pub fn latlng(p: LatLon) -> JsValue {
    js_sys::Array::of2(&JsValue::from_f64(p.lat), &JsValue::from_f64(p.lon)).into()
}

/// Escape text for Leaflet's HTML tooltips.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
