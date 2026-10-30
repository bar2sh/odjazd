// odjazd web app: the poles around you with live departures, and which pole to walk to
// for the fastest way home or to work.

import * as api from './api.js';
import * as fmt from './format.js';
import { PLACE_ICONS, createMap } from './map.js';

const REFRESH_MS = 20_000;
const RETRY_MS = 5_000;
const STATUS_MS = 60_000;
const STALE_MS = 90_000;
const MOVED_M = 25; // refetch once you have walked this far
const STORE = 'odjazd.destinations';
const HASHES = { nearby: '', home: '#dom', work: '#praca' };
const PLACES = {
  home: { to: 'do domu', where: 'Gdzie jest dom?', set: 'Ustaw dom', what: 'dom' },
  work: { to: 'do pracy', where: 'Gdzie jest praca?', set: 'Ustaw pracę', what: 'praca' },
};

const ICON = {
  pin: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 21s-7-6.2-7-11.5A7 7 0 0 1 19 9.5C19 14.8 12 21 12 21z"/><circle cx="12" cy="9.5" r="2.5"/></svg>',
  pinOff: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8.2 4.2A7 7 0 0 1 19 9.5c0 1.6-.6 3.2-1.5 4.7M15 18a40 40 0 0 1-3 3s-7-6.2-7-11.5c0-.8.1-1.5.4-2.2M3 3l18 18"/></svg>',
  walk: '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="13" cy="4.5" r="1.8"/><path d="M9.5 21l2.3-6.3 2.7 2.6V21M11.8 14.7L13 8.8l-3 1.4-1.4 3.1M13 8.8l1.8 3.4 3 1.1"/></svg>',
  offline: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M2 8.8a15 15 0 0 1 4.6-2.7M22 8.8A15 15 0 0 0 10.6 4.9M5 12.3a10 10 0 0 1 3.7-2.2M19 12.3a10 10 0 0 0-3-1.9M8.5 15.8a5 5 0 0 1 7 0M3 3l18 18"/><circle cx="12" cy="19.3" r="1"/></svg>',
  alert: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l9.5 16.5h-19z"/><path d="M12 10v4.5M12 17.5v.01"/></svg>',
  chevron: '<svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="M9 6l6 6-6 6"/></svg>',
  ...PLACE_ICONS,
};

const $ = (selector) => document.querySelector(selector);
const esc = fmt.esc;
const content = $('#content');

const state = {
  view: viewFromHash(),
  pos: null, // { lat, lon, acc, source: 'gps' | 'url' | 'map' }
  geo: 'waiting', // 'waiting' | 'ok' | 'denied' | 'unavailable'
  radius: 600,
  nearby: null,
  more: new Map(), // pole id → its longer departure list, once expanded
  expanded: new Set(),
  plans: {}, // 'home' | 'work' → { to, data }
  open: {}, // 'home' | 'work' → index of the option drawn on the map
  vehicles: [],
  dests: loadDests(),
  status: null,
  problem: null, // null | 'offline' | 'loading' | 'error'
  problemText: '',
  fetched: null, // { pos, at }: what the data on screen was fetched for
  picking: null, // null | 'pos' | 'home' | 'work'
  fitted: '',
};

const map = createMap($('#map'), { onTap: tapMap, onPole: showPole });
document.body.classList.toggle('no-map', !map);
if (!map) $('#map-wrap').classList.add('no-map');

/* Position */

let lastFix = null;
let moveTimer = 0;
let asked = null; // the position the latest refresh asked about

function startGps() {
  if (!('geolocation' in navigator)) {
    state.geo = 'unavailable';
    return;
  }
  navigator.geolocation.watchPosition(
    (p) => {
      lastFix = { lat: p.coords.latitude, lon: p.coords.longitude, acc: p.coords.accuracy, source: 'gps' };
      state.geo = 'ok';
      if (!state.pos || state.pos.source === 'gps') setPosition(lastFix);
    },
    (error) => {
      if (error.code === error.PERMISSION_DENIED) state.geo = 'denied';
      else if (state.geo !== 'ok') state.geo = 'unavailable';
      if (state.pos) renderHeader();
      else render();
    },
    { enableHighAccuracy: true, maximumAge: 5000, timeout: 20000 },
  );
}

