#!/usr/bin/env python3
"""
fetch_tiles.py - download Mapy.com map tiles ONCE and store them on disk.

The Rust/Yew app then serves the stored tiles itself and never calls the API.

    python3 fetch_tiles.py --gpx data/trasa_m.gpx --zoom 12-18 --dry-run
    python3 fetch_tiles.py --gpx data/trasa_m.gpx data/trasa_d.gpx --zoom 12-18

The area is the bounding box of all given GPX files (tracks, routes and
waypoints), widened by --pad metres. Alternatively give --bbox.

Output (default folder ./tiles):
    tiles/<mapset>/<z>/<x>/<y>.png     the tiles (standard XYZ layout)
    tiles/tiles.json                   manifest read by the app
    tiles/mapy-logo.svg                Mapy.com logo (attribution requires it)

The API key is read from --api-key, the MAPY_API_KEY environment variable, or
a .env file (MAPY_API_KEY=...) in the current folder or next to this script.

Re-running is safe: tiles already on disk are skipped, so an interrupted run
can be resumed and nothing is ever downloaded twice. Before downloading, the
script shows how many tiles (API requests) it will make and asks to confirm.

Only the Python standard library is used.
"""

import argparse
import datetime as dt
import json
import math
import os
import sys
import threading
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

API_BASE = "https://api.mapy.com"
TILE_PATH = "/v1/maptiles/{mapset}/{size}/{z}/{x}/{y}"
LOGO_URL = "https://api.mapy.com/img/api/logo.svg"
ATTRIBUTION_HTML = ('<a href="https://api.mapy.com/copyright" target="_blank" rel="noopener">'
                    "&copy; Seznam.cz a.s. a další</a>")
LOGO_LINK = "https://mapy.com/"
MAPSETS = ("basic", "outdoor", "winter", "aerial", "names-overlay")
RETINA_MAPSETS = ("basic", "outdoor")
EXT_BY_TYPE = {"image/png": "png", "image/jpeg": "jpg", "image/jpg": "jpg", "image/webp": "webp"}
USER_AGENT = "race-replay-tile-cache/1.0"


# ---------------------------------------------------------------- area

def gpx_points(path):
    """All lat/lon points of a GPX file (trkpt, rtept, wpt), namespace-agnostic."""
    root = ET.parse(path).getroot()
    pts = []
    for e in root.iter():
        if e.tag.rsplit("}", 1)[-1] in ("trkpt", "rtept", "wpt"):
            pts.append((float(e.get("lat")), float(e.get("lon"))))
    if not pts:
        raise SystemExit(f"{path}: no points found.")
    return pts


def padded_bbox(points, pad_m):
    lats = [p[0] for p in points]
    lons = [p[1] for p in points]
    s, n, w, e = min(lats), max(lats), min(lons), max(lons)
    mid = math.radians((s + n) / 2)
    dlat = pad_m / 111_320
    dlon = pad_m / (111_320 * max(math.cos(mid), 1e-6))
    return s - dlat, w - dlon, n + dlat, e + dlon


