# Race Replay
 
> [!IMPORTANT] Kód aplikace je celý vibe-coded, napsal ho Claude. Specifikace aplikace a návrh simulačního algoritmu jsou dílem člověka s pomocí Clauda. Zobrazená data jsou skutečná a anonymizovaná a zachycují průběh soutěže ZaPsem 2026, kterou pořádalo Sdružení pionýrských tábornických oddílů z Brna.
 
Statická webová aplikace (Rust, Yew, WebAssembly, Leaflet), která na mapě přehrává šifrovací závod. Hlídky všech kategorií se po minutách posouvají po trase podle časů příchodů, záznamů o nápovědách a dohodnutého rozdělení času na pobyt na stanovišti a chůzi.
 
## Data
 
Všechna zobrazovaná data jsou ve složce `data/`:
 
- `race.toml`: název, nastavení a pro každou kategorii jeden blok `[[category]]` s id, názvem, barvou, časovým limitem a jejími čtyřmi soubory.
- `*_casy.csv`: časy příchodů. Číslo hlídky a pak sloupec pro každé stanoviště v pořadí trasy (`09:45`), prázdná buňka znamená chybějící čas.
- `*_napovedy.csv`: nápovědy na stanovištích (`bez nápovědy`, `nápověda`, `totálka`, prázdné = neznámo). Sloupec startu se ignoruje. Nepovinné.
- `trasa_*.gpx`: trasa exportovaná z mapy.cz (jedna stopa).
- `*_stanoviste.csv`: `stanoviste,lat,lon` v desetinných stupních. Nepovinný sloupec `misto` pojmenuje místo společné více kategoriím. Řádky začínající `#` se přeskakují.
Stanoviště různých kategorií, která jsou od sebe blíž než `merge_radius_m` (15 m) nebo mají stejné `misto`, se sloučí do jednoho místa. Totéž platí pro MZ a Cíl na stejných souřadnicích.
 
Soubor `m_stanoviste.csv` je zatím jen provizorní. M0 je začátek trasy a MZ/Cíl její konec, ale M1–M6 jsou zdvojené body z GPX, ne skutečná stanoviště. Je potřeba doplnit skutečné souřadnice a přidat kategorie S a R (v `race.toml` jsou zakomentované).
 
## Mapové dlaždice
 
Klíč k API Mapy.com patří do `.env` (`MAPY_API_KEY=...`). Pak s trasami všech kategorií:
 
```
python3 fetch_tiles.py --gpx data/trasa_m.gpx data/trasa_s.gpx data/trasa_r.gpx --zoom 12-18 --dry-run
python3 fetch_tiles.py --gpx data/trasa_m.gpx data/trasa_s.gpx data/trasa_r.gpx --zoom 12-18
```
 
První příkaz jen vypíše, kolik dlaždic (a tedy požadavků na API) by to stálo. Už stažené dlaždice se znovu nestahují. Bez dlaždic aplikace běží na prázdném pozadí a upozorní na to v poznámkách k datům.
 
## Spuštění
 
Jednou na začátku:
 
```
rustup target add wasm32-unknown-unknown
cargo install trunk
```
 
Při vývoji stačí `trunk serve --open`, jinak:
 
```
trunk build --release
python3 -m http.server 8080 --directory dist     # pak otevřít http://localhost:8080
```
 
`dist/` je obyčejná statická složka s aplikací, Leafletem, fonty, `data/` a `tiles/`. Pro umístění do podsložky webu se staví s `--public-url /slozka/`. Otevřít `dist/index.html` přímo z disku nejde, musí běžet přes webový server.
 
## Ovládání
 
- **Mezerník** spouští a zastavuje přehrávání, **← / →** posouvá o minutu, se **Shiftem** o deset minut, **Esc** zavře panel.
- Kliknutí na tečku ukáže hlídku: stav, součty nápověd a totálek, poslední a další stanoviště, rychlost chůze, celý závod jako pruh a v cíli umístění.
- Kliknutí na stanoviště ukáže hlídky, které tam právě jsou, a jak dlouho tam jsou vzhledem k hranicím 20 a 40 minut.
- Kliknutím na štítek kategorie se kategorie skryje nebo zobrazí. Když není nic vybráno, panel ukazuje přehled a **Poznámky k datům**, tedy co aplikace doplnila, vynechala nebo jí přišlo podezřelé.

