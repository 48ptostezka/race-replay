//! The side panel: a patrol, a location, or the race summary with the data notes.

use yew::prelude::*;

use crate::app::DataRef;
use crate::fmt::{dec, duration, hm, plural};
use crate::input::Hint;
use crate::map::COURSE;
use crate::model::{Level, LocKind, Outcome, Race};
use crate::sim::{counts, occupants, state_at, Kind};
use crate::state::{Action, Sel, Ui};

#[derive(Properties, PartialEq)]
pub struct PanelProps {
    pub data: DataRef,
    pub sel: Option<Sel>,
    pub minute: i64,
    pub visible: Vec<bool>,
    pub dispatch: UseReducerDispatcher<Ui>,
}

#[function_component(Panel)]
pub fn panel(p: &PanelProps) -> Html {
    let race = &p.data.0.race;
    let t = p.minute as f64;
    match p.sel {
        Some(Sel::Patrol { cat, patrol })
            if cat < race.categories.len() && patrol < race.categories[cat].patrols.len() =>
        {
            patrol_panel(race, cat, patrol, t, &p.dispatch)
        }
        Some(Sel::Location(l)) if l < race.locations.len() => location_panel(race, l, t, &p.visible, &p.dispatch),
        _ => summary(race, t, &p.visible),
    }
}

fn close(d: &UseReducerDispatcher<Ui>) -> Html {
    let d = d.clone();
    html! {
        <button class="close" title="Zavřít (Esc)" aria-label="Zavřít"
                onclick={Callback::from(move |_: MouseEvent| d.dispatch(Action::Select(None)))}>
            <svg viewBox="0 0 24 24" width="18" height="18"><path d="M6 6l12 12M18 6L6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
        </button>
    }
}

fn section(title: String, body: Html) -> Html {
    html! {
        <section class="sec">
            <h3>{ title }</h3>
            { body }
        </section>
    }
}

fn row(k: &str, v: String, note: Option<String>) -> Html {
    html! {
        <div class="row">
            <span class="k">{ k }</span>
            <span class="v">{ v }</span>
            { for note.map(|n| html! { <span class="n">{ n }</span> }) }
        </div>
    }
}

fn est_note(est: bool) -> Option<String> {
    est.then(|| "odhad".to_string())
}

fn notes_list(race: &Race, filter: impl Fn(&crate::model::Note) -> bool) -> Html {
    let notes: Vec<_> = race.notes.iter().filter(|n| filter(n)).collect();
    if notes.is_empty() {
        return html! {};
    }
    html! {
        <ul class="notes">
            { for notes.iter().map(|n| html! {
                <li class={match n.level { Level::Error => "error", Level::Warn => "warn", Level::Info => "info" }}>{ &n.text }</li>
            }) }
        </ul>
    }
}