function setPosition(pos) {
  const first = !state.pos;
  state.pos = pos;
  map?.setUser(pos);
  renderHeader();
  if (first || pos.source !== 'gps') {
    render();
    refresh();
  } else if (!moveTimer && asked && fmt.metres(asked, pos) > MOVED_M) {
    // Not a debounce: while you walk a fix comes every second, so it would never fire.
    moveTimer = setTimeout(refresh, 1500);
  }
}

/** `?at=52.2351,21.0080` pins the position, for testing on a desktop. */
function parseAt(value) {
  const match = /^\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*$/.exec(value ?? '');
  if (!match) return null;
  const [lat, lon] = [Number(match[1]), Number(match[2])];
  return Math.abs(lat) <= 90 && Math.abs(lon) <= 180 ? { lat, lon } : null;
}

/* Destinations */

function loadDests() {
  try {
    return JSON.parse(localStorage.getItem(STORE)) ?? {};
  } catch {
    return {};
  }
}

function saveDests() {
  try {
    localStorage.setItem(STORE, JSON.stringify(state.dests));
  } catch {
    // Private mode: they last until the tab is closed.
  }
}

const destKey = (d) => `${d.lat.toFixed(5)},${d.lon.toFixed(5)}`;

function setDest(kind, dest, { nameIt = false } = {}) {
  state.dests[kind] = dest;
  saveDests();
  delete state.plans[kind];
  state.open[kind] = 0;
  if (state.view === kind) {
    render();
    refresh();
  } else {
    setView(kind);
  }
  if (nameIt) nameDest(kind, dest);
}

/** Names a picked point after the stop nearest to it. */
async function nameDest(kind, dest) {
  try {
    const at = { lat: dest.lat.toFixed(6), lon: dest.lon.toFixed(6) };
    const { stops } = await api.get('/api/nearby', { ...at, radius: 1500, limit: 1, departures: 1 });
    if (stops[0] && state.dests[kind] === dest) {
      dest.label = `przy przystanku ${stops[0].name}`;
      saveDests();
      render();
    }
  } catch {
    // Keep the generic label.
  }
}

/* Fetching */

let timer = 0;
let generation = 0;

async function refresh() {
  clearTimeout(timer);
  clearTimeout(moveTimer);
  moveTimer = 0;
  if (document.hidden || !state.pos) return;
  const run = ++generation;
  const pos = state.pos;
  const view = state.view;
  asked = pos;
  const at = { lat: pos.lat.toFixed(6), lon: pos.lon.toFixed(6) };

  let main = Promise.resolve();
  const extras = [];
  if (view === 'nearby') {
    main = api.get('/api/nearby', { ...at, radius: state.radius }).then((r) => {
      state.nearby = r;
    });
    for (const id of state.expanded) extras.push(loadMore(id));
  } else if (state.dests[view]) {
    const dest = state.dests[view];
    const to = `${dest.lat.toFixed(6)},${dest.lon.toFixed(6)}`;
    main = api.get('/api/plan', { from: `${at.lat},${at.lon}`, to }).then((r) => {
      state.plans[view] = { to: destKey(dest), data: r };
    });
  }
  if (map) {
    extras.push(api.get('/api/vehicles', at).then((r) => {
      state.vehicles = r.vehicles;
    }));
  }

  const [result] = await Promise.allSettled([main, ...extras]);
  if (run !== generation) return;
  if (result.status === 'rejected') {
    state.problem = result.reason?.kind ?? 'error';
    state.problemText = result.reason?.message ?? '';
  } else {
    state.problem = null;
    state.fetched = { pos, at: Date.now() };
  }
  render();
  timer = setTimeout(refresh, state.problem === 'loading' ? RETRY_MS : REFRESH_MS);
}

async function loadMore(id) {
  const { departures } = await api.get('/api/departures', { stops: id, limit: 12 });
  state.more.set(id, departures);
}

async function loadStatus() {
  let ok = false;
  try {
    state.status = await api.get('/api/status');
    ok = true;
    renderHeader();
    renderAbout();
  } catch {
    // The live pill stays off until the server answers.
  }
  setTimeout(loadStatus, ok ? STATUS_MS : RETRY_MS);
}

/* Rendering */

