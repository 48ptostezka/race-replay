//! Fetching the data files that are served next to the app.

use std::rc::Rc;

use crate::input::{parse_manifest, TileManifest};
use crate::model::{build, CategoryInput, Race};

const DATA: &str = "data";

pub struct Data {
    pub race: Rc<Race>,
    pub tiles: Option<TileManifest>,
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let resp = gloo_net::http::Request::get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    resp.text().await.map_err(|e| format!("{url}: {e}"))
}

pub async fn load() -> Result<Data, String> {
    let text = fetch_text(&format!("{DATA}/race.toml")).await.map_err(|e| {
        format!("Nepodařilo se načíst popis závodu ({e}). Zkontrolujte, že je ve složce data soubor race.toml.")
    })?;
    let manifest = parse_manifest(&text)?;
    let mut inputs = Vec::new();
    for c in &manifest.categories {
        let path = |f: &str| format!("{DATA}/{f}");
        inputs.push(CategoryInput {
            manifest: c.clone(),
            times: fetch_text(&path(&c.times)).await,
            hints: match &c.hints {
                Some(h) => Some(fetch_text(&path(h)).await),
                None => None,
            },
            route: fetch_text(&path(&c.route)).await,
            sites: fetch_text(&path(&c.sites)).await,
        });
    }
    let mut race = build(&manifest, inputs)?;
    let tiles = match fetch_text("tiles/tiles.json").await {
        Ok(t) => match serde_json::from_str::<TileManifest>(&t) {
            Ok(m) => Some(m),
            Err(e) => {
                race.notes.push(crate::model::Note {
                    level: crate::model::Level::Warn,
                    cat: None,
                    patrol: None,
                    text: format!("Soubor tiles/tiles.json je poškozený ({e}); mapa se zobrazí bez podkladu."),
                });
                None
            }
        },
        Err(_) => {
            race.notes.push(crate::model::Note {
                level: crate::model::Level::Warn,
                cat: None,
                patrol: None,
                text: "Chybí stažené dlaždice mapy (tiles/tiles.json); mapa se zobrazí bez podkladu. Spusťte fetch_tiles.py.".into(),
            });
            None
        }
    };
    if let Some(t) = &tiles {
        let [[s, w], [n, e]] = t.bounds;
        let outside: Vec<&str> = race
            .categories
            .iter()
            .filter(|c| c.route.iter().any(|p| p.lat < s || p.lat > n || p.lon < w || p.lon > e))
            .map(|c| c.id.as_str())
            .collect();
        if !outside.is_empty() {
            race.notes.push(crate::model::Note {
                level: crate::model::Level::Warn,
                cat: None,
                patrol: None,
                text: format!(
                    "Trasa {} vede mimo stažené dlaždice; spusťte fetch_tiles.py se všemi trasami.",
                    outside.join(", ")
                ),
            });
        }
    }
    Ok(Data { race: Rc::new(race), tiles })
}
