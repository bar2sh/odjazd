// The page: odjazd-core, compiled to WebAssembly, loads the timetable once and then answers
// every search and departure board in the browser. There is no server behind it.

import init, { Timetable } from './pkg/odjazd_wasm.js?v=__CODE__';
import {
  badge,
  countdown,
  date,
  dates,
  day,
  distance,
  esc,
  hhmm,
  megabytes,
  milliseconds,
  warsawDay,
} from './format.js?v=__CODE__';

const DATA = './timetable.br?v=__DATA__';
const WASM = new URL('./pkg/odjazd_wasm_bg.wasm?v=__CODE__', import.meta.url);

/** Departures on a board, and stops listed under the search box. */
const BOARD = 12;
const FOUND = 8;
/** Line badges shown for a stop in the list; the rest are counted. */
const BADGES = 8;
/** How far "Stops near me" looks, in metres. */
const NEAR = 800;
/** How often an open board moves on with the clock. */
const REFRESH_MS = 15_000;
const TITLE = document.title;

const $ = (id) => document.getElementById(id);
const el = {
  finder: document.querySelector('.finder'),
  form: document.querySelector('.search'),
  query: $('query'),
  locate: $('locate'),
  results: $('results'),
  map: $('map'),
  board: $('board'),
  name: $('board-name'),
  lines: $('board-lines'),
  at: $('at'),
  atDay: $('at-day'),
  now: $('now'),
  departures: $('departures'),
  intro: $('intro'),
  status: $('status'),
  stats: $('stats'),
};

/** The WebAssembly timetable, once loaded. */
let timetable = null;
/** The map, if this browser can show one. */
let map = null;
/** The stop group on the board. */
let shown = null;
/** The moment picked under "Leaving", or null to follow the clock. */
let leaving = null;
/** Stops listed under the search box, and the one the arrow keys are on. */
let options = [];
let active = -1;
/** What "How this page works" reports. */
const facts = {};

// The map loads beside the timetable, and the page works without it.
const mapReady = import('./map.js?v=__CODE__')
  .then(({ createMap }) => {
    map = createMap(el.map, { onPick: (id) => open(timetable?.group(id)) });
  })
  .catch((error) => {
    console.warn('No map:', error);
    el.map.hidden = true;
    document.body.classList.add('no-map');
  });

load();

async function load() {
  if (typeof WebAssembly !== 'object') {
    status('This browser cannot run WebAssembly, which this page needs.', 'error');
    return;
  }
  status('Loading the timetable…');
  try {
    const started = performance.now();
    const [bytes] = await Promise.all([download(), init({ module_or_path: WASM })]);
    facts.size = bytes.length;
    facts.download = performance.now() - started;
    const read = performance.now();
    timetable = Timetable.fromBrotli(bytes);
    facts.read = performance.now() - read;
  } catch (error) {
    console.error(error);
    fail(error);
    return;
  }
  ready();
}

/** Downloads the timetable, showing how far it got. */
async function download() {
  const response = await fetch(DATA);
  if (!response.ok) throw new Error(`the server answered ${response.status}`);
  const total = Number(response.headers.get('Content-Length')) || 0;
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    size += value.length;
    const of = total >= size ? ` of ${megabytes(total)}` : '';
    status(`Loading the timetable… ${megabytes(size)}${of}`);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  return bytes;
}

function ready() {
  const poles = timetable.poles();
  Object.assign(facts, {
    poles: poles.length,
    groups: timetable.groupCount,
    lines: timetable.lineCount,
    days: timetable.days(),
    version: timetable.version,
  });
  el.query.disabled = false;
  el.locate.disabled = false;
  el.query.placeholder = `Search ${facts.groups.toLocaleString('en')} stops`;
  mapReady.then(() => {
    map?.setPoles(poles);
    if (shown) map?.select(shown);
  });
  fromAddress();
  checkDays();
  stats();
  setInterval(tick, REFRESH_MS);
  document.addEventListener('visibilitychange', tick);
}

function tick() {
  if (document.hidden) return;
  board();
  checkDays();
}

/** Says which day the timetable is for, and warns when today is not in it. */
function checkDays() {
  const [first, middle, last] = facts.days;
  const today = warsawDay(Date.now());
  if (today < first || today > last) {
    status(`This copy of the timetable covers ${dates(first, last)} only. Reload the page for today's.`, 'warn');
  } else {
    status(`Timetable of ${date(middle)}${facts.version ? `, feed version ${facts.version}` : ''}.`);
  }
}

function status(text, kind = '') {
  el.status.className = kind ? `status status--${kind}` : 'status';
  el.status.textContent = text;
}

