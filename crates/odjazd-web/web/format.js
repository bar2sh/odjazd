// Text and markup helpers. Times are Unix seconds, always shown in Warsaw time.

const clock = new Intl.DateTimeFormat('pl-PL', {
  hour: '2-digit',
  minute: '2-digit',
  timeZone: 'Europe/Warsaw',
});

const stamp = new Intl.DateTimeFormat('pl-PL', {
  dateStyle: 'short',
  timeStyle: 'medium',
  timeZone: 'Europe/Warsaw',
});

export const hhmm = (t) => clock.format(new Date(t * 1000));

export const dateTime = (t) => stamp.format(new Date(t * 1000));

/** "teraz" under a minute, "3 min" within the hour, the clock time after that. */
export function countdown(t, now) {
  const s = t - now;
  if (s < 60) return 'teraz';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  return hhmm(t);
}

/** Label and value: "Wyjdź teraz", "Wyjdź za 4 min" or "Wyjdź o 18:20". Short values fit
 * three facts side by side on a narrow phone. */
export function leaveIn(t, now) {
  const s = t - now;
  if (s < 60) return ['Wyjdź', 'teraz'];
  if (s < 3600) return ['Wyjdź za', `${Math.floor(s / 60)} min`];
  return ['Wyjdź o', hhmm(t)];
}

/** Metres, to the metre under 100 m: the user tells poles 30 m apart. */
export function distance(m) {
  if (m < 100) return `${Math.round(m)} m`;
  if (m < 1000) return `${Math.round(m / 10) * 10} m`;
  return `${(m / 1000).toFixed(1).replace('.', ',')} km`;
}

export const minutes = (s) => `${Math.max(1, Math.round(s / 60))} min`;

/** Polish plural: 1 przesiadka, 2 przesiadki, 5 przesiadek, 22 przesiadki. */
export function plural(n, one, few, many) {
  if (n === 1) return one;
  const tens = n % 100;
  const units = n % 10;
  return units >= 2 && units <= 4 && (tens < 12 || tens > 14) ? few : many;
}

export const transfers = (n) =>
  n === 0 ? 'bez przesiadek' : `${n} ${plural(n, 'przesiadka', 'przesiadki', 'przesiadek')}`;

/** Delay badge text and colour class, or null without live data. */
export function delayInfo(delay) {
  if (delay == null) return null;
  const m = Math.round(delay / 60);
  if (m === 0) return { text: 'o czasie', cls: 'ok' };
  if (m > 0) return { text: `+${m} min`, cls: m >= 5 ? 'very-late' : 'late' };
  return { text: `−${-m} min`, cls: 'early' };
}

export function stopsAway(n) {
  if (n == null) return '';
  return n === 0 ? 'dojeżdża' : `za ${n} przyst.`;
}

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

export const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ESCAPES[c]);

const KINDS = new Set(['tram', 'bus', 'metro', 'rail']);
const HEX = /^#[0-9a-f]{6}$/i;
const FALLBACK = {
  tram: ['#b60000', '#ffffff'],
  bus: ['#880077', '#ffffff'],
  metro: ['#0000bb', '#ffffff'],
  rail: ['#006b3c', '#ffffff'],
};

export const kindOf = (route) => (KINDS.has(route?.kind) ? route.kind : 'bus');

/** Background and text colour of a line, safe to put in a style attribute. */
export function lineColors(route) {
  const [bg, fg] = FALLBACK[kindOf(route)];
  return [HEX.test(route?.color) ? route.color : bg, HEX.test(route?.text) ? route.text : fg];
}

export const lineStyle = (route) => {
  const [bg, fg] = lineColors(route);
  return `--b-bg:${bg};--b-fg:${fg}`;
};

export const badge = (route, extra = '') =>
  `<span class="badge badge--${kindOf(route)} ${extra}" style="${lineStyle(route)}">${esc(route?.name)}</span>`;

/** "Metro Świętokrzyska 03": the name riders read on the pole. */
export const poleName = (stop) => (stop?.code ? `${stop.name} ${stop.code}` : stop?.name ?? '');

/** Metres between two points; plenty accurate within a city. */
export function metres(a, b) {
  const rad = Math.PI / 180;
  const x = (b.lon - a.lon) * rad * Math.cos(((a.lat + b.lat) / 2) * rad);
  const y = (b.lat - a.lat) * rad;
  return 6371000 * Math.hypot(x, y);
}
