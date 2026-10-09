//! Reading the input files: the race manifest (TOML), the CSV files and the GPX route.
//! Everything here is plain Rust and runs the same natively (tests) and in the browser.

use serde::Deserialize;

use crate::geo::LatLon;

fn default_speed() -> f64 {
    5.0
}
fn default_hint() -> f64 {
    20.0
}
fn default_solution() -> f64 {
    40.0
}
fn default_merge() -> f64 {
    15.0
}

/// `data/race.toml`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub title: String,
    #[serde(default = "default_speed")]
    pub speed_kmh: f64,
    #[serde(default = "default_hint")]
    pub hint_after_min: f64,
    #[serde(default = "default_solution")]
    pub solution_after_min: f64,
    #[serde(default = "default_merge")]
    pub merge_radius_m: f64,
    #[serde(rename = "category", default)]
    pub categories: Vec<CategoryManifest>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct CategoryManifest {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub limit: Option<String>,
    /// Assumed walking speed of this category, km/h; the race-wide `speed_kmh` if missing.
    #[serde(default)]
    pub speed_kmh: Option<f64>,
    pub times: String,
    #[serde(default)]
    pub hints: Option<String>,
    pub route: String,
    pub sites: String,
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let m: Manifest = toml::from_str(text).map_err(|e| format!("Soubor race.toml nelze přečíst: {e}"))?;
    if m.categories.is_empty() {
        return Err("Soubor race.toml neobsahuje žádnou kategorii ([[category]]).".into());
    }
    if m.speed_kmh <= 0.0 {
        return Err("V race.toml musí být speed_kmh větší než 0.".into());
    }
    Ok(m)
}

/// `tiles/tiles.json`, written by fetch_tiles.py.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct TileManifest {
    pub bounds: [[f64; 2]; 2],
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub mapsets: std::collections::BTreeMap<String, MapsetInfo>,
    pub default_mapset: String,
    pub attribution_html: String,
    #[serde(default)]
    pub logo: Option<String>,
    pub logo_link: String,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct MapsetInfo {
    pub ext: String,
}

/// Lower-case, Czech diacritics removed, whitespace collapsed: for matching names.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars().flat_map(|c| c.to_lowercase()) {
        let c = match c {
            'á' | 'ä' => 'a',
            'č' => 'c',
            'ď' => 'd',
            'é' | 'ě' | 'ë' => 'e',
            'í' => 'i',
            'ľ' | 'ĺ' => 'l',
            'ň' => 'n',
            'ó' | 'ö' | 'ô' => 'o',
            'ř' | 'ŕ' => 'r',
            'š' => 's',
            'ť' => 't',
            'ú' | 'ů' | 'ü' => 'u',
            'ý' => 'y',
            'ž' => 'z',
            c => c,
        };
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A CSV table: header cells and rows (cells trimmed). Detects `,` `;` or tab from the
/// header line, strips a BOM, skips empty lines and lines starting with `#`.
pub struct Table {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn parse_csv(text: &str) -> Result<Table, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let head = lines.next().ok_or("Soubor je prázdný.")?;
    let delim = [',', ';', '\t'].into_iter().max_by_key(|d| head.matches(*d).count()).unwrap_or(',');
    let header = split_line(head, delim);
    let rows = lines.map(|l| split_line(l, delim)).collect();
    Ok(Table { header, rows })
}

fn split_line(line: &str, d: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == d {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// "09:45", "9.45" or "09:45:30" -> minutes since midnight. Empty -> Ok(None), unreadable -> Err.
pub fn parse_time(s: &str) -> Result<Option<f64>, ()> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = s.split([':', '.']).collect();
    if !(2..=3).contains(&parts.len()) || parts[1].len() != 2 || parts.get(2).is_some_and(|p| p.len() != 2) {
        return Err(());
    }
    let num = |p: &str| p.parse::<u32>().map_err(|_| ());
    let (h, m) = (num(parts[0])?, num(parts[1])?);
    let sec = match parts.get(2) {
        Some(p) => num(p)?,
        None => 0,
    };
    if parts[0].is_empty() || parts[0].len() > 2 || h > 23 || m > 59 || sec > 59 {
        return Err(());
    }
    Ok(Some(h as f64 * 60.0 + m as f64 + sec as f64 / 60.0))
}

/// The hint record of one site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Hint {
    /// "bez nápovědy": stayed under 20 min
    None,
    /// "nápověda": 20–40 min
    Hint,
    /// "totálka": 40 min or more (the hint came first)
    Solution,
    /// empty or "?": anything
    Unknown,
}

impl Hint {
    /// Czech label shown in the app.
    pub fn label(self) -> &'static str {
        match self {
            Hint::None => "bez nápovědy",
            Hint::Hint => "nápověda",
            Hint::Solution => "totálka",
            Hint::Unknown => "neznámo",
        }
    }
}

/// A cell of the hints file. `None` = a value that is not recognised.
pub fn parse_hint(s: &str) -> Option<Hint> {
    match norm(s).as_str() {
        "" | "?" | "nevime" | "nezname" | "neznamo" | "unknown" | "n/a" => Some(Hint::Unknown),
        "bez napovedy" | "bez napovedi" | "bez" | "none" | "no hint" | "ne" | "0" | "-" => Some(Hint::None),
        "napoveda" | "napovedu" | "n" | "hint" | "h" | "1" => Some(Hint::Hint),
        "totalka" | "t" | "reseni" | "r" | "solution" | "s" | "2" => Some(Hint::Solution),
        _ => None,
    }
}