fn patrol_panel(race: &Race, ci: usize, pi: usize, t: f64, d: &UseReducerDispatcher<Ui>) -> Html {
    let c = &race.categories[ci];
    let p = &c.patrols[pi];
    let st = state_at(race, ci, pi, t);
    let (status, pill) = match st.kind {
        Kind::Waiting => (format!("Čeká na start · {}", hm(p.arrivals[0])), "pill wait"),
        Kind::AtSite { k, since } => {
            (format!("Na stanovišti {} · {}", c.sites[k], duration((t - since).floor())), "pill active")
        }
        Kind::Walking { k, .. } => (format!("Na cestě {} → {}", c.sites[k], c.sites[k + 1]), "pill active"),
        Kind::Finished => ("V cíli".to_string(), "pill done"),
        Kind::Disqualified => ("Diskvalifikována".to_string(), "pill dq"),
    };
    let mut totals = format!("Nápovědy {} · Totálky {}", p.hints, p.solutions);
    if p.unknown > 0 {
        totals
            .push_str(&format!(" · {}", plural(p.unknown as usize, "záznam chybí", "záznamy chybí", "záznamů chybí")));
    }

    let body = match st.kind {
        Kind::Finished => {
            let Outcome::Finished { at, race_min, place } = &p.outcome else { unreachable!() };
            section(
                "Výsledek".into(),
                html! { <>
                    { row("Cíl", hm(*at), est_note(p.estimated[p.last])) }
                    { row("Čas na trati", duration(*race_min as f64), None) }
                    { for place.map(|pl| row("Umístění", pl.label(), Some(format!("hodnoceno {}", pl.ranked)))) }
                </> },
            )
        }
        Kind::Disqualified => {
            let why = match &p.outcome {
                Outcome::Late { at } => format!("do cíle v {}, limit {}", hm(*at), c.limit.map(hm).unwrap_or_default()),
                _ => format!("chybí další časy po {}", c.sites[p.last]),
            };
            section(
                "Výsledek".into(),
                html! { <>
                    { row("Stav", "Diskvalifikována".into(), None) }
                    { row("Důvod", why, None) }
                    { row("Poslední čas", format!("{} {}", c.sites[p.last], hm(p.arrivals[p.last])), est_note(p.estimated[p.last])) }
                </> },
            )
        }
        Kind::Waiting => section("Start".into(), row("Start", hm(p.arrivals[0]), est_note(p.estimated[0]))),
        Kind::AtSite { k, .. } | Kind::Walking { k, .. } => {
            let iv = p.intervals.iter().find(|iv| iv.k == k).expect("interval of the current site");
            let leg = &c.legs[k];
            let km = leg.length_m / 1000.0;
            let record = if k == 0 {
                "start – bez nápověd".to_string()
            } else if iv.used != iv.record {
                format!("{} (nepoužit)", iv.record.label())
            } else {
                iv.record.label().to_string()
            };
            let record_note = match iv.record {
                _ if k == 0 => None,
                Hint::Hint if iv.used == Hint::Hint => {
                    Some(format!("nejdřív v {}", hm(iv.arrive + race.settings.hint_after)))
                }
                Hint::Solution if iv.used == Hint::Solution => {
                    Some(format!("nejdřív v {}", hm(iv.arrive + race.settings.solution_after)))
                }
                _ => None,
            };
            let walking = match st.kind {
                Kind::Walking { frac, .. } => Some(frac),
                _ => None,
            };
            html! { <>
                { section(format!("Poslední stanoviště · {}", c.sites[k]), html! { <>
                    { row("Příchod", hm(iv.arrive), est_note(p.estimated[k])) }
                    { row("Záznam", record, record_note) }
                    { row("Odchod", format!("cca {}", hm(iv.leave)), Some("odhad".into())) }
                </> }) }
                { section(format!("Další stanoviště · {}", c.sites[k + 1]), html! { <>
                    { row("Příchod", hm(iv.next), est_note(p.estimated[k + 1])) }
                    { for walking.map(|f| row("Zbývá", format!("{} km · {}", dec(km * (1.0 - f), 2), duration((iv.next - t).max(0.0))), None)) }
                </> }) }
                { section(format!("Přesun {} → {}", c.sites[k], c.sites[k + 1]), html! { <>
                    if km < 0.001 {
                        { row("Úsek", "stejné místo, bez přesunu".into(), None) }
                    } else {
                        { row("Rychlost", iv.speed_kmh.map(|v| format!("cca {} km/h", dec(v, 1))).unwrap_or_else(|| "–".into()),
                              Some(format!("předpoklad {} km/h", dec(race.settings.speed_kmh, 0)))) }
                        { row("Úsek", format!("{} km · chůze {}", dec(km, 2), duration(iv.walk)), None) }
                    }
                    { for walking.map(|f| html! {
                        <div class="legbar" style={format!("--p:{}%; --cat:{}", (f * 100.0).clamp(0.0, 100.0), c.color)}>
                            <div class="fill"></div><div class="knob"></div>
                            <span class="a">{ format!("{} odchod {}", c.sites[k], hm(iv.leave)) }</span>
                            <span class="b">{ format!("{} příchod {}", c.sites[k + 1], hm(iv.next)) }</span>
                        </div>
                    }) }
                </> }) }
            </> }
        }
    };

    // Whole-race strip.
    let a0 = p.arrivals[0];
    let a1 = p.arrivals[p.last].max(a0 + 1.0);
    let x = |m: f64| 4.0 + (m - a0) / (a1 - a0) * 292.0;
    let visited = p.arrivals.iter().filter(|&&a| a <= t).count();
    let on_course =
        if t < a0 { "ještě nestartovala".to_string() } else { format!("na trati {}", duration(t.min(a1) - a0)) };
    let mut hours = Vec::new();
    let mut h = (a0 / 60.0).ceil() * 60.0;
    while h <= a1 {
        hours.push(h);
        h += 60.0;
    }
    let strip = html! {
        <>
        <svg class="strip" viewBox="0 0 300 40" role="img" aria-label="Pobyty a přesuny během závodu">
            <defs>
                <pattern id="hatch" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                    <rect width="5" height="5" fill={c.color.clone()} fill-opacity="0.18" />
                    <line x1="0" y1="0" x2="0" y2="5" stroke={c.color.clone()} stroke-opacity="0.6" stroke-width="2" />
                </pattern>
                <pattern id="hatchw" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                    <rect width="5" height="5" fill="#c98500" fill-opacity="0.18" />
                    <line x1="0" y1="0" x2="0" y2="5" stroke="#c98500" stroke-opacity="0.85" stroke-width="2" />
                </pattern>
            </defs>
            { for p.intervals.iter().map(|iv| html! { <>
                if iv.leave > iv.arrive {
                    <rect x={x(iv.arrive).to_string()} y="4" width={(x(iv.leave) - x(iv.arrive)).to_string()} height="18" fill={c.color.clone()} />
                }
                if iv.next > iv.leave {
                    <rect x={x(iv.leave).to_string()} y="4" width={(x(iv.next) - x(iv.leave)).to_string()} height="18"
                          fill={if iv.speed_note { "url(#hatchw)" } else { "url(#hatch)" }} />
                }
                <line x1={x(iv.arrive).to_string()} x2={x(iv.arrive).to_string()} y1="2" y2="24" stroke="#fbfaf6" stroke-width="1.2" />
            </> }) }
            if t >= a0 && t <= a1 {
                <line x1={x(t).to_string()} x2={x(t).to_string()} y1="0" y2="26" stroke={COURSE} stroke-width="2.2" />
            }
            { for hours.iter().map(|&m| html! {
                <text x={x(m).to_string()} y="37" text-anchor="middle" class="strip-tick">{ format!("{}:00", (m / 60.0).round() as i64) }</text>
            }) }
        </svg>
        <div class="strip-legend">
            <span><i class="sw stay" style={format!("--cat:{}", c.color)}></i>{ "na stanovišti" }</span>
            <span><i class="sw walk" style={format!("--cat:{}", c.color)}></i>{ "přesun" }</span>
        </div>
        <p class="muted">{ format!("Prošla {} z {} stanovišť · {}", visited, c.sites.len(), on_course) }</p>
        </>
    };

    html! {
        <div class="pp" style={format!("--cat:{}", c.color)}>
            <div class="ph"><span class="eyebrow">{ "Hlídka" }</span>{ close(d) }</div>
            <div class="who">
                <span class="badge">{ &c.id }</span>
                <div>
                    <div class="big">{ format!("Hlídka {}", p.id) }</div>
                    <div class="muted">{ &c.name }</div>
                </div>
            </div>
            <div class={pill}>{ status }</div>
            <div class="totals">{ totals }</div>
            { body }
            { section("Průběh závodu".into(), strip) }
            { notes_list(race, |n| n.cat == Some(ci) && n.patrol == Some(pi)) }
        </div>
    }
}

