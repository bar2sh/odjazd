<p align="center">
  <img src="docs/banner.svg" alt="odjazd: Warsaw departures, right in your terminal" width="100%">
</p>

<p align="center">
  <a href="https://github.com/bar2sh/odjazd/actions/workflows/ci.yml"><img src="https://github.com/bar2sh/odjazd/actions/workflows/ci.yml/badge.svg" alt="CI status"></a>
  <img src="https://img.shields.io/badge/Rust-2024_edition-dea584?logo=rust&logoColor=white" alt="Rust 2024 edition">
  <img src="https://img.shields.io/badge/data-GTFS-4c8eda" alt="GTFS data">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <b>Next departures from any Warsaw public transport stop, right in your terminal.</b><br>
  <sub><i>Odjazd</i> is Polish for “departure”.</sub>
</p>

<p align="center">
  <img src="docs/demo.svg" width="640" alt="Typing “odjazd swietokrzyska” shows the next trams and metro trains at Metro Świętokrzyska, each line in its own colours">
</p>

## Highlights

- **Forgiving stop search.** `plac narutowicza`, `pl narutowicza` and `narutowica` all
  find *Pl. Narutowicza*; Polish letters are optional.
- **Whole stops, not single poles.** All poles of a stop are shown together, and a metro
  station is merged with the "Metro …" bus and tram stops above it.
- **Lines in their real colours**: red trams, purple buses, blue M1. The column after the
  direction is the pole number from the stop sign, and `~` marks metro times, which the
  timetable only gives as "every few minutes".
- **GTFS done right.** Night trips written as `24:05:00`, holiday timetables, metro
  headways, the 25-hour day when the clocks go back, and trips that end at the stop or
  only loop around it.
- **One small binary.** No runtime, no account, no API key.

## Install

You need a recent stable Rust ([rustup.rs](https://rustup.rs)).

```bash
cargo install --git https://github.com/bar2sh/odjazd odjazd
```

The first run downloads Warsaw's timetable (about 110 MB, 570 MB unpacked) into your
cache directory.

## Usage

```bash
odjazd "dworzec centralny"        # the next 12 departures
odjazd politechnika -n 5          # just 5 of them
odjazd centrum --at 23:50         # what still runs late tonight
odjazd stops narutowicza          # which stops match, with their poles
odjazd update                     # download a fresh timetable
odjazd update --url <gtfs.zip>    # ...or another city's GTFS feed
```

The timetable lives in the system cache directory (`~/Library/Caches/odjazd` on macOS,
`~/.cache/odjazd` on Linux). Set `ODJAZD_DATA_DIR` to keep it somewhere else.

## How it works

odjazd reads Warsaw's timetable as [GTFS](https://gtfs.org), the format behind transit
directions in Google Maps: a zip of CSV tables.

```mermaid
flowchart LR
    zip["GTFS feed<br/>zip, 108 MB"] -->|odjazd update| cache[("Unpacked tables<br/>in the cache")]
    cache -->|on start| small["stops · lines · calendar"]
    small --> search["Stop search<br/>plac narutowicza → Pl. Narutowicza"]
    cache -->|streamed once per query| scan["stop_times.txt<br/>8 million rows"]
    search --> scan
    scan --> board["Departures from yesterday's,<br/>today's and tomorrow's timetables"]
```

A few details make this harder than it looks:

- **A service day is not a calendar day.** GTFS counts time from "noon minus 12 hours" of
  the day a timetable belongs to, so Friday's last night bus leaves at `24:05:00`. odjazd
  reads yesterday's, today's and tomorrow's timetables and turns every time into a real
  instant with [jiff](https://github.com/BurntSushi/jiff), which also gets the 25-hour
  day in October right.
- **The metro has no timetable**, only headways: "every 150 seconds between 05:50 and
  09:23" in `frequencies.txt`. odjazd expands them into estimated departures, marked `~`.
- **Arriving is not departing.** A trip is listed only if it goes on to somewhere else.
  A tram that ends here, or only loops round to another pole of the same stop, is not.
- **Speed.** Most of the 8 million rows belong to other days. Trips are looked up once
  per trip rather than once per row, so those rows are skipped after a single comparison.
  Trimming whitespace in every CSV field took two thirds of the run time, so only the
  fields in use are trimmed. A query takes about 0.75 s on an Apple M-series laptop.

The code is split into two crates:

| Crate | What it does |
|---|---|
| [`odjazd-core`](crates/odjazd-core) | GTFS parsing, stop search, departures. No networking or terminal I/O, so it can be compiled to WebAssembly. |
| [`odjazd`](crates/odjazd) | The command-line app: downloading, arguments, colours. |

Built with [clap](https://crates.io/crates/clap), [csv](https://crates.io/crates/csv) +
[serde](https://serde.rs), [jiff](https://crates.io/crates/jiff),
[ureq](https://crates.io/crates/ureq), [zip](https://crates.io/crates/zip) and
[anstream](https://crates.io/crates/anstream).

## Roadmap

- [x] **v0.1:** departure board for any stop, forgiving stop search
- [ ] **v0.2:** a live, self-refreshing board in a TUI (`ratatui`), and instant start from
  a precomputed binary timetable (`rkyv`)
- [ ] **v0.3:** where is my tram right now: live vehicle positions from the city's open
  data API
- [ ] **v0.4:** journey planner (`odjazd route A B`) using the RAPTOR algorithm (Delling,
  Pajor, Werneck, Microsoft Research), with benchmarks
- [ ] **v1.0:** `odjazd-core` compiled to WebAssembly, with a map you can try in the
  browser

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

The tests run against a tiny hand-written feed in
[`crates/odjazd-core/tests/fixtures/mini`](crates/odjazd-core/tests/fixtures/mini) that
covers each tricky case above. The animated demo is rendered from real output by
[`docs/demo.py`](docs/demo.py): `cargo build --release && python3 docs/demo.py swietokrzyska`.

## Data

The timetable comes from [Zarząd Transportu Miejskiego w Warszawie](https://ztm.waw.pl),
converted to GTFS by [Mikołaj Kuranowski](https://mkuran.pl/gtfs/). odjazd downloads it
from there and does not redistribute it.

## License

[MIT](LICENSE)