def tile_xy(lat, lon, z):
    """Web Mercator (XYZ / slippy map) tile containing the point."""
    n = 2 ** z
    lat = max(min(lat, 85.05112878), -85.05112878)
    x = int((lon + 180.0) / 360.0 * n)
    y = int((1.0 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2.0 * n)
    return min(max(x, 0), n - 1), min(max(y, 0), n - 1)


def tile_ranges(bbox, zooms):
    s, w, n, e = bbox
    out = {}
    for z in zooms:
        x0, y0 = tile_xy(n, w, z)   # north-west corner: smallest x and y
        x1, y1 = tile_xy(s, e, z)   # south-east corner: largest x and y
        out[z] = (x0, x1, y0, y1)
    return out


def parse_zoom(s):
    if "-" in s:
        a, b = s.split("-", 1)
        lo, hi = int(a), int(b)
    else:
        lo = hi = int(s)
    if not (0 <= lo <= hi <= 22):
        raise SystemExit(f"--zoom '{s}': use e.g. 12-18 (0..22).")
    return list(range(lo, hi + 1))


# ---------------------------------------------------------------- api key

def read_env_file(path):
    vals = {}
    try:
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            k = k.strip().removeprefix("export ").strip()
            vals[k] = v.strip().strip('"').strip("'")
    except FileNotFoundError:
        pass
    return vals


def find_api_key(args):
    if args.api_key:
        return args.api_key
    if os.environ.get("MAPY_API_KEY"):
        return os.environ["MAPY_API_KEY"]
    for p in [args.env_file] if args.env_file else [Path.cwd() / ".env", Path(__file__).resolve().parent / ".env"]:
        v = read_env_file(p).get("MAPY_API_KEY")
        if v:
            return v
    return None


# ---------------------------------------------------------------- download

class Fatal(Exception):
    pass


class Downloader:
    def __init__(self, args, key):
        self.base = args.base_url.rstrip("/")
        self.key = key
        self.size = "256@2x" if args.retina else "256"
        self.out = Path(args.out)
        self.retries = args.retries
        self.delay = args.delay
        self.lock = threading.Lock()
        self.stats = {"downloaded": 0, "skipped": 0, "missing": 0, "failed": 0, "bytes": 0}
        self.ext = {}            # mapset -> extension (decided by the first tile)
        self.bad_streak = 0      # consecutive client errors: stop early instead of burning credits
        # Tiles the API answered 404 for, remembered so a re-run does not ask again.
        self.missing = {}
        for m in args.mapset:
            p = self.out / m / "missing.json"
            try:
                self.missing[m] = set(json.loads(p.read_text(encoding="utf-8")))
            except (FileNotFoundError, ValueError):
                self.missing[m] = set()

    def save_missing(self):
        for m, keys in self.missing.items():
            if keys:
                p = self.out / m / "missing.json"
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(json.dumps(sorted(keys)), encoding="utf-8")

    def url(self, mapset, z, x, y):
        path = TILE_PATH.format(mapset=mapset, size=self.size, z=z, x=x, y=y)
        return f"{self.base}{path}?apikey={self.key}"

    def existing(self, mapset, z, x, y):
        d = self.out / mapset / str(z) / str(x)
        for ext in ("png", "jpg", "webp"):
            if (d / f"{y}.{ext}").exists():
                return ext
        return None

    def fetch(self, mapset, z, x, y):
        if self.existing(mapset, z, x, y) or f"{z}/{x}/{y}" in self.missing[mapset]:
            with self.lock:
                self.stats["skipped"] += 1
            return
        req = urllib.request.Request(self.url(mapset, z, x, y), headers={"User-Agent": USER_AGENT})
        for attempt in range(self.retries + 1):
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    ctype = r.headers.get("Content-Type", "").split(";")[0].strip().lower()
                    data = r.read()
                if not ctype.startswith("image/"):
                    raise Fatal(f"Expected an image for {mapset}/{z}/{x}/{y}, got '{ctype}': "
                                f"{data[:200].decode('utf-8', 'replace')}")
                ext = EXT_BY_TYPE.get(ctype, "png")
                with self.lock:
                    ext = self.ext.setdefault(mapset, ext)
                    self.bad_streak = 0
                path = self.out / mapset / str(z) / str(x) / f"{y}.{ext}"
                path.parent.mkdir(parents=True, exist_ok=True)
                tmp = path.with_suffix(path.suffix + ".part")
                tmp.write_bytes(data)
                tmp.replace(path)
                with self.lock:
                    self.stats["downloaded"] += 1
                    self.stats["bytes"] += len(data)
                if self.delay:
                    time.sleep(self.delay)
                return
            except urllib.error.HTTPError as e:
                body = e.read()[:300].decode("utf-8", "replace")
                if e.code in (401, 403):
                    raise Fatal(f"API refused the key (HTTP {e.code}): {body}")
                if e.code == 404:
                    # Outside the map's coverage or zoom range: nothing to cache.
                    with self.lock:
                        self.stats["missing"] += 1
                        self.missing[mapset].add(f"{z}/{x}/{y}")
                    return
                if e.code == 429 or e.code >= 500:
                    wait = float(e.headers.get("Retry-After") or 0) or min(60, 2 ** attempt)
                    time.sleep(wait)
                    continue
                with self.lock:
                    self.bad_streak += 1
                    streak = self.bad_streak
                if streak >= 5:
                    raise Fatal(f"HTTP {e.code} for several tiles in a row, stopping: {body}")
                break
            except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
                if attempt == self.retries:
                    break
                time.sleep(min(30, 2 ** attempt))
        with self.lock:
            self.stats["failed"] += 1


def download_logo(out, base):
    path = Path(out) / "mapy-logo.svg"
    if path.exists():
        return True
    url = LOGO_URL if base == API_BASE else f"{base}/img/api/logo.svg"
    try:
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=30) as r:
            path.write_bytes(r.read())
        return True
    except Exception as e:  # the app falls back to a text link
        print(f"  Could not download the logo ({e}); the app will show a text link instead.")
        return False


