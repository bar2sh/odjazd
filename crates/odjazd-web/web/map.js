// The Leaflet map: you, the poles around you, Dom and Praca, live vehicles and the journey
// picked in a plan. Without Leaflet (offline on first visit) `createMap` returns null.

import { esc, kindOf, lineColors, lineStyle, poleName } from './format.js';

// OpenStreetMap's own tiles need no key; style.css darkens them with a filter in dark mode.
const TILES = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png';
const CREDIT = '© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors';
const WARSAW = [52.2297, 21.0122];

export const PLACE_ICONS = {
  home: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 11l8-7 8 7"/><path d="M6 9.5V20h12V9.5"/></svg>',
  work: '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="7" width="18" height="13" rx="2"/><path d="M9 7V5h6v2M3 12.5h18"/></svg>',
};

const PLACE_NAMES = { home: 'Dom', work: 'Praca' };

const latLng = (p) => [p.lat, p.lon];

export function createMap(el, { onTap, onPole }) {
  const L = window.L;
  if (!L) return null;

  const map = L.map(el, { zoomControl: false, center: WARSAW, zoom: 13, maxZoom: 19 });
  map.attributionControl.setPrefix(false);
  L.tileLayer(TILES, { maxZoom: 19, attribution: CREDIT }).addTo(map);
  map.on('click', (e) => onTap(e.latlng.lat, e.latlng.lng));
  // The strip grows and shrinks (and is a side panel on wide screens): keep tiles covering it.
  new ResizeObserver(() => map.invalidateSize()).observe(el);

  const journey = L.layerGroup().addTo(map);
  const poles = L.layerGroup().addTo(map);
  const vehicles = L.layerGroup().addTo(map);
  const places = L.layerGroup().addTo(map);
  let me = null;
  let accuracy = null;

  return {
    setUser(pos) {
      if (!pos) {
        me?.remove();
        accuracy?.remove();
        me = accuracy = null;
        return;
      }
      if (!me) {
        accuracy = L.circle(latLng(pos), { radius: 0, className: 'acc', interactive: false }).addTo(map);
        const icon = L.divIcon({ className: 'me-icon', html: '<div class="me"></div>', iconSize: [18, 18] });
        me = L.marker(latLng(pos), { icon, zIndexOffset: 1000, interactive: false, keyboard: false }).addTo(map);
      }
      me.setLatLng(latLng(pos));
      accuracy.setLatLng(latLng(pos)).setRadius(pos.acc ?? 0);
    },

    setPoles(stops, highlight) {
      poles.clearLayers();
      for (const stop of stops) {
        const hl = stop.id === highlight;
        const cls = hl ? 'pm pm--hl' : highlight ? 'pm pm--dim' : 'pm';
        const icon = L.divIcon({ className: 'pm-icon', html: `<div class="${cls}">${esc(stop.code ?? '•')}</div>`, iconSize: [30, 22] });
        L.marker(latLng(stop), { icon, title: poleName(stop), zIndexOffset: hl ? 400 : 0, keyboard: false })
          .on('click', () => onPole(stop.id))
          .addTo(poles);
      }
    },

    setPlaces(dests) {
      places.clearLayers();
      for (const kind of ['home', 'work']) {
        const place = dests[kind];
        if (!place) continue;
        const html = `<div class="dm">${PLACE_ICONS[kind]}${PLACE_NAMES[kind]}</div>`;
        const icon = L.divIcon({ className: 'dm-icon', html, iconSize: [0, 0] });
        L.marker(latLng(place), { icon, zIndexOffset: 900, interactive: false, keyboard: false }).addTo(places);
      }
    },

    setVehicles(list) {
      vehicles.clearLayers();
      for (const v of list) {
        const turn = Number.isFinite(v.bearing) ? `<div class="vm-arrow" style="transform:rotate(${v.bearing}deg)"></div>` : '';
        const html = `<div class="vm vm--${kindOf(v.route)}" style="${lineStyle(v.route)}">${turn}<div class="vm-name">${esc(v.route?.name)}</div></div>`;
        const icon = L.divIcon({ className: 'vm-icon', html, iconSize: [32, 32] });
        const late = v.delay == null ? '' : v.delay >= 60 ? ` · +${Math.round(v.delay / 60)} min` : v.delay <= -60 ? ` · −${Math.round(-v.delay / 60)} min` : ' · o czasie';
        const number = v.number ? `<br><span class="muted">pojazd nr ${esc(v.number)}</span>` : '';
        L.marker(latLng(v), { icon, zIndexOffset: 500, keyboard: false })
          .bindPopup(`<b>${esc(v.route?.name)}</b> → ${esc(v.headsign)}${late}${number}`)
          .addTo(vehicles);
      }
    },

    /** Draws `option` (a plan option) from `from` to `to`, or clears it. */
    setJourney(option, from, to) {
      journey.clearLayers();
      if (!option) return;
      for (const leg of option.legs) {
        const a = leg.from ?? from;
        const b = leg.to ?? to;
        if (!a || !b) continue;
        if (leg.kind === 'walk') {
          L.polyline([latLng(a), latLng(b)], { className: 'jr-walk', weight: 4, dashArray: '1 8', interactive: false }).addTo(journey);
          continue;
        }
        const [color] = lineColors(leg.route);
        L.polyline([latLng(a), latLng(b)], { color, weight: 5, opacity: 0.9, interactive: false }).addTo(journey);
        for (const stop of [a, b]) {
          L.circleMarker(latLng(stop), { radius: 5, color, weight: 3, fillColor: '#fff', fillOpacity: 1, interactive: false }).addTo(journey);
        }
      }
    },

    fit(points) {
      const list = points.filter(Boolean).map(latLng);
      if (list.length === 1) map.setView(list[0], 17);
      else if (list.length) map.fitBounds(list, { padding: [30, 30], maxZoom: 17 });
    },

    focus(pos) {
      map.setView(latLng(pos), Math.max(map.getZoom(), 16));
    },
  };
}