fn location_panel(race: &Race, li: usize, t: f64, visible: &[bool], d: &UseReducerDispatcher<Ui>) -> Html {
    let loc = &race.locations[li];
    let occ = occupants(race, li, t, visible);
    let eyebrow = match loc.kind {
        LocKind::Start => "Start",
        LocKind::Control => "Stanoviště",
        LocKind::Finish => "Cíl",
    };
    let sub = if occ.is_empty() {
        format!("Právě tu nikdo není · {}", hm(t))
    } else {
        format!("Právě zde {} · {}", plural(occ.len(), "hlídka", "hlídky", "hlídek"), hm(t))
    };
    html! {
        <div class="lp">
            <div class="ph"><span class="eyebrow">{ eyebrow }</span>{ close(d) }</div>
            <h2 class="loc-title">
                { for loc.label.iter().enumerate().map(|(i, (c, name))| html! { <>
                    if i > 0 { <span class="sep">{ " · " }</span> }
                    <span style={format!("color:{}", c.map_or(COURSE, |c| race.categories[c].color.as_str()))}>{ name }</span>
                </> }) }
            </h2>
            <p class="muted">{ sub }</p>
            if !occ.is_empty() {
                <div class="occ">
                    <div class="occ-h"><span>{ "Hlídka" }</span><span>{ "Od" }</span><span>{ "Na místě" }</span><span>{ "Záznam" }</span></div>
                    { for occ.iter().map(|o| {
                        let c = &race.categories[o.cat];
                        let p = &c.patrols[o.patrol];
                        let sel = Sel::Patrol { cat: o.cat, patrol: o.patrol };
                        let dd = d.clone();
                        let (from, there, record) = match o.state.kind {
                            Kind::AtSite { k, since } => {
                                let mins = (t - since).floor().max(0.0);
                                let rec = if k == 0 { "start".to_string() } else { p.records[k].label().to_string() };
                                (hm(since), html! {
                                    <span class="mins">
                                        <b>{ format!("{mins} min") }</b>
                                        <span class="mbar" style={format!("--m:{}%", (mins / 60.0 * 100.0).min(100.0))}><i></i></span>
                                    </span>
                                }, rec)
                            }
                            Kind::Waiting => (hm(p.arrivals[0]), html! { <span class="muted">{ "čeká na start" }</span> }, "start".to_string()),
                            Kind::Finished => {
                                let place = match &p.outcome {
                                    Outcome::Finished { place: Some(pl), .. } => pl.label(),
                                    _ => String::new(),
                                };
                                (hm(o.state.since), html! { <span class="muted">{ "v cíli" }</span> }, place)
                            }
                            Kind::Disqualified => (hm(o.state.since), html! { <span class="muted">{ "–" }</span> }, "diskvalifikována".to_string()),
                            Kind::Walking { .. } => (String::new(), html! {}, String::new()),
                        };
                        html! {
                            <button class="occ-r" onclick={Callback::from(move |_: MouseEvent| dd.dispatch(Action::Select(Some(sel))))}
                                    title="Otevřít hlídku">
                                <span class="who-c"><i class="dot" style={format!("background:{}", c.color)}></i>{ format!("{} {}", c.id, p.id) }</span>
                                <span>{ from }</span>
                                { there }
                                <span class={classes!("rec", (record == "neznámo").then_some("unk"))}>{ record }</span>
                            </button>
                        }
                    }) }
                </div>
                <p class="foot">{ "Pruh: minuty na místě, značky u 20 a 40 min (nápověda, totálka)." }</p>
            }
        </div>
    }
}

