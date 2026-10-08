//! Průběh závodu: a replay of a cipher race on a map (Rust, Yew, Leaflet).
//! See the design document for how positions, stays and walks are computed.

mod app;
mod fmt;
mod geo;
mod input;
mod layout;
mod leaflet;
mod load;
mod map;
mod model;
mod sim;
mod state;
mod ui;

fn main() {
    yew::Renderer::<app::App>::new().render();
}