function fail(error) {
  const why = navigator.onLine === false ? 'you seem to be offline' : error.message;
  status(`The timetable did not load: ${why}. `, 'error');
  const retry = document.createElement('button');
  retry.type = 'button';
  retry.className = 'text-button';
  retry.textContent = 'Try again';
  retry.addEventListener('click', load, { once: true });
  el.status.append(retry);
  el.query.placeholder = 'The timetable did not load';
}

/** Runs `work` and notes how long odjazd-core took for "How this page works". */
function timed(name, work) {
  const start = performance.now();
  const result = work();
  facts[name] = performance.now() - start;
  stats();
  return result;
}

function stats() {
  const { days } = facts;
  const rows = [
    ['Download', `${megabytes(facts.size)} in ${milliseconds(facts.download)}`],
    ['Unpacked, checked and loaded in', milliseconds(facts.read)],
    ['Stops', `${facts.groups.toLocaleString('en')}, with ${facts.poles.toLocaleString('en')} poles`],
    ['Lines', facts.lines.toLocaleString('en')],
    ['Service days', dates(days[0], days[2])],
    ['Last search', facts.search == null ? 'none yet' : milliseconds(facts.search)],
    ['Last board', facts.board == null ? 'none yet' : milliseconds(facts.board)],
  ];
  el.stats.innerHTML = rows.map(([term, value]) => `<dt>${term}</dt><dd>${esc(value)}</dd>`).join('');
}

// Search

el.form.addEventListener('submit', (event) => event.preventDefault());

el.query.addEventListener('input', () => {
  const query = el.query.value.trim();
  if (!query) {
    close();
    return;
  }
  options = timed('search', () => timetable.search(query, FOUND));
  list(options.length ? null : `No stop matches “${query}”.`);
});

el.query.addEventListener('keydown', (event) => {
  const listed = !el.results.hidden && options.length > 0;
  if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && listed) {
    event.preventDefault();
    const step = event.key === 'ArrowDown' ? 1 : -1;
    const next = active < 0 ? (step > 0 ? 0 : options.length - 1) : (active + step + options.length) % options.length;
    highlight(next);
  } else if (event.key === 'Enter') {
    event.preventDefault();
    if (listed) choose(options[Math.max(active, 0)]);
  } else if (event.key === 'Escape' && !el.results.hidden) {
    event.preventDefault();
    close();
  }
});

// Clicking an option must not take the focus off the box first.
el.results.addEventListener('mousedown', (event) => event.preventDefault());
el.results.addEventListener('click', (event) => {
  const option = event.target.closest('[data-i]');
  if (option) choose(options[option.dataset.i]);
});

document.addEventListener('pointerdown', (event) => {
  if (!el.finder.contains(event.target)) close();
});
el.finder.addEventListener('focusout', (event) => {
  if (!el.finder.contains(event.relatedTarget)) close();
});

el.locate.addEventListener('click', () => {
  if (!navigator.geolocation) {
    options = [];
    list('This browser cannot tell where you are.');
    return;
  }
  el.locate.setAttribute('aria-busy', 'true');
  navigator.geolocation.getCurrentPosition(
    ({ coords }) => {
      el.locate.removeAttribute('aria-busy');
      const { latitude: lat, longitude: lon } = coords;
      options = timed('search', () => timetable.nearby(lat, lon, NEAR, FOUND));
      map?.showYou(lat, lon);
      if (options.length) {
        map?.fit([{ lat, lon }, ...options]);
        list(null, 'Near you');
      } else {
        list(`No stops within ${distance(NEAR)} of you. odjazd knows Warsaw's public transport only.`);
      }
    },
    (error) => {
      el.locate.removeAttribute('aria-busy');
      options = [];
      list(
        error.code === error.PERMISSION_DENIED
          ? 'This page may not see your location. You can allow it in the browser.'
          : 'Your location is not available right now.',
      );
    },
    { enableHighAccuracy: true, timeout: 15_000, maximumAge: 60_000 },
  );
});

/** Shows `options` under the search box, with a heading or a message. */
function list(message = null, heading = null) {
  active = -1;
  const items = options.map(
    (group, i) => `
      <li id="found-${i}" role="option" aria-selected="false" data-i="${i}">
        <span class="found-name">${esc(group.name)}</span>
        ${group.distance == null ? '' : `<span class="found-far">${distance(group.distance)}</span>`}
        <span class="found-lines">${badges(group.lines, BADGES)}</span>
      </li>`,
  );
  if (heading) items.unshift(`<li class="results-note" role="presentation">${esc(heading)}</li>`);
  if (message) items.push(`<li class="results-note" role="presentation">${esc(message)}</li>`);
  el.results.innerHTML = items.join('');
  el.results.hidden = false;
  el.query.setAttribute('aria-expanded', 'true');
  el.query.removeAttribute('aria-activedescendant');
}