function render() {
  const focus = focusKey();
  renderHeader();
  for (const tab of document.querySelectorAll('.tabs [data-view]')) {
    tab.setAttribute('aria-selected', String(tab.dataset.view === state.view));
  }
  content.innerHTML = state.view === 'nearby' ? renderNearby() : renderPlan(state.view);
  renderMap();
  tick();
  restoreFocus(focus);
}

function renderHeader() {
  const gps = $('#gps');
  const pos = state.pos;
  let text = 'szukam GPS…';
  let quality = '';
  if (pos?.source === 'url') [text, quality] = ['punkt z adresu', 'fair'];
  else if (pos?.source === 'map') [text, quality] = ['wskazane na mapie', 'fair'];
  else if (pos) {
    const acc = Math.round(pos.acc);
    text = `GPS ±${acc} m`;
    quality = acc <= 25 ? 'good' : acc <= 100 ? 'fair' : 'poor';
  } else if (state.geo === 'denied') [text, quality] = ['brak GPS', 'off'];
  else if (state.geo === 'unavailable') [text, quality] = ['słaby GPS', 'off'];
  gps.textContent = text;
  gps.dataset.q = quality;
  gps.title = pos?.source === 'map' && lastFix ? 'Wróć do GPS' : 'Wskaż swoją pozycję na mapie';

  const updated = state.status?.realtime?.updated;
  $('#live').hidden = !(updated && api.now() - updated < 180);
  for (const el of document.querySelectorAll('.rt-time')) {
    el.textContent = updated ? fmt.dateTime(updated) : '—';
  }
}

function renderAbout() {
  const { timetable, realtime } = state.status ?? {};
  $('#about-version').textContent = timetable?.version ? `Wersja rozkładu: ${timetable.version}.` : '';
  const n = realtime?.vehicles;
  $('#about-vehicles').textContent = n
    ? `; na trasach ${n} ${fmt.plural(n, 'pojazd', 'pojazdy', 'pojazdów')}, ` +
      `opóźnienie znane dla ${realtime.tracked}`
    : '';
}

function renderNearby() {
  if (!state.pos) return renderNoPosition();
  const data = state.nearby;
  if (!data) return state.problem ? renderProblem() : skeleton(3);
  if (!data.stops.length) {
    const wider = state.radius < 1500
      ? '<button class="btn" type="button" data-act="wider">Szukaj w promieniu 1,5 km</button>'
      : '';
    return banner() + block(ICON.pin, `Brak przystanków w promieniu ${fmt.distance(state.radius)}`,
      'W pobliżu nie ma żadnego słupka. Może jesteś poza Warszawą?', [wider, pickButton()]);
  }
  return banner() + data.stops.map(renderPole).join('');
}

function renderPole(stop) {
  const open = state.expanded.has(stop.id);
  const departures = (open && state.more.get(stop.id)) || stop.departures;
  const meta = [stop.street, fmt.distance(stop.distance), `${fmt.minutes(stop.walk)} pieszo`]
    .filter(Boolean)
    .join(' · ');
  const list = departures.length
    ? `<ul class="deps">${departures.map(renderDeparture).join('')}</ul>`
    : '<p class="more-hint">Nic stąd nie odjeżdża w najbliższym czasie.</p>';
  const loading = open && !state.more.has(stop.id) ? '<p class="more-hint">Wczytuję więcej odjazdów…</p>' : '';
  return `<article class="pole" id="pole-${esc(stop.id)}">
    <button class="pole-head" type="button" data-act="pole" data-id="${esc(stop.id)}" aria-expanded="${open}">
      <span class="code">${esc(stop.code ?? '—')}</span>
      <span><span class="pole-name">${esc(stop.name)}</span><span class="pole-meta">${esc(meta)}</span></span>
      ${ICON.chevron}
    </button>
    <div class="routes">${stop.routes.map((r) => fmt.badge(r, 'badge--sm')).join('')}</div>
    ${list}${loading}
  </article>`;
}

function renderDeparture(d) {
  const delay = fmt.delayInfo(d.delay);
  const sub = [
    d.live ? '<span class="live-dot" title="na żywo"></span>' : '',
    delay ? `<span class="delay ${delay.cls}">${delay.text}</span>` : '',
    delay && delay.cls !== 'ok' ? `<span class="sched" title="według rozkładu">${fmt.hhmm(d.scheduled)}</span>` : '',
    d.stops_away != null ? `<span>${fmt.stopsAway(d.stops_away)}</span>` : '',
  ].join('');
  return `<li class="dep">
    ${fmt.badge(d.route)}
    <span class="dep-main"><span class="headsign">${esc(d.headsign)}</span><span class="dep-sub">${sub}</span></span>
    ${when(d.expected, d.approximate)}
  </li>`;
}