fn summary(race: &Race, t: f64, visible: &[bool]) -> Html {
    let problems: Vec<_> = race.notes.iter().filter(|n| n.level != Level::Info).collect();
    let infos = race.notes.iter().filter(|n| n.level == Level::Info).count();
    html! {
        <div class="sp">
            <div class="ph"><span class="eyebrow">{ "Přehled" }</span></div>
            <h2 class="race-title">{ &race.title }</h2>
            <p class="muted">{ format!("Stav v {}", hm(t)) }</p>
            <div class="cats">
                { for race.categories.iter().enumerate().map(|(ci, c)| {
                    let (on, fin, dq) = counts(race, ci, t);
                    let shown = visible.get(ci).copied().unwrap_or(true);
                    html! {
                        <div class={classes!("cat", (!shown).then_some("off"))} style={format!("--cat:{}", c.color)}>
                            <div class="cat-top">
                                <b>{ &c.name }</b>
                                <span>{ plural(c.patrols.len(), "hlídka", "hlídky", "hlídek") }</span>
                            </div>
                            <div class="muted">{ format!("na trati {on} · v cíli {fin} · diskv. {dq}") }</div>
                            { for c.limit.map(|l| html! { <div class="muted">{ format!("limit {}", hm(l)) }</div> }) }
                        </div>
                    }
                }) }
            </div>
            <p class="hint">{ "Klikněte na tečku hlídky nebo na stanoviště na mapě. Mezerník spustí a zastaví přehrávání, šipky posunou čas o minutu (se Shiftem o 10 minut)." }</p>
            <section class="sec">
                <h3>{ format!("Poznámky k datům ({})", race.notes.len()) }</h3>
                if race.notes.is_empty() {
                    <p class="muted">{ "Žádné chybějící ani podezřelé hodnoty." }</p>
                }
                if !problems.is_empty() {
                    { notes_list(race, |n| n.level != Level::Info) }
                }
                if infos > 0 {
                    <details>
                        <summary>{ format!("Doplněné a další údaje ({infos})") }</summary>
                        { notes_list(race, |n| n.level == Level::Info) }
                    </details>
                }
            </section>
        </div>
    }
}
