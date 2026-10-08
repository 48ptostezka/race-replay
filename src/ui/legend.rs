//! The legend card over the map.

use yew::prelude::*;

use crate::map::{AMBER, COURSE};

const SAMPLE: &str = "#5d6168";

fn dot(fill: &str, fill_opacity: f64, stroke: &str, width: f64, dash: Option<&str>) -> Html {
    html! {
        <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
            <circle cx="9" cy="9" r="8" fill="#1d1f22" fill-opacity="0.35" />
            <circle cx="9" cy="9" r="6.2" fill={fill.to_string()} fill-opacity={fill_opacity.to_string()}
                    stroke={stroke.to_string()} stroke-width={width.to_string()}
                    stroke-dasharray={dash.unwrap_or("none").to_string()} />
        </svg>
    }
}

#[function_component(Legend)]
pub fn legend() -> Html {
    html! {
        <div class="legend" role="note" aria-label="Legenda">
            <div class="li">{ dot(SAMPLE, 1.0, "#ffffff", 1.5, None) }{ "Na trati" }</div>
            <div class="li">{ dot(SAMPLE, 0.45, "#ffffff", 1.5, None) }{ "V cíli" }</div>
            <div class="li">{ dot("#ffffff", 1.0, SAMPLE, 2.4, None) }{ "Diskvalifikována" }</div>
            <div class="li">{ dot(SAMPLE, 1.0, AMBER, 2.0, Some("3 2")) }{ "Odhadnutý čas" }</div>
            <div class="li">
                <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
                    <circle cx="9" cy="9" r="6.5" fill="none" stroke={COURSE} stroke-width="2" />
                </svg>
                { "Stanoviště" }
            </div>
            <div class="li">
                <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
                    <circle cx="9" cy="9" r="7" fill="none" stroke={COURSE} stroke-width="1.8" />
                    <circle cx="9" cy="9" r="4" fill="none" stroke={COURSE} stroke-width="1.8" />
                </svg>
                { "Cíl" }
            </div>
            <div class="li note">{ "Barva tečky = kategorie" }</div>
        </div>
    }
}