const when = (t, approximate) =>
  `<span class="when" data-t="${t}"${approximate ? ' data-approx' : ''}></span>`;

function renderPlan(kind) {
  const place = PLACES[kind];
  const dest = state.dests[kind];
  if (!dest) {
    return block(ICON[kind], `Dojazd ${place.to}`,
      `Ustaw, gdzie jest ${place.what}, a podpowiem, na który przystanek iść, żeby dojechać najszybciej.`,
      [`<button class="btn btn--primary" type="button" data-act="dest" data-kind="${kind}">${place.set}</button>`]);
  }
  const bar = `<div class="dest-bar">
    <p>Dojazd ${place.to}: <b>${esc(dest.label)}</b></p>
    <button class="btn btn--small" type="button" data-act="dest" data-kind="${kind}">Zmień</button>
  </div>`;
  if (!state.pos) return bar + renderNoPosition();
  const plan = state.plans[kind];
  if (!plan || plan.to !== destKey(dest)) return bar + (state.problem ? renderProblem() : skeleton(2));

  const { options, walk } = plan.data;
  if (arrived(plan.data)) {
    return bar + block(ICON[kind], 'Jesteś na miejscu', `To już tutaj, nie trzeba nigdzie jechać.`);
  }
  if (!options.length && !walk) {
    return bar + banner() + block(ICON.alert, 'Brak połączenia',
      'W najbliższych godzinach nic stąd tam nie dojeżdża. Spróbuj później albo z innego miejsca.');
  }
  const now = api.now();
  const walkFirst = walk && (!options.length || now + walk.seconds <= options[0].arrive);
  const [best, ...rest] = options;
  let html = bar + banner();
  if (walkFirst) html += renderWalkBest(walk, kind);
  if (best) html += renderBest(best, kind, walkFirst ? 'Komunikacją najszybciej' : `Najszybciej ${place.to}`);
  if (rest.length) {
    html += `<h3 class="section-title">Inne przystanki</h3><ul class="options">${rest
      .map((option, i) => renderOption(option, i + 1, kind, best))
      .join('')}</ul>`;
  }
  if (walk && !walkFirst) html += renderWalk(walk);
  return html;
}

/** Whether a plan's destination is less than a minute's walk away. */
const arrived = (data) => data.walk != null && data.walk.distance < 60;

function renderBest(option, kind, eyebrow) {
  const start = option.start;
  const ride = option.legs.find((leg) => leg.kind === 'ride');
  const meta = [start.street, fmt.distance(start.distance), `${fmt.minutes(start.walk)} pieszo`]
    .filter(Boolean)
    .join(' · ');
  const delay = ride && fmt.delayInfo(ride.delay);
  const live = ride?.live
    ? ` · <span class="live-dot"></span> ${delay ? `<span class="delay ${delay.cls}">${delay.text}</span>` : ''}`
    : '';
  return `<article class="best">
    <p class="eyebrow">${esc(eyebrow)}</p>
    <h2 class="best-title"><span class="code">${esc(start.code ?? '—')}</span>
      <span><small>Idź na przystanek</small>${esc(start.name)}</span></h2>
    <p class="best-street">${esc(meta)}</p>
    <div class="facts-row">
      <div class="fact"><span class="k">Wyjdź</span><span class="v" data-leave="${option.leave}"></span></div>
      <div class="fact"><span class="k">Odjazd${ride ? fmt.badge(ride.route, 'badge--xs') : ''}</span><span class="v">${ride ? `${ride.approximate ? '~' : ''}${fmt.hhmm(ride.departure)}` : '—'}</span></div>
      <div class="fact"><span class="k">Na miejscu</span><span class="v">${fmt.hhmm(option.arrive)}</span></div>
    </div>
    <p class="trip-sum">${fmt.transfers(option.transfers)} · w drodze ${fmt.minutes(option.arrive - option.leave)}${live}</p>
    <ol class="legs">${option.legs.map(renderLeg).join('')}</ol>
  </article>`;
}