def write_manifest(args, bbox, zooms, dl, counts):
    path = Path(args.out) / "tiles.json"
    old = {}
    if path.exists():
        try:
            old = json.loads(path.read_text(encoding="utf-8"))
        except ValueError:
            old = {}
    mapsets = old.get("mapsets", {})
    for m in args.mapset:
        ext = dl.ext.get(m) or mapsets.get(m, {}).get("ext")
        if ext is None:
            for z in zooms:   # every tile was skipped: look at what is on disk
                d = Path(args.out) / m / str(z)
                f = next(d.rglob("*.*"), None) if d.exists() else None
                if f is not None:
                    ext = f.suffix.lstrip(".")
                    break
        mapsets[m] = {"ext": ext or "png", "tiles": counts[m]}
    s, w, n, e = bbox
    old_b = old.get("bounds")
    if old_b and old.get("min_zoom") is not None:
        # Keep the union of earlier and new areas so earlier tiles stay visible.
        s, w = min(s, old_b[0][0]), min(w, old_b[0][1])
        n, e = max(n, old_b[1][0]), max(e, old_b[1][1])
    manifest = {
        "version": 1,
        "bounds": [[round(s, 6), round(w, 6)], [round(n, 6), round(e, 6)]],
        "min_zoom": min(zooms + ([old["min_zoom"]] if old.get("min_zoom") is not None else [])),
        "max_zoom": max(zooms + ([old["max_zoom"]] if old.get("max_zoom") is not None else [])),
        "tile_size": 256,
        "retina": bool(args.retina),
        "mapsets": mapsets,
        "default_mapset": old.get("default_mapset") or args.mapset[0],
        "attribution_html": ATTRIBUTION_HTML,
        "logo": "mapy-logo.svg" if (Path(args.out) / "mapy-logo.svg").exists() else None,
        "logo_link": LOGO_LINK,
        "fetched_at": dt.datetime.now().astimezone().isoformat(timespec="seconds"),
    }
    path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return path


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    area = ap.add_mutually_exclusive_group(required=True)
    area.add_argument("--gpx", nargs="+", help="GPX file(s) whose area to cover")
    area.add_argument("--bbox", help="south,west,north,east in degrees")
    ap.add_argument("--pad", type=float, default=300, help="margin around the GPX area in metres (300)")
    ap.add_argument("--zoom", default="12-18", help="zoom range, e.g. 12-18 (default)")
    ap.add_argument("--mapset", nargs="+", default=["outdoor"], choices=MAPSETS,
                    help="map set(s) to download (outdoor)")
    ap.add_argument("--retina", action="store_true",
                    help="download 512 px high-DPI tiles (basic and outdoor only)")
    ap.add_argument("--out", default="tiles", help="output folder (tiles)")
    ap.add_argument("--api-key", help="API key (otherwise MAPY_API_KEY from the environment or .env)")
    ap.add_argument("--env-file", help="path of the .env file to read")
    ap.add_argument("--workers", type=int, default=4, help="parallel downloads (4)")
    ap.add_argument("--delay", type=float, default=0.05, help="pause after each tile per worker, s (0.05)")
    ap.add_argument("--retries", type=int, default=4, help="retries for network errors / 429 / 5xx (4)")
    ap.add_argument("--dry-run", action="store_true", help="only show how many tiles would be downloaded")
    ap.add_argument("--yes", action="store_true", help="do not ask for confirmation")
    ap.add_argument("--base-url", default=API_BASE, help=argparse.SUPPRESS)  # for testing
    args = ap.parse_args()

    if args.retina:
        bad = [m for m in args.mapset if m not in RETINA_MAPSETS]
        if bad:
            raise SystemExit(f"Retina tiles exist only for {', '.join(RETINA_MAPSETS)}, not {', '.join(bad)}.")

    if args.gpx:
        pts = [p for f in args.gpx for p in gpx_points(f)]
        bbox = padded_bbox(pts, args.pad)
    else:
        try:
            bbox = tuple(float(v) for v in args.bbox.split(","))
            assert len(bbox) == 4 and bbox[0] < bbox[2] and bbox[1] < bbox[3]
        except (ValueError, AssertionError):
            raise SystemExit("--bbox must be south,west,north,east with south < north and west < east.")
    zooms = parse_zoom(args.zoom)
    ranges = tile_ranges(bbox, zooms)

    s, w, n, e = bbox
    width_km = (e - w) * 111.32 * math.cos(math.radians((s + n) / 2))
    height_km = (n - s) * 111.32
    print(f"Area: {s:.5f},{w:.5f} – {n:.5f},{e:.5f}  (about {width_km:.1f} x {height_km:.1f} km)")
    print(f"Map sets: {', '.join(args.mapset)}   tile size: {'512 px (retina)' if args.retina else '256 px'}")
    total_per_set = 0
    print("  zoom   tiles   done")
    out = Path(args.out)
    todo_total = 0
    counts = {}
    for m in args.mapset:
        counts[m] = 0
    known_missing = {}
    for m in args.mapset:
        try:
            known_missing[m] = set(json.loads((out / m / "missing.json").read_text(encoding="utf-8")))
        except (FileNotFoundError, ValueError):
            known_missing[m] = set()
    for z, (x0, x1, y0, y1) in ranges.items():
        k = (x1 - x0 + 1) * (y1 - y0 + 1)
        total_per_set += k
        have = 0
        for m in args.mapset:
            d = out / m / str(z)
            have += sum(1 for x in range(x0, x1 + 1) for y in range(y0, y1 + 1)
                        if f"{z}/{x}/{y}" in known_missing[m]
                        or any((d / str(x) / f"{y}.{ext}").exists() for ext in ("png", "jpg", "webp")))
            counts[m] += k
        todo_total += k * len(args.mapset) - have
        print(f"  {z:>4}  {k:>6}   {have:>6}")
    total = total_per_set * len(args.mapset)
    print(f"Total {total} tiles, {total - todo_total} already done, {todo_total} to download "
          f"= {todo_total} API requests.")

    if args.dry_run:
        return
    if todo_total == 0:
        print("Nothing to download.")
    else:
        key = find_api_key(args)
        if not key:
            raise SystemExit("No API key. Put MAPY_API_KEY=... into .env, set MAPY_API_KEY, or use --api-key.")
        if not args.yes:
            try:
                ans = input(f"Download {todo_total} tiles? [y/N] ").strip().lower()
            except EOFError:
                ans = ""
            if ans not in ("y", "yes", "a", "ano"):
                print("Cancelled.")
                return

    out.mkdir(parents=True, exist_ok=True)
    dl = Downloader(args, find_api_key(args) or "")
    if todo_total:
        jobs = [(m, z, x, y) for m in args.mapset for z, (x0, x1, y0, y1) in ranges.items()
                for x in range(x0, x1 + 1) for y in range(y0, y1 + 1)]
        t0 = time.time()
        done = 0
        try:
            with ThreadPoolExecutor(max_workers=max(1, args.workers)) as pool:
                futs = [pool.submit(dl.fetch, *j) for j in jobs]
                for f in as_completed(futs):
                    f.result()
                    done += 1
                    if done % 50 == 0 or done == len(jobs):
                        st = dl.stats
                        print(f"\r  {done}/{len(jobs)}  downloaded {st['downloaded']}, skipped {st['skipped']}, "
                              f"missing {st['missing']}, failed {st['failed']}", end="", flush=True)
        except Fatal as e:
            print(f"\nStopped: {e}")
            for f in futs:
                f.cancel()
            dl.save_missing()
            write_manifest(args, bbox, zooms, dl, counts)
            sys.exit(1)
        except KeyboardInterrupt:
            print("\nInterrupted. Run the same command again to continue; finished tiles are kept.")
            dl.save_missing()
            write_manifest(args, bbox, zooms, dl, counts)
            sys.exit(130)
        print()
        dl.save_missing()
        st = dl.stats
        print(f"Done in {time.time() - t0:.0f} s: {st['downloaded']} downloaded "
              f"({st['bytes'] / 1e6:.1f} MB), {st['skipped']} already on disk, "
              f"{st['missing']} not available (404), {st['failed']} failed.")
        if st["failed"]:
            print("Some tiles failed. Run the same command again to retry only those.")
    download_logo(args.out, args.base_url.rstrip("/"))
    path = write_manifest(args, bbox, zooms, dl, counts)
    print(f"Manifest: {path}")


if __name__ == "__main__":
    main()