function highlight(i) {
  el.results.querySelector('[aria-selected="true"]')?.setAttribute('aria-selected', 'false');
  active = i;
  const option = $(`found-${i}`);
  option.setAttribute('aria-selected', 'true');
  option.scrollIntoView({ block: 'nearest' });
  el.query.setAttribute('aria-activedescendant', option.id);
}

function close() {
  el.results.hidden = true;
  el.query.setAttribute('aria-expanded', 'false');
  el.query.removeAttribute('aria-activedescendant');
}

function choose(group) {
  close();
  el.query.value = '';
  // Hides the keyboard on a phone.
  el.query.blur();
  open(group, { scroll: true });
}

function badges(lines, max = Infinity) {
  const shown = lines.slice(0, max).map(badge).join('');
  const more = lines.length - max;
  return more > 0 ? `${shown}<span class="more">+${more}</span>` : shown;
}

// The board, whose stop is in the address: #Pl.%20Narutowicza

/** Shows the board of `group` and puts it in the address, for bookmarks and Back. */
function open(group, { scroll = false } = {}) {
  if (!group) return;
  const hash = `#${encodeURIComponent(group.name)}`;
  if (location.hash !== hash) history.pushState(null, '', hash);
  show(group, scroll);
}

function fromAddress() {
  if (!timetable) return;
  let name = '';
  try {
    name = decodeURIComponent(location.hash.slice(1));
  } catch {
    // Not a stop name, so no board.
  }
  if (!name) {
    show(null);
    return;
  }
  if (shown?.name === name) return;
  const found = timetable.search(name, FOUND);
  const group = found.find((g) => g.name.toLowerCase() === name.toLowerCase()) ?? found[0];
  if (!group) {
    show(null);
    options = [];
    list(`No stop matches “${name}”.`);
    return;
  }
  // "#Dworzec Centralny" becomes "#Dw.%20Centralny", the stop it found.
  if (group.name !== name) history.replaceState(null, '', `#${encodeURIComponent(group.name)}`);
  show(group);
}

addEventListener('popstate', fromAddress);
addEventListener('hashchange', fromAddress);

function show(group, scroll = false) {
  shown = group;
  map?.select(group);
  el.board.hidden = !group;
  el.intro.hidden = Boolean(group);
  if (!group) {
    document.title = TITLE;
    return;
  }
  document.title = `${group.name} · odjazd`;
  el.name.textContent = group.name;
  el.lines.innerHTML = badges(group.lines);
  board();
  // On a phone the board is under the map.
  if (scroll && matchMedia('(max-width: 899px)').matches) {
    const smooth = !matchMedia('(prefers-reduced-motion: reduce)').matches;
    el.board.scrollIntoView({ behavior: smooth ? 'smooth' : 'auto', block: 'start' });
  }
}

function board() {
  if (!shown) return;
  const now = Date.now();
  const at = leaving ?? now;
  const departures = timed('board', () => timetable.departures(shown.id, at, BOARD));
  const rows = [];
  // Past midnight the times start again, so the board says which day they are on.
  let today = warsawDay(now);
  for (const departure of departures) {
    if (warsawDay(departure.time) !== today) {
      today = warsawDay(departure.time);
      rows.push(`<li class="dep-day">${day(departure.time)}</li>`);
    }
    rows.push(row(departure, now));
  }
  el.departures.innerHTML = rows.length
    ? rows.join('')
    : `<li class="empty">Nothing leaves here${leaving ? ` after ${hhmm(at)}` : ''} in this timetable.</li>`;
  if (leaving == null && document.activeElement !== el.at) el.at.value = hhmm(now);
  el.atDay.textContent = leaving != null && warsawDay(leaving) !== warsawDay(now) ? 'tomorrow' : '';
}

/** One departure, in the columns of the command-line board. */
function row(departure, now) {
  const { time, line, headsign, pole, approximate } = departure;
  const gone = now - time >= 60_000;
  const left = countdown(time, now);
  const estimate = approximate
    ? ' title="The timetable only says how often it runs, so this time is an estimate"'
    : '';
  return `
    <li class="dep${gone ? ' dep--gone' : ''}">
      ${badge(line)}
      <span class="dep-to">${esc(headsign)}</span>
      <span class="dep-pole">${pole ? `<span class="sr">pole </span>${esc(pole)}` : ''}</span>
      <time class="dep-time" datetime="${new Date(time).toISOString()}"${estimate}>${approximate ? '~' : ''}${hhmm(time)}</time>
      <span class="dep-in${left === 'now' ? ' dep-in--now' : ''}">${left}</span>
    </li>`;
}

el.at.addEventListener('change', () => {
  try {
    leaving = el.at.value ? timetable.leavingAt(Date.now(), el.at.value) : null;
  } catch {
    leaving = null;
  }
  el.now.hidden = leaving == null;
  board();
});

el.now.addEventListener('click', () => {
  leaving = null;
  el.now.hidden = true;
  board();
});