function renderOption(option, i, kind, best) {
  const start = option.start;
  const open = state.open[kind] === i;
  const lines = option.legs
    .filter((leg) => leg.kind === 'ride')
    .map((leg) => fmt.badge(leg.route, 'badge--sm'))
    .join('');
  const later = Math.round((option.arrive - best.arrive) / 60);
  return `<li class="opt">
    <button class="opt-row" type="button" data-act="option" data-kind="${kind}" data-i="${i}" aria-expanded="${open}">
      <span class="code code--sm">${esc(start.code ?? '—')}</span>
      <span>
        <span class="name">${esc(start.name)}</span>
        <span class="sub"><span class="lines">${lines}</span>
          <span>${fmt.minutes(start.walk)} pieszo · ${fmt.transfers(option.transfers)}</span></span>
      </span>
      <span class="opt-arrive"><span class="v">${fmt.hhmm(option.arrive)}</span>
        <span class="k">${later > 0 ? `+${later} min` : 'tak samo'}</span></span>
    </button>
    ${open ? `<ol class="legs">${option.legs.map(renderLeg).join('')}</ol>` : ''}
  </li>`;
}

function renderLeg(leg) {
  if (leg.kind === 'walk') {
    const how = `${fmt.distance(leg.distance)} (${fmt.minutes(leg.seconds)})`;
    let text;
    if (!leg.from) text = `Idź ${how} na przystanek <b>${esc(fmt.poleName(leg.to))}</b>`;
    else if (!leg.to) text = `Idź ${how} do celu`;
    else if (leg.distance < 15) text = `Przesiadka na tym samym przystanku, <b>${esc(fmt.poleName(leg.to))}</b>`;
    else text = `Przesiadka: przejdź ${how} na <b>${esc(fmt.poleName(leg.to))}</b>`;
    return `<li class="leg leg--walk"><span class="leg-icon">${ICON.walk}</span><div class="leg-body">${text}</div></li>`;
  }
  const delay = fmt.delayInfo(leg.delay);
  return `<li class="leg leg--ride" style="${fmt.lineStyle(leg.route)}">
    <span class="leg-icon">${fmt.badge(leg.route)}</span>
    <div class="leg-body">
      <div class="leg-head">→ ${esc(leg.headsign)}${leg.live ? ' <span class="live-dot" title="na żywo"></span>' : ''}${delay ? ` <span class="delay ${delay.cls}">${delay.text}</span>` : ''}</div>
      <div class="leg-stop"><time>${leg.approximate ? '~' : ''}${fmt.hhmm(leg.departure)}</time><span>${esc(fmt.poleName(leg.from))}</span></div>
      <div class="leg-mid">${leg.stops} przyst.</div>
      <div class="leg-stop"><time>${fmt.hhmm(leg.arrival)}</time><span>${esc(fmt.poleName(leg.to))}</span></div>
    </div>
  </li>`;
}

function renderWalkBest(walk, kind) {
  return `<article class="best">
    <p class="eyebrow">Najszybciej ${PLACES[kind].to}</p>
    <h2 class="best-title"><span class="code">${ICON.walk}</span><span><small>Idź pieszo</small>${fmt.distance(walk.distance)}</span></h2>
    <div class="facts-row">
      <div class="fact"><span class="k">Wyjdź</span><span class="v">teraz</span></div>
      <div class="fact"><span class="k">Pieszo</span><span class="v">${fmt.minutes(walk.seconds)}</span></div>
      <div class="fact"><span class="k">Na miejscu</span><span class="v">${fmt.hhmm(api.now() + walk.seconds)}</span></div>
    </div>
  </article>`;
}

const renderWalk = (walk) => `<div class="walk-all">${ICON.walk}<span>Pieszo całą drogę: <b>${fmt.minutes(walk.seconds)}</b>
  · ${fmt.distance(walk.distance)} · na miejscu ${fmt.hhmm(api.now() + walk.seconds)}</span></div>`;

function renderNoPosition() {
  if (state.geo === 'denied') {
    return block(ICON.pinOff, 'Brak dostępu do lokalizacji',
      'Zezwól stronie na lokalizację w ustawieniach przeglądarki albo wskaż na mapie, gdzie jesteś.', [pickButton()]);
  }
  if (state.geo === 'unavailable') {
    return block(ICON.pin, 'Nie mogę ustalić pozycji',
      'Sygnał GPS jest za słaby. Poczekaj chwilę albo wskaż na mapie, gdzie jesteś.', [pickButton()]);
  }
  return block('<div class="spinner"></div>', 'Ustalam Twoją pozycję…',
    'Pierwszy odczyt GPS może potrwać kilka sekund.', [pickButton()]);
}

