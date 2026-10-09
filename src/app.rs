//! The application: loading, the replay screen, playback and keyboard control.

use std::cell::Cell;
use std::rc::Rc;

use gloo_events::{EventListener, EventListenerOptions};
use gloo_timers::callback::Interval;
use wasm_bindgen::JsCast;
use yew::prelude::*;

use crate::fmt::hm;
use crate::leaflet;
use crate::load::{load, Data};
use crate::state::{Action, Sel, Ui};
use crate::ui::legend::Legend;
use crate::ui::mappane::MapPane;
use crate::ui::panel::Panel;
use crate::ui::timebar::{LimitMark, TimeBar};
use crate::ui::topbar::TopBar;

/// Loaded data, compared by identity (it never changes after loading).
#[derive(Clone)]
pub struct DataRef(pub Rc<Data>);

impl PartialEq for DataRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

enum LoadState {
    Loading,
    Failed(String),
    Ready(DataRef),
}

#[function_component(App)]
pub fn app() -> Html {
    let state = use_state(|| LoadState::Loading);
    {
        let state = state.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let result = if leaflet::available() {
                    load().await
                } else {
                    Err("Knihovna Leaflet se nenačetla (chybí leaflet/leaflet.js).".to_string())
                };
                state.set(match result {
                    Ok(d) => LoadState::Ready(DataRef(Rc::new(d))),
                    Err(e) => LoadState::Failed(e),
                });
            });
            || ()
        });
    }
    match &*state {
        LoadState::Loading => html! { <div class="splash"><p>{ "Načítám data závodu…" }</p></div> },
        LoadState::Failed(e) => html! {
            <div class="splash">
                <h1>{ "Průběh závodu" }</h1>
                <p class="error">{ e }</p>
            </div>
        },
        LoadState::Ready(d) => html! { <Replay data={d.clone()} /> },
    }
}

#[derive(Properties, PartialEq)]
pub struct ReplayProps {
    pub data: DataRef,
}

#[function_component(Replay)]
fn replay(props: &ReplayProps) -> Html {
    let race = props.data.0.race.clone();
    let ui = {
        let race = race.clone();
        use_reducer(move || Ui::new(race.t0, race.t1, race.categories.len()))
    };
    let dispatch = ui.dispatcher();

    // Playback: advance t by speed × elapsed real time, about 60 times a second.
    {
        let dispatch = dispatch.clone();
        use_effect_with(ui.playing, move |playing| {
            let interval = playing.then(|| {
                let last = Rc::new(Cell::new(js_sys::Date::now()));
                Interval::new(16, move || {
                    let now = js_sys::Date::now();
                    let dt = (now - last.get()) / 1000.0;
                    last.set(now);
                    dispatch.dispatch(Action::Tick(dt));
                })
            });
            move || drop(interval)
        });
    }

    // Keyboard: Space plays/pauses, arrows step one minute (Shift: ten), Esc closes the panel.
    {
        let dispatch = dispatch.clone();
        use_effect_with((), move |_| {
            let document = web_sys::window().and_then(|w| w.document());
            let listener = document.map(|doc| {
                EventListener::new_with_options(
                    &doc,
                    "keydown",
                    EventListenerOptions::enable_prevent_default(),
                    move |e| {
                        let Some(e) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
                        if e.ctrl_key() || e.meta_key() || e.alt_key() {
                            return;
                        }
                        let typing = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|el| {
                            let tag = el.tag_name().to_uppercase();
                            let range = el.get_attribute("type").as_deref() == Some("range");
                            (tag == "INPUT" && !range) || tag == "TEXTAREA" || tag == "SELECT"
                        });
                        if typing {
                            return;
                        }
                        let step = if e.shift_key() { 10.0 } else { 1.0 };
                        match e.key().as_str() {
                            " " => {
                                e.prevent_default();
                                dispatch.dispatch(Action::TogglePlay);
                            }
                            "ArrowLeft" => {
                                e.prevent_default();
                                dispatch.dispatch(Action::Step(-step));
                            }
                            "ArrowRight" => {
                                e.prevent_default();
                                dispatch.dispatch(Action::Step(step));
                            }
                            "Escape" => dispatch.dispatch(Action::Select(None)),
                            _ => {}
                        }
                    },
                )
            });
            move || drop(listener)
        });
    }

    let minute = ui.minute();
    let limits = limit_marks(&race);
    let tiles = props.data.0.tiles.as_ref();
    let logo = match tiles {
        Some(t) => match &t.logo {
            Some(file) => html! {
                <a class="mapy-logo" href={t.logo_link.clone()} target="_blank" rel="noopener">
                    <img src={format!("tiles/{file}")} alt="Mapy.com" width="100" />
                </a>
            },
            None => {
                html! { <a class="mapy-logo text" href={t.logo_link.clone()} target="_blank" rel="noopener">{ "Mapy.com" }</a> }
            }
        },
        None => html! {},
    };
    let sel: Option<Sel> = ui.sel;

    html! {
        <div class="app">
            <TopBar data={props.data.clone()} minute={minute} playing={ui.playing} speed={ui.speed}
                    visible={ui.visible.clone()} info_open={ui.sel == Some(Sel::Info)} dispatch={dispatch.clone()} />
            <TimeBar t0={ui.t0} t1={ui.t1} t={ui.t} limits={limits} dispatch={dispatch.clone()} />
            <div class="body">
                <div class="mapwrap">
                    <MapPane data={props.data.clone()} t={ui.t} sel={sel} visible={ui.visible.clone()} dispatch={dispatch.clone()} />
                    <Legend />
                    { logo }
                </div>
                <aside class="panel" aria-live="polite">
                    <Panel data={props.data.clone()} sel={sel} minute={minute} visible={ui.visible.clone()} dispatch={dispatch} />
                </aside>
            </div>
        </div>
    }
}