/// One row of a sites file.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteRow {
    pub name: String,
    pub pos: LatLon,
    /// Optional explicit name of a shared location (column `misto`).
    pub misto: Option<String>,
}

/// `stanoviste,lat,lon[,misto]`
pub fn parse_sites(text: &str) -> Result<Vec<SiteRow>, String> {
    let t = parse_csv(text)?;
    let col = |names: &[&str]| t.header.iter().position(|h| names.contains(&norm(h).as_str()));
    let (Some(cn), Some(ca), Some(co)) = (
        col(&["stanoviste", "site", "nazev"]),
        col(&["lat", "sirka", "latitude"]),
        col(&["lon", "lng", "delka", "longitude"]),
    ) else {
        return Err("Soubor se stanovišti musí mít sloupce stanoviste, lat, lon.".into());
    };
    let cm = col(&["misto", "location"]);
    let mut out = Vec::new();
    for (i, r) in t.rows.iter().enumerate() {
        let cell = |c: usize| r.get(c).map(String::as_str).unwrap_or("");
        let num = |c: usize| cell(c).replace(',', ".").parse::<f64>();
        match (num(ca), num(co)) {
            (Ok(lat), Ok(lon)) if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) => {
                out.push(SiteRow {
                    name: cell(cn).to_string(),
                    pos: LatLon::new(lat, lon),
                    misto: cm.map(|c| cell(c).to_string()).filter(|s| !s.is_empty()),
                });
            }
            _ => return Err(format!("Řádek {} souboru se stanovišti nemá platné souřadnice.", i + 2)),
        }
    }
    Ok(out)
}

/// Points of all `<trkpt>` and `<rtept>` elements in file order (waypoints ignored).
pub fn parse_gpx(text: &str) -> Result<Vec<LatLon>, String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("GPX není platné XML: {e}"))?;
    let mut pts = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        if tag != "trkpt" && tag != "rtept" {
            continue;
        }
        let get = |a: &str| node.attribute(a).and_then(|v| v.parse::<f64>().ok());
        match (get("lat"), get("lon")) {
            (Some(lat), Some(lon)) => pts.push(LatLon::new(lat, lon)),
            _ => return Err("GPX obsahuje bod bez platných souřadnic.".into()),
        }
    }
    if pts.len() < 2 {
        return Err("GPX neobsahuje trasu s alespoň dvěma body.".into());
    }
    Ok(pts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse_time("09:45"), Ok(Some(585.0)));
        assert_eq!(parse_time("9.45"), Ok(Some(585.0)));
        assert_eq!(parse_time(" 16:15 "), Ok(Some(975.0)));
        assert_eq!(parse_time("10:00:30"), Ok(Some(600.5)));
        assert_eq!(parse_time(""), Ok(None));
        for bad in ["abc", "25:00", "10:7", "10:60", "1000", "10:00:5"] {
            assert_eq!(parse_time(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn csv_variants() {
        let t = parse_csv("\u{feff}a;b;\"c;d\"\r\n\r\n# comment\n1;2;\"x \"\"y\"\"\"\n").unwrap();
        assert_eq!(t.header, vec!["a", "b", "c;d"]);
        assert_eq!(t.rows, vec![vec!["1", "2", "x \"y\""]]);
    }

    #[test]
    fn hints() {
        assert_eq!(parse_hint("bez nápovědy"), Some(Hint::None));
        assert_eq!(parse_hint("Nápověda"), Some(Hint::Hint));
        assert_eq!(parse_hint("totálka"), Some(Hint::Solution));
        assert_eq!(parse_hint(""), Some(Hint::Unknown));
        assert_eq!(parse_hint("?"), Some(Hint::Unknown));
        assert_eq!(parse_hint("huh"), None);
    }

    #[test]
    fn manifest() {
        let m = parse_manifest(
            "title = \"Závod\"\n[[category]]\nid = \"M\"\nname = \"Kategorie M\"\ncolor = \"#2f9e44\"\nlimit = \"16:15\"\n\
             times = \"m.csv\"\nroute = \"m.gpx\"\nsites = \"ms.csv\"\n",
        )
        .unwrap();
        assert_eq!(m.speed_kmh, 5.0);
        assert_eq!(m.merge_radius_m, 15.0);
        assert_eq!(m.categories[0].hints, None);
        assert_eq!(m.categories[0].speed_kmh, None);
        assert!(parse_manifest("title = \"x\"").is_err());
    }

    #[test]
    fn sites() {
        let s = parse_sites("stanoviste,lat,lon,misto\nM0,49.2,16.6,\nM1,\"49,3\",16.7,Rozcestí\n").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].pos, LatLon::new(49.3, 16.7));
        assert_eq!(s[0].misto, None);
        assert_eq!(s[1].misto.as_deref(), Some("Rozcestí"));
        assert!(parse_sites("stanoviste,lat,lon\nM0,x,16\n").is_err());
    }

    #[test]
    fn norm_strips_diacritics() {
        assert_eq!(norm("  Cíl "), "cil");
        assert_eq!(norm("Číslo   hlídky"), "cislo hlidky");
    }
}