function renderProblem() {
  if (state.problem === 'loading') {
    return block('<div class="spinner"></div>', 'Ładuję rozkład…',
      'Serwer właśnie wczytuje rozkład jazdy. Spróbuję ponownie za kilka sekund.');
  }
  if (state.problem === 'offline') {
    return block(ICON.offline, 'Brak połączenia',
      'Sprawdź internet. Odjazdy pokażę, gdy tylko wróci zasięg.', [retryButton()]);
  }
  const detail = state.problemText ? `<br><small>(${esc(state.problemText)})</small>` : '';
  return block(ICON.alert, 'Coś poszło nie tak',
    `Serwer odpowiedział błędem. Spróbuję ponownie za chwilę.${detail}`, [retryButton()]);
}

/** A strip over data that could not be refreshed, saying how old it is. */
function banner() {
  if (!state.problem || !state.fetched) return '';
  const age = fmt.hhmm(state.fetched.at / 1000);
  if (state.problem === 'loading') {
    return `<div class="banner banner--info">${ICON.alert}<span>Serwer odświeża rozkład. Dane z ${age}.</span></div>`;
  }
  const why = state.problem === 'offline' ? 'Brak połączenia' : 'Nie udało się odświeżyć';
  return `<div class="banner">${ICON.offline}<span>${why}. Odjazdy z ${age} mogą być nieaktualne.</span></div>`;
}

function block(icon, title, text, actions = []) {
  const buttons = actions.filter(Boolean).join('');
  return `<div class="state">${icon}<h2>${title}</h2><p>${text}</p>${buttons ? `<div class="actions">${buttons}</div>` : ''}</div>`;
}

const pickButton = () =>
  map ? '<button class="btn" type="button" data-act="pick-pos">Wskaż na mapie</button>' : '';

const retryButton = () => '<button class="btn" type="button" data-act="retry">Spróbuj ponownie</button>';

const skeleton = (n) => '<div class="skel"></div>'.repeat(n);

function renderMap() {
  if (!map) return;
  map.setUser(state.pos);
  map.setPlaces(state.dests);
  let fitTo;
  let key;
  if (state.view === 'nearby') {
    const stops = state.nearby?.stops ?? [];
    map.setVehicles(state.vehicles);
    map.setJourney(null);
    map.setPoles(stops);
    fitTo = [state.pos, ...stops.slice(0, 6)];
    key = `nearby:${stops.length > 0}`;
  } else {
    const dest = state.dests[state.view];
    const plan = state.plans[state.view];
    const current = plan && dest && plan.to === destKey(dest) && !arrived(plan.data);
    const options = current ? plan.data.options : [];
    const index = state.open[state.view] ?? 0;
    const chosen = options[index];
    // Only the vehicles you could ride: every bus around would hide the way there.
    const rides = chosen?.legs.filter((leg) => leg.kind === 'ride') ?? [];
    map.setVehicles(state.vehicles.filter((v) =>
      rides.some((leg) => leg.route.name === v.route?.name && leg.headsign === v.headsign)));
    map.setPoles(options.map((option) => option.start), chosen?.start.id);
    map.setJourney(chosen, state.pos, dest);
    fitTo = [state.pos, dest, ...(chosen?.legs.flatMap((leg) => [leg.from, leg.to]) ?? [])];
    key = `${state.view}:${dest ? destKey(dest) : ''}:${chosen ? index : '-'}`;
  }
  // Re-fit only when what is shown changes, not on every refresh while you pan around.
  const posKey = state.pos ? `${state.pos.lat.toFixed(3)},${state.pos.lon.toFixed(3)}` : '';
  if (`${key}@${posKey}` !== state.fitted && fitTo.some(Boolean)) {
    state.fitted = `${key}@${posKey}`;
    map.fit(fitTo);
  }
}