/// One mark per distinct limit time, labelled with the categories that share it.
fn limit_marks(race: &crate::model::Race) -> Vec<LimitMark> {
    let mut marks: Vec<LimitMark> = Vec::new();
    for c in &race.categories {
        let Some(l) = c.limit else { continue };
        match marks.iter_mut().find(|m| (m.t - l).abs() < 1e-9) {
            Some(m) => {
                m.ids.push(c.id.clone());
                m.color = crate::map::INK.to_string();
            }
            None => marks.push(LimitMark { t: l, ids: vec![c.id.clone()], color: c.color.clone(), label: hm(l) }),
        }
    }
    marks
}

/// Server-side rendering of the screen and the panels, so their content can be
/// checked natively without a browser (effects such as the map do not run here).
#[cfg(test)]
mod ssr_tests {
    use super::*;
    use crate::model::tests::race_m;
    use crate::state::Sel;

    #[derive(Properties, PartialEq)]
    struct PanelAt {
        data: DataRef,
        sel: Option<Sel>,
        minute: i64,
    }

    #[function_component(PanelHost)]
    fn panel_host(p: &PanelAt) -> Html {
        let ui = use_reducer(|| Ui::new(0.0, 1.0, 1));
        html! { <Panel data={p.data.clone()} sel={p.sel} minute={p.minute} visible={vec![true]} dispatch={ui.dispatcher()} /> }
    }

    fn data() -> DataRef {
        DataRef(Rc::new(Data { race: Rc::new(race_m()), tiles: None }))
    }

    async fn panel(sel: Option<Sel>, hhmm: &str) -> String {
        let t = crate::input::parse_time(hhmm).unwrap().unwrap() as i64;
        let props = PanelAt { data: data(), sel, minute: t };
        yew::LocalServerRenderer::<PanelHost>::with_props(props).hydratable(false).render().await
    }

