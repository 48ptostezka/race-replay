//! UI state of the replay: the time, playback, selection and hidden categories.

use std::rc::Rc;

use yew::Reducible;

/// What the side panel shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sel {
    Patrol { cat: usize, patrol: usize },
    Location(usize),
}

/// Playback speeds, race minutes per second.
pub const SPEEDS: [f64; 4] = [5.0, 10.0, 20.0, 40.0];

#[derive(Clone, Debug, PartialEq)]
pub struct Ui {
    /// Current time, minutes since midnight (fractional while playing).
    pub t: f64,
    pub t0: f64,
    pub t1: f64,
    pub playing: bool,
    pub speed: f64,
    pub sel: Option<Sel>,
    /// Visible flag per category.
    pub visible: Vec<bool>,
}

impl Ui {
    pub fn new(t0: f64, t1: f64, categories: usize) -> Self {
        Self { t: t0, t0, t1, playing: false, speed: 10.0, sel: None, visible: vec![true; categories] }
    }

    /// The whole minute shown on the clock and used by the panels.
    pub fn minute(&self) -> i64 {
        self.t.floor() as i64
    }
}

pub enum Action {
    /// Advance by `dt` seconds of real time (while playing).
    Tick(f64),
    /// Jump to a time (pauses).
    SetTime(f64),
    /// Step by whole minutes (pauses).
    Step(f64),
    TogglePlay,
    SetSpeed(f64),
    Select(Option<Sel>),
    ToggleCategory(usize),
}

impl Reducible for Ui {
    type Action = Action;

    fn reduce(self: Rc<Self>, action: Action) -> Rc<Self> {
        let mut s = (*self).clone();
        match action {
            Action::Tick(dt) => {
                if !s.playing {
                    return self;
                }
                s.t += s.speed * dt.clamp(0.0, 0.25);
                if s.t >= s.t1 {
                    s.t = s.t1;
                    s.playing = false;
                }
            }
            Action::SetTime(v) => {
                s.playing = false;
                s.t = v.clamp(s.t0, s.t1);
            }
            Action::Step(d) => {
                s.playing = false;
                s.t = (s.t.floor() + d).clamp(s.t0, s.t1);
            }
            Action::TogglePlay => {
                if s.playing {
                    s.playing = false;
                } else {
                    if s.t >= s.t1 {
                        s.t = s.t0;
                    }
                    s.playing = true;
                }
            }
            Action::SetSpeed(v) => s.speed = v,
            Action::Select(sel) => s.sel = sel,
            Action::ToggleCategory(c) => {
                if let Some(v) = s.visible.get_mut(c) {
                    *v = !*v;
                }
                if let Some(Sel::Patrol { cat, .. }) = s.sel {
                    if cat == c {
                        s.sel = None;
                    }
                }
            }
        }
        if s == *self {
            self
        } else {
            Rc::new(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ui: Ui, actions: Vec<Action>) -> Ui {
        let mut s = Rc::new(ui);
        for a in actions {
            s = s.reduce(a);
        }
        (*s).clone()
    }

    #[test]
    fn playback() {
        let ui = Ui::new(585.0, 975.0, 2);
        let s = run(ui.clone(), vec![Action::TogglePlay, Action::Tick(0.1)]);
        assert!(s.playing);
        assert!((s.t - 586.0).abs() < 1e-9); // 10 min/s for 0.1 s
        let s =
            run(ui.clone(), vec![Action::SetTime(970.0), Action::TogglePlay, Action::Tick(0.25), Action::Tick(0.25)]);
        assert_eq!(s.t, 975.0);
        assert!(!s.playing); // stops at the end
        let s = run(s, vec![Action::TogglePlay]);
        assert_eq!(s.t, 585.0); // play again from the start
        let s = run(ui, vec![Action::TogglePlay, Action::Step(1.0)]);
        assert!(!s.playing);
        assert_eq!(s.t, 586.0);
    }

    #[test]
    fn hiding_a_category_closes_its_patrol() {
        let mut ui = Ui::new(0.0, 10.0, 2);
        ui.sel = Some(Sel::Patrol { cat: 1, patrol: 3 });
        let s = run(ui, vec![Action::ToggleCategory(0)]);
        assert_eq!(s.visible, vec![false, true]);
        assert!(s.sel.is_some());
        let s = run(s, vec![Action::ToggleCategory(1)]);
        assert_eq!(s.sel, None);
    }
}