/** Updates countdowns every second between fetches. */
function tick() {
  const now = api.now();
  for (const el of content.querySelectorAll('[data-t]')) {
    const t = Number(el.dataset.t);
    const text = fmt.countdown(t, now);
    el.textContent = el.hasAttribute('data-approx') && text !== 'teraz' ? `~${text}` : text;
    el.classList.toggle('now', t - now < 60);
    el.closest('.dep')?.classList.toggle('gone', t < now - 30);
  }
  for (const el of content.querySelectorAll('[data-leave]')) {
    [el.previousElementSibling.textContent, el.textContent] = fmt.leaveIn(Number(el.dataset.leave), now);
  }
  content.classList.toggle('stale', Boolean(state.fetched) && Date.now() - state.fetched.at > STALE_MS);
}

/* Keeping keyboard focus across re-renders */

function focusKey() {
  const el = document.activeElement;
  return content.contains(el) && el.dataset.act ? [el.dataset.act, el.dataset.id, el.dataset.i] : null;
}

function restoreFocus(key) {
  if (!key) return;
  const [act, id, i] = key;
  const match = [...content.querySelectorAll(`[data-act="${act}"]`)].find(
    (el) => el.dataset.id === id && el.dataset.i === i,
  );
  match?.focus({ preventScroll: true });
}

/* Interaction */

function setView(view) {
  history.replaceState(null, '', location.pathname + location.search + HASHES[view]);
  state.view = view;
  render();
  refresh();
}

function viewFromHash() {
  if (location.hash === '#dom') return 'home';
  if (location.hash === '#praca') return 'work';
  return 'nearby';
}

function togglePole(id) {
  if (state.expanded.delete(id)) {
    state.more.delete(id);
    render();
    return;
  }
  state.expanded.add(id);
  render();
  loadMore(id)
    .catch(() => state.more.set(id, state.nearby?.stops.find((s) => s.id === id)?.departures ?? []))
    .then(render);
}

function toggleOption(kind, i) {
  state.open[kind] = state.open[kind] === i ? 0 : i;
  render();
}

function showPole(id) {
  if (state.view !== 'nearby') {
    const options = state.plans[state.view]?.data.options ?? [];
    const i = options.findIndex((option) => option.start.id === id);
    if (i >= 0) toggleOption(state.view, i);
    return;
  }
  const card = document.getElementById(`pole-${id}`);
  if (!card) return;
  card.scrollIntoView({ behavior: 'smooth', block: 'center' });
  card.classList.remove('flash');
  void card.offsetWidth;
  card.classList.add('flash');
}

function startPicking(what) {
  if (!map) return;
  state.picking = what;
  const text = what === 'pos' ? 'Dotknij mapy tam, gdzie jesteś' : `Dotknij mapy tam, gdzie jest ${PLACES[what].what}`;
  $('#pick-text').textContent = text;
  $('#pick').hidden = false;
  $('#map-wrap').classList.add('picking');
  setTall(true);
  scrollTo({ top: 0, behavior: 'smooth' });
}

function stopPicking() {
  state.picking = null;
  $('#pick').hidden = true;
  $('#map-wrap').classList.remove('picking');
  setTall(false);
}

function tapMap(lat, lon) {
  const what = state.picking ?? (state.pos ? null : 'pos');
  if (!what) return;
  stopPicking();
  if (what === 'pos') setPosition({ lat, lon, acc: null, source: 'map' });
  else setDest(what, { lat, lon, label: 'punkt na mapie' }, { nameIt: true });
}

function setTall(on) {
  $('#map-wrap').classList.toggle('tall', on);
  const button = $('#map-size');
  button.setAttribute('aria-expanded', String(on));
  button.setAttribute('aria-label', on ? 'Zmniejsz mapę' : 'Powiększ mapę');
  button.title = button.getAttribute('aria-label');
}

content.addEventListener('click', (event) => {
  const el = event.target.closest('[data-act]');
  if (!el) return;
  const { act, id, kind, i } = el.dataset;
  if (act === 'pole') togglePole(id);
  else if (act === 'option') toggleOption(kind, Number(i));
  else if (act === 'dest') openDestSheet(kind);
  else if (act === 'pick-pos') startPicking('pos');
  else if (act === 'retry') refresh();
  else if (act === 'wider') {
    state.radius = 1500;
    state.nearby = null;
    render();
    refresh();
  }
});

for (const tab of document.querySelectorAll('.tabs [data-view]')) {
  tab.addEventListener('click', () => setView(tab.dataset.view));
}