    fn text(html: &str) -> String {
        // strip tags, collapse spaces, for readable assertions
        let mut out = String::new();
        let mut tag = false;
        for c in html.chars() {
            match c {
                '<' => tag = true,
                '>' => {
                    tag = false;
                    out.push(' ');
                }
                c if !tag => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn idx(d: &DataRef, id: &str) -> usize {
        d.0.race.categories[0].patrols.iter().position(|p| p.id == id).unwrap()
    }

    #[tokio::test]
    async fn replay_screen() {
        let html = yew::LocalServerRenderer::<Replay>::with_props(ReplayProps { data: data() })
            .hydratable(false)
            .render()
            .await;
        let tx = text(&html);
        std::fs::write(std::env::temp_dir().join("ssr_replay.html"), &html).ok();
        assert!(tx.contains("Průběh závodu"));
        assert!(tx.contains("Šifrovačka 2026 · 1 kategorie · 21 hlídek"), "{tx}");
        assert!(tx.contains("09:45"));
        assert!(tx.contains("Kategorie M 21 na trati 21 · v cíli 0"), "{tx}");
        assert!(tx.contains("limit M 16:15"));
        assert!(tx.contains("Přehled"));
        assert!(tx.contains("Jak to funguje?"));
        assert!(tx.contains("Rychlost 1 2 5 10 20 min/s"), "{tx}");
        assert!(tx.contains("Poznámky k datům"));
        assert!(html.contains("id=\"map\""));
    }

    #[tokio::test]
    async fn patrol_walking() {
        let d = data();
        let html = panel(Some(Sel::Patrol { cat: 0, patrol: idx(&d, "14") }), "12:00").await;
        std::fs::write(std::env::temp_dir().join("ssr_p14.html"), &html).ok();
        let tx = text(&html);
        assert!(tx.contains("Hlídka 14"), "{tx}");
        // hints so far / at the end: both hints (M2, M3) were taken by 12:00
        assert!(tx.contains("Teď Na konci Nápovědy 2 2 Totálky 0 0 Pořadí"), "{tx}");
        assert!(tx.contains("Na cestě M3 → M4") || tx.contains("Na stanovišti M3"), "{tx}");
        assert!(tx.contains("Poslední stanoviště · M3"), "{tx}");
        assert!(tx.contains("Příchod 11:37"), "{tx}");
        assert!(tx.contains("Další stanoviště · M4"), "{tx}");
        assert!(tx.contains("Příchod 12:46"), "{tx}");
        assert!(tx.contains("Prošla 4 z 9 stanovišť · na trati 2 h 15 min"), "{tx}");
    }

    #[tokio::test]
    async fn patrol_finished_and_disqualified() {
        let d = data();
        let tx = text(&panel(Some(Sel::Patrol { cat: 0, patrol: idx(&d, "16") }), "13:00").await);
        assert!(tx.contains("V cíli"), "{tx}");
        assert!(tx.contains("Cíl 12:35"), "{tx}");
        assert!(tx.contains("Čas na trati 2 h 50 min"), "{tx}");
        assert!(tx.contains("Umístění 1. místo hodnoceno 19"), "{tx}");
        assert!(tx.contains("Pořadí 1. 1."), "{tx}");
        let tx = text(&panel(Some(Sel::Patrol { cat: 0, patrol: idx(&d, "11") }), "16:15").await);
        assert!(tx.contains("Diskvalifikována"), "{tx}");
        assert!(tx.contains("do cíle v 16:15, limit 16:15"), "{tx}");
        assert!(tx.contains("Nápovědy 6 6 Totálky 2 2 Pořadí diskv. diskv."), "{tx}");
        let tx = text(&panel(Some(Sel::Patrol { cat: 0, patrol: idx(&d, "12") }), "12:00").await);
        assert!(tx.contains("U 1 stanoviště chybí záznam o nápovědě."), "{tx}");
    }

    #[tokio::test]
    async fn info_panel() {
        let tx = text(&panel(Some(Sel::Info), "12:00").await);
        assert!(tx.contains("Jak aplikace odhaduje polohu hlídek"), "{tx}");
        assert!(tx.contains("Polohy hlídek na mapě nejsou úplně skutečné."), "{tx}");
        assert!(tx.contains("s totálkou právě 40 minut"), "{tx}");
        assert!(!tx.contains("Mapu berte"), "{tx}");
    }

    #[tokio::test]
    async fn location_with_patrols() {
        let d = data();
        let loc = d.0.race.categories[0].site_loc[5];
        let html = panel(Some(Sel::Location(loc)), "13:24").await;
        std::fs::write(std::env::temp_dir().join("ssr_loc.html"), &html).ok();
        let tx = text(&html);
        assert!(tx.contains("Stanoviště M5"), "{tx}");
        // patrol 9 arrived 13:02 and has a hint at M5; it must be listed first
        let first = tx.find("M 9").expect("M 9 listed");
        assert!(tx[first..].starts_with("M 9 13:02 22 min nápověda"), "{}", &tx[first..]);
    }

    #[tokio::test]
    async fn start_location_before_the_start() {
        let d = data();
        let loc = d.0.race.categories[0].site_loc[0];
        let tx = text(&panel(Some(Sel::Location(loc)), "09:30").await);
        assert!(tx.contains("Start"), "{tx}");
        assert!(tx.contains("Právě zde 21 hlídek · 09:30"), "{tx}");
        assert!(tx.contains("čeká na start"), "{tx}");
    }
}
