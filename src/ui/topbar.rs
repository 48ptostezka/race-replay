//! Title, clock, playback controls, speed and one chip per category.

use yew::prelude::*;

use crate::app::DataRef;
use crate::fmt::{hm, plural};
use crate::sim::counts;
use crate::state::{Action, Sel, Ui, SPEEDS};

#[derive(Properties, PartialEq)]
pub struct TopBarProps {
    pub data: DataRef,
    pub minute: i64,
    pub playing: bool,
    pub speed: f64,
    pub visible: Vec<bool>,
    /// The "Jak to funguje?" panel is open.
    pub info_open: bool,
    pub dispatch: UseReducerDispatcher<Ui>,
}

#[function_component(TopBar)]
pub fn top_bar(p: &TopBarProps) -> Html {
    let race = &p.data.0.race;
    let t = p.minute as f64;
    let patrols: usize = race.categories.iter().map(|c| c.patrols.len()).sum();
    let subtitle = format!(
        "{} · {}",
        plural(race.categories.len(), "kategorie", "kategorie", "kategorií"),
        plural(patrols, "hlídka", "hlídky", "hlídek")
    );
    let d = p.dispatch.clone();
    let step = |delta: f64| {
        let d = d.clone();
        Callback::from(move |_: MouseEvent| d.dispatch(Action::Step(delta)))
    };
    let toggle = {
        let d = d.clone();
        Callback::from(move |_: MouseEvent| d.dispatch(Action::TogglePlay))
    };
    let info = {
        let d = d.clone();
        let open = p.info_open;
        Callback::from(move |_: MouseEvent| d.dispatch(Action::Select(if open { None } else { Some(Sel::Info) })))
    };

    html! {
        <header class="topbar">
            <div class="title">
                <h1>
                    { "Průběh závodu" }
                    <button class={classes!("info-btn", p.info_open.then_some("on"))} onclick={info}
                            aria-pressed={p.info_open.to_string()} title="Jak aplikace odhaduje polohu hlídek">
                        { "Jak to funguje?" }
                    </button>
                </h1>
                <p>{ format!("{} · {}", race.title, subtitle) }</p>
            </div>
            <div class="clock" aria-label="Čas závodu">{ hm(t) }</div>
            <div class="transport">
                <button class="btn" onclick={step(-1.0)} title="O minutu zpět (←)" aria-label="O minutu zpět">
                    <svg viewBox="0 0 24 24" width="18" height="18"><path d="M15 6l-6 6 6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
                <button class="btn play" onclick={toggle} title="Přehrát / pozastavit (mezerník)"
                        aria-label={if p.playing { "Pozastavit" } else { "Přehrát" }}>
                    if p.playing {
                        <svg viewBox="0 0 24 24" width="20" height="20"><rect x="6.5" y="5" width="4" height="14" rx="1" fill="currentColor"/><rect x="13.5" y="5" width="4" height="14" rx="1" fill="currentColor"/></svg>
                    } else {
                        <svg viewBox="0 0 24 24" width="20" height="20"><path d="M8 5.5v13l11-6.5z" fill="currentColor"/></svg>
                    }
                </button>
                <button class="btn" onclick={step(1.0)} title="O minutu vpřed (→)" aria-label="O minutu vpřed">
                    <svg viewBox="0 0 24 24" width="18" height="18"><path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
            </div>
            <div class="speed" role="group" aria-label="Rychlost přehrávání">
                <span class="eyebrow">{ "Rychlost" }</span>
                <div class="seg">
                    { for SPEEDS.iter().map(|&v| {
                        let d = d.clone();
                        html! {
                            <button class={classes!("segbtn", (v == p.speed).then_some("on"))}
                                    aria-pressed={(v == p.speed).to_string()}
                                    onclick={Callback::from(move |_: MouseEvent| d.dispatch(Action::SetSpeed(v)))}>
                                { v }
                            </button>
                        }
                    }) }
                </div>
                <span class="unit">{ "min/s" }</span>
            </div>
            <div class="chips">
                { for race.categories.iter().enumerate().map(|(ci, c)| {
                    let (on, fin, dq) = counts(race, ci, t);
                    let shown = p.visible.get(ci).copied().unwrap_or(true);
                    let d = d.clone();
                    let mut line = format!("na trati {on} · v cíli {fin}");
                    if dq > 0 {
                        line.push_str(&format!(" · diskv. {dq}"));
                    }
                    html! {
                        <button class={classes!("chip", (!shown).then_some("off"))} style={format!("--cat:{}", c.color)}
                                aria-pressed={shown.to_string()}
                                title={if shown { "Skrýt kategorii" } else { "Zobrazit kategorii" }}
                                onclick={Callback::from(move |_: MouseEvent| d.dispatch(Action::ToggleCategory(ci)))}>
                            <span class="chip-top"><span class="chip-name">{ &c.name }</span><span class="chip-n">{ c.patrols.len() }</span></span>
                            <span class="chip-line">{ if shown { line } else { "skryto".to_string() } }</span>
                        </button>
                    }
                }) }
            </div>
        </header>
    }
}
