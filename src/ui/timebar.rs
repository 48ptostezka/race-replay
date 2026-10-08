//! The time slider with hour ticks and the categories' time limits.

use yew::prelude::*;

use crate::fmt::hm;
use crate::state::{Action, Ui};

#[derive(Clone, PartialEq, Debug)]
pub struct LimitMark {
    pub t: f64,
    pub ids: Vec<String>,
    pub color: String,
    pub label: String,
}

#[derive(Properties, PartialEq)]
pub struct TimeBarProps {
    pub t0: f64,
    pub t1: f64,
    pub t: f64,
    pub limits: Vec<LimitMark>,
    pub dispatch: UseReducerDispatcher<Ui>,
}

#[function_component(TimeBar)]
pub fn time_bar(p: &TimeBarProps) -> Html {
    let span = (p.t1 - p.t0).max(1.0);
    let pct = |m: f64| ((m - p.t0) / span * 100.0).clamp(0.0, 100.0);
    let oninput = {
        let d = p.dispatch.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            if let Ok(v) = input.value().parse::<f64>() {
                d.dispatch(Action::SetTime(v));
            }
        })
    };
    let mut hours = Vec::new();
    let mut h = (p.t0 / 60.0).ceil() * 60.0;
    while h <= p.t1 {
        hours.push(h);
        h += 60.0;
    }
    html! {
        <div class="timebar">
            <div class="track" style={format!("--done:{}%", pct(p.t))}>
                <input type="range" min={p.t0.to_string()} max={p.t1.to_string()} step="1"
                       value={p.t.floor().to_string()} {oninput} aria-label="Čas závodu"
                       aria-valuetext={hm(p.t.floor())} />
                { for p.limits.iter().map(|l| html! {
                    <div class="limit" style={format!("left:{}%; --c:{}", pct(l.t), l.color)}
                         title={format!("Limit {}: {}", l.ids.join(", "), l.label)}>
                        <span>{ format!("limit {} {}", l.ids.join(" · "), l.label) }</span>
                    </div>
                }) }
            </div>
            <div class="ticks" aria-hidden="true">
                <span class="tick first" style="left:0%">{ hm(p.t0) }</span>
                { for hours.iter().filter(|&&m| pct(m) > 4.0 && pct(m) < 97.0).map(|&m| html! {
                    <span class="tick" style={format!("left:{}%", pct(m))}>{ hm(m) }</span>
                }) }
                <span class="tick last" style="left:100%">{ hm(p.t1) }</span>
            </div>
        </div>
    }
}
