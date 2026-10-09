//! The legend card over the map.

use yew::prelude::*;

use crate::map::COURSE;

const SAMPLE: &str = "--c:#5d6168;--t:#ffffff";

/// A sample patrol dot, drawn exactly like the dots on the map.
fn dot(class: &str) -> Html {
    html! {
        <span class={classes!("pdot", "sample", class.to_string())} aria-hidden="true">
            <span style={SAMPLE}>{ "7" }</span>
        </span>
    }
}

#[function_component(Legend)]
pub fn legend() -> Html {
    html! {
        <div class="legend" role="note" aria-label="Legenda">
            <div class="li">{ dot("") }{ "Na trati" }</div>
            <div class="li">{ dot("finished") }{ "V cíli" }</div>
            <div class="li">{ dot("dq") }{ "Diskvalifikována" }</div>
            <div class="li">{ dot("est") }{ "Odhadnutý čas" }</div>
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
        </div>
    }
}
