// Text and markup helpers. Times are milliseconds since 1970, always shown in Warsaw time.

const ZONE = 'Europe/Warsaw';

const clock = new Intl.DateTimeFormat('en-GB', {
  hour: '2-digit',
  minute: '2-digit',
  timeZone: ZONE,
});

const dayOnly = new Intl.DateTimeFormat('en-GB', {
  day: 'numeric',
  month: 'short',
  year: 'numeric',
  timeZone: ZONE,
});

const weekday = new Intl.DateTimeFormat('en-GB', {
  weekday: 'long',
  day: 'numeric',
  month: 'long',
  timeZone: ZONE,
});

export const hhmm = (ms) => clock.format(ms);

/** "Saturday 31 October" */
export const day = (ms) => weekday.format(ms);

const noon = (day) => new Date(`${day}T12:00:00Z`);

/** "30 Oct 2026" for a `YYYY-MM-DD` day. */
export const date = (day) => dayOnly.format(noon(day));

/** "29–31 Oct 2026" */
export const dates = (first, last) => dayOnly.formatRange(noon(first), noon(last));

/** "now", "5 min", or nothing an hour or more away, as the command-line board shows it. */
export function countdown(ms, now) {
  const minutes = Math.trunc((ms - now) / 60_000);
  if (minutes === 0) return 'now';
  if (minutes > 0 && minutes < 60) return `${minutes} min`;
  return '';
}

/** Metres, rounded the way people say them. */
export function distance(m) {
  if (m < 100) return `${Math.round(m)} m`;
  if (m < 1000) return `${Math.round(m / 10) * 10} m`;
  return `${(m / 1000).toFixed(1)} km`;
}

/** Browsers time to a tenth of a millisecond at best. */
export function milliseconds(ms) {
  if (ms < 0.1) return 'under 0.1 ms';
  if (ms < 10) return `${ms.toFixed(1)} ms`;
  return `${Math.round(ms)} ms`;
}

export const megabytes = (bytes) => `${(bytes / 1e6).toFixed(2)} MB`;

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

export const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ESCAPES[c]);

/** Warsaw's colours, for lines the feed gives none. */
const FALLBACK = {
  metro: ['#0000bb', '#ffffff'],
  tram: ['#b60000', '#ffffff'],
  rail: ['#006b3c', '#ffffff'],
  bus: ['#880077', '#ffffff'],
  other: ['#59636e', '#ffffff'],
};

const HEX = /^#[0-9a-f]{6}$/i;

/** A line's badge in its own colours: red trams, purple buses, the blue metro. */
export function badge(line) {
  const [bg, fg] = FALLBACK[line.kind] ?? FALLBACK.other;
  const colors = `--b-bg:${HEX.test(line.color) ? line.color : bg};--b-fg:${HEX.test(line.text) ? line.text : fg}`;
  return `<span class="badge badge--${esc(line.kind)}" style="${colors}">${esc(line.name)}</span>`;
}

const isoDay = new Intl.DateTimeFormat('en-CA', {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  timeZone: ZONE,
});

/** The `YYYY-MM-DD` day it is in Warsaw at `ms`. */
export const warsawDay = (ms) => isoDay.format(ms);
