//! The map as a Yew component: creates the Leaflet map once, then moves the dots
//! whenever the time, the selection or the visible categories change.

use std::rc::Rc;

use yew::prelude::*;

use crate::app::DataRef;
use crate::map::{create, MapView};
use crate::state::{Action, Sel, Ui};

#[derive(Properties, PartialEq)]
pub struct MapProps {
    pub data: DataRef,
    pub t: f64,
    pub sel: Option<Sel>,
    pub visible: Vec<bool>,
    pub dispatch: UseReducerDispatcher<Ui>,
}

#[function_component(MapPane)]
pub fn map_pane(p: &MapProps) -> Html {
    let node = use_node_ref();
    let holder = use_mut_ref(|| Option::<MapView>::None);

    {
        let node = node.clone();
        let holder = holder.clone();
        let data = p.data.clone();
        let dispatch = p.dispatch.clone();
        use_effect_with(p.data.clone(), move |_| {
            if let Some(el) = node.cast::<web_sys::HtmlElement>() {
                let on_select: Rc<dyn Fn(Option<Sel>)> = Rc::new(move |s| dispatch.dispatch(Action::Select(s)));
                create(&el, data.0.race.clone(), data.0.tiles.as_ref(), on_select, holder);
            }
            || ()
        });
    }
    {
        let holder = holder.clone();
        use_effect_with((p.t, p.sel, p.visible.clone()), move |(t, sel, visible)| {
            if let Ok(mut g) = holder.try_borrow_mut() {
                if let Some(view) = g.as_mut() {
                    view.update(*t, *sel, visible);
                }
            }
            || ()
        });
    }

    html! { <div id="map" ref={node}></div> }
}