$('#gps').addEventListener('click', () => {
  if (state.pos?.source === 'map' && lastFix) setPosition(lastFix);
  else startPicking('pos');
});

$('#pick-cancel').addEventListener('click', stopPicking);
$('#map-size').addEventListener('click', () => setTall(!$('#map-wrap').classList.contains('tall')));
$('#map-me').addEventListener('click', () => state.pos && map?.focus(state.pos));
$('#about-open').addEventListener('click', () => $('#about-sheet').showModal());

/* The Dom / Praca sheet */

const sheet = $('#dest-sheet');
let sheetKind = 'home';
let found = [];
let searchTimer = 0;

function openDestSheet(kind) {
  sheetKind = kind;
  const dest = state.dests[kind];
  $('#dest-title').textContent = PLACES[kind].where;
  $('#dest-current').hidden = !dest;
  $('#dest-current').textContent = dest ? `Teraz: ${dest.label}` : '';
  sheet.querySelector('[data-act="remove"]').hidden = !dest;
  sheet.querySelector('[data-act="here"]').disabled = !state.pos;
  $('#dest-q').value = '';
  $('#dest-results').innerHTML = '';
  found = [];
  sheet.showModal();
}

sheet.addEventListener('click', (event) => {
  if (event.target === sheet) {
    sheet.close();
    return;
  }
  const el = event.target.closest('[data-act]');
  if (!el) return;
  const kind = sheetKind;
  if (el.dataset.act === 'here' && state.pos) {
    // Not from state.nearby: in the Dom and Praca tabs it can be from where you were hours ago.
    sheet.close();
    setDest(kind, { lat: state.pos.lat, lon: state.pos.lon, label: 'moja lokalizacja' }, { nameIt: true });
  } else if (el.dataset.act === 'map') {
    sheet.close();
    startPicking(kind);
  } else if (el.dataset.act === 'remove') {
    delete state.dests[kind];
    delete state.plans[kind];
    saveDests();
    sheet.close();
    render();
  } else if (el.dataset.act === 'stop') {
    const group = found[Number(el.dataset.i)];
    sheet.close();
    setDest(kind, { lat: group.lat, lon: group.lon, label: group.name });
  }
});

// Enter in the search box picks the first stop found. Left alone, it would press the
// form's first button, which closes the sheet.
$('#dest-q').addEventListener('keydown', (event) => {
  if (event.key !== 'Enter' || event.isComposing) return;
  event.preventDefault();
  sheet.querySelector('[data-act="stop"]')?.click();
});

$('#dest-q').addEventListener('input', (event) => {
  clearTimeout(searchTimer);
  const q = event.target.value.trim();
  if (q.length < 2) {
    $('#dest-results').innerHTML = '';
    return;
  }
  searchTimer = setTimeout(async () => {
    let html;
    let stops = [];
    try {
      ({ stops } = await api.get('/api/stops', { q }));
      html = stops.length
        ? stops.map((group, i) => `<li><button type="button" data-act="stop" data-i="${i}">
            <span>${esc(group.name)}</span>
            <span class="codes">${group.poles.map((pole) => esc(pole.code ?? '')).filter(Boolean).slice(0, 8).join(' ')}</span>
          </button></li>`).join('')
        : '<li class="empty">Nie znam takiego przystanku.</li>';
    } catch {
      html = '<li class="empty">Nie udało się wyszukać. Sprawdź połączenie.</li>';
    }
    // An answer to an older query, overtaken by a newer one.
    if ($('#dest-q').value.trim() !== q) return;
    found = stops;
    $('#dest-results').innerHTML = html;
  }, 250);
});

$('#about-sheet').addEventListener('click', (event) => {
  if (event.target === event.currentTarget) event.currentTarget.close();
});

/* Start */

addEventListener('hashchange', () => {
  state.view = viewFromHash();
  render();
  refresh();
});

document.addEventListener('visibilitychange', () => {
  if (!document.hidden) refresh();
});

addEventListener('online', refresh);
addEventListener('offline', () => {
  state.problem = 'offline';
  render();
});

const at = parseAt(new URLSearchParams(location.search).get('at'));
if (at) setPosition({ ...at, acc: null, source: 'url' });
else startGps();
render();
loadStatus();
setInterval(() => {
  if (!document.hidden) tick();
}, 1000);

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(() => {});
}
