# odjazd-web HTTP API

The JSON API behind the odjazd web app. Every endpoint is a `GET` under `/api`.

- Times are Unix timestamps in seconds. Every response carries `now`, the server's clock,
  so countdowns stay right on a phone whose clock is off.
- Distances are in metres, durations in seconds.
- Errors come with a 4xx/5xx status and `{"error": "…"}`. Right after the server starts,
  while the timetable is still loading, every endpoint answers `503`.

## Shared objects

**Route**: a line.

```json
{ "name": "9", "kind": "tram", "color": "#B60000", "text": "#FFFFFF" }
```

`kind` is `tram`, `bus`, `metro` or `rail`. `color` and `text` may be `null`.

**Stop**: a single pole or platform, e.g. *Metro Świętokrzyska 03*.

```json
{ "id": "701403", "name": "Metro Świętokrzyska", "code": "03",
  "street": "Świętokrzyska", "lat": 52.234869, "lon": 21.007222 }
```

`code` is the pole number from the sign (`03`) or the metro line of a platform (`M1`).
`code` and `street` may be `null`.

**Departure**

```json
{
  "route": { "name": "9", "kind": "tram", "color": "#B60000", "text": "#FFFFFF" },
  "headsign": "Banacha",
  "stop": "701403",
  "scheduled": 1793390520,
  "expected": 1793390640,
  "delay": 120,
  "live": true,
  "approximate": false,
  "stops_away": 3
}
```

| Field | Meaning |
|---|---|
| `scheduled` | Departure time from the timetable. |
| `expected` | When it will really leave: `scheduled + delay` for live trips, else `scheduled`. Sort and count down by this. |
| `delay` | Seconds late (negative: early), estimated from the vehicle's GPS position. `null` without live data. |
| `live` | A vehicle doing this trip is on the road and being tracked right now. |
| `approximate` | The timetable only says "every few minutes" (the metro), so the time is an estimate. |
| `stops_away` | How many stops the live vehicle still has to make before this one. `null` when not live. |

## `GET /api/nearby?lat=…&lon=…`

The poles nearest to a point, each with its next departures.

Optional: `radius` (metres, default 600, at most 2000), `limit` (poles, default 12),
`departures` (per pole, default 4).

```json
{
  "now": 1793390400,
  "stops": [
    {
      "id": "701403", "name": "Metro Świętokrzyska", "code": "03",
      "street": "Świętokrzyska", "lat": 52.234869, "lon": 21.007222,
      "distance": 85,
      "walk": 75,
      "routes": [ { "name": "9", "kind": "tram", "color": "#B60000", "text": "#FFFFFF" } ],
      "departures": [ "…Departure objects…" ]
    }
  ]
}
```

Poles come nearest first. `distance` is in a straight line; `walk` is the estimated walk
in seconds, with a detour factor for streets. `routes` lists every line that leaves from
the pole today, and may be longer than what `departures` shows.

## `GET /api/departures?stops=701403,701404`

More departures from one or more poles (comma-separated ids). Optional: `limit`
(default 12).

```json
{ "now": 1793390400, "departures": [ "…Departure objects…" ] }
```

## `GET /api/plan?from=LAT,LON&to=LAT,LON`

Answers "which nearby pole should I walk to, to get to `to` soonest?". It plans the
fastest journey from each pole within walking distance of `from` and returns the best
ones, soonest arrival first. Live delays are taken into account. Optional: `at` (Unix time
to leave at instead of now).

```json
{
  "now": 1793390400,
  "options": [
    {
      "start": { "…Stop fields…": "", "distance": 85, "walk": 75 },
      "leave": 1793390500,
      "arrive": 1793392000,
      "transfers": 1,
      "legs": [
        { "kind": "walk", "from": null, "to": { "…Stop…": "" }, "seconds": 75, "distance": 85 },
        {
          "kind": "ride",
          "route": { "…Route…": "" },
          "headsign": "Banacha",
          "from": { "…Stop…": "" },
          "to": { "…Stop…": "" },
          "departure": 1793390640,
          "arrival": 1793391500,
          "scheduled": 1793390520,
          "delay": 120,
          "live": true,
          "approximate": false,
          "stops": 6
        },
        { "kind": "walk", "from": { "…Stop…": "" }, "to": null, "seconds": 200, "distance": 230 }
      ]
    }
  ],
  "walk": { "seconds": 1500, "distance": 1800 }
}
```

- `options` holds at most one journey per starting pole, best first. It is empty when
  nothing gets there in the next few hours.
- `leave` is when to set off from `from` so as to catch the first vehicle.
- In legs, a `from` or `to` of `null` means the requested `from` or `to` point.
- A ride's `departure` and `arrival` are expected times (live delay included);
  `scheduled` is the timetable departure. `stops` counts the stops ridden.
- `walk` is walking the whole way, given only when it takes under 40 minutes; otherwise
  `null`.

## `GET /api/stops?q=…`

Stops matching a name, for picking a destination. Forgiving like the CLI: `narutowica`
finds *Pl. Narutowicza*.

```json
{
  "stops": [
    { "name": "Pl. Narutowicza", "lat": 52.2189, "lon": 20.9842,
      "poles": [ { "…Stop…": "" } ] }
  ]
}
```

## `GET /api/vehicles?lat=…&lon=…`

Live vehicles near a point, for the map. Optional: `radius` (metres, default 1500, at most
5000).

```json
{
  "now": 1793390400,
  "vehicles": [
    {
      "route": { "…Route…": "" }, "headsign": "Banacha",
      "lat": 52.2351, "lon": 21.0079, "bearing": 61.7,
      "delay": 120, "number": "9947", "updated": 1793390395
    }
  ]
}
```

`bearing` (degrees clockwise from north) and `delay` may be `null`.

## `GET /api/status`

```json
{
  "now": 1793390400,
  "timetable": { "version": "2026-10-30 00:15:11", "days": ["2026-10-29", "2026-10-30", "2026-10-31"] },
  "realtime": { "updated": 1793390395, "vehicles": 1234, "tracked": 1100 }
}
```

`realtime.updated` is when the city last published vehicle positions; it is `null` until
the first download succeeds. The data licence requires showing this time next to the
credit (see below).

## Data credits the app must show

- Timetable: *Zarząd Transportu Miejskiego w Warszawie* (https://ztm.waw.pl), converted
  to GTFS by *Mikołaj Kuranowski* (https://mkuran.pl/gtfs/).
- Vehicle positions: *Miasto Stołeczne Warszawa*, with the download time
  (`realtime.updated`) and a link to http://api.um.warszawa.pl.
- Map: © OpenStreetMap contributors, plus the tile provider's own credit.
