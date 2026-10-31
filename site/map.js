// The map: every pole in Warsaw, coloured by what leaves it, on OpenFreeMap's vector tiles
// of OpenStreetMap. The page imports this module on its own, so that without WebGL or the
// map library it still searches stops and shows boards.

import * as maplibregl from 'maplibre-gl';

const STYLES = {
  dark: 'https://tiles.openfreemap.org/styles/dark',
  light: 'https://tiles.openfreemap.org/styles/positron',
};

const WARSAW = [21.0122, 52.2297];

const CREDIT =
  'Timetable <a href="https://ztm.waw.pl">ZTM Warszawa</a>, ' +
  '<a href="https://mkuran.pl/gtfs/">mkuran.pl</a>';

/** Pole colours by what leaves them: the line colours, a little brighter on the dark map. */
const COLORS = {
  dark: { metro: '#5b7bff', tram: '#ff4d4d', rail: '#2fbf71', bus: '#c45cf0', none: '#59636e', ring: '#ffb000', halo: '#0d1117' },
  light: { metro: '#0000bb', tram: '#b60000', rail: '#006b3c', bus: '#880077', none: '#8c959f', ring: '#d18f00', halo: '#ffffff' },
};

/** Drawn on top of each other in this order, so the metro is never hidden under buses. */
const SORT = ['match', ['get', 'kind'], 'metro', 4, 'tram', 3, 'rail', 2, 'bus', 1, 0];

const RADIUS = ['interpolate', ['linear'], ['zoom'], 10, 1.6, 13, 3, 16, 6.5, 18, 9];

const RING = ['interpolate', ['linear'], ['zoom'], 10, 6, 13, 7, 16, 11, 18, 14];

/** Taps this many pixels off a pole still pick it: poles are small on a phone. */
const REACH = 12;

/**
 * Shows the map in `container`. `onPick(group)` is called with the stop group of a pole
 * the rider clicks.
 */
export function createMap(container, { onPick }) {
  const dark = matchMedia('(prefers-color-scheme: dark)');
  const theme = () => (dark.matches ? 'dark' : 'light');
  const map = new maplibregl.Map({
    container,
    style: STYLES[theme()],
    center: WARSAW,
    zoom: innerWidth < 600 ? 10.6 : 11.4,
    minZoom: 8,
    maxZoom: 19,
    // On a phone the map is part of a page that scrolls: one finger scrolls the page.
    cooperativeGestures: matchMedia('(pointer: coarse)').matches,
  });
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'top-right');
  new ResizeObserver(() => map.resize()).observe(container);

  let data = null;
  let selected = -1;
  let styled = false;
  let you = null;

  // Called when a style has loaded, and again once the poles arrive.
  function draw() {
    if (!styled || !data) return;
    const source = map.getSource('poles');
    if (source) {
      source.setData(data);
      return;
    }
    const c = COLORS[theme()];
    map.addSource('poles', { type: 'geojson', data, attribution: CREDIT });
    map.addLayer({
      id: 'poles',
      type: 'circle',
      source: 'poles',
      layout: { 'circle-sort-key': SORT },
      paint: {
        'circle-color': ['match', ['get', 'kind'], 'metro', c.metro, 'tram', c.tram, 'rail', c.rail, 'bus', c.bus, c.none],
        'circle-radius': RADIUS,
        'circle-stroke-color': c.halo,
        'circle-stroke-width': ['interpolate', ['linear'], ['zoom'], 12, 0, 14, 1.2],
      },
    });
    map.addLayer({
      id: 'selected',
      type: 'circle',
      source: 'poles',
      filter: ['==', ['get', 'group'], selected],
      paint: {
        'circle-color': 'rgba(0, 0, 0, 0)',
        'circle-radius': RING,
        'circle-stroke-color': c.ring,
        'circle-stroke-width': 3,
      },
    });
    map.addLayer({
      id: 'codes',
      type: 'symbol',
      source: 'poles',
      minzoom: 16,
      layout: {
        'text-field': ['get', 'code'],
        'text-font': ['Noto Sans Bold'],
        'text-size': 11,
        'text-anchor': 'top',
        'text-offset': [0, 0.9],
      },
      paint: {
        'text-color': dark.matches ? '#e6edf3' : '#1f2328',
        'text-halo-color': c.halo,
        'text-halo-width': 1.5,
      },
    });
  }

  map.on('style.load', () => {
    styled = true;
    draw();
  });
  // The dark style names a pattern its sprite does not have: draw nothing for it.
  map.setMissingStyleImageResolver((id) => {
    if (!map.hasImage(id)) map.addImage(id, { width: 1, height: 1, data: new Uint8Array(4) });
  });
  dark.addEventListener('change', () => {
    styled = false;
    map.setStyle(STYLES[theme()], { diff: false });
  });

  map.on('click', (event) => {
    if (!map.getLayer('poles')) return;
    const { x, y } = event.point;
    const near = map.queryRenderedFeatures(
      [
        [x - REACH, y - REACH],
        [x + REACH, y + REACH],
      ],
      { layers: ['poles'] },
    );
    const away = (feature) => map.project(feature.geometry.coordinates).dist(event.point);
    const nearest = near.reduce((best, f) => (!best || away(f) < away(best) ? f : best), null);
    if (nearest) onPick(nearest.properties.group);
  });

  if (matchMedia('(hover: hover)').matches) {
    const tip = new maplibregl.Popup({
      closeButton: false,
      closeOnClick: false,
      offset: 10,
      className: 'pole-tip',
    });
    map.on('mousemove', 'poles', (event) => {
      const pole = event.features[0];
      const { name, code } = pole.properties;
      map.getCanvas().style.cursor = 'pointer';
      tip.setLngLat(pole.geometry.coordinates).setText(code ? `${name} ${code}` : name).addTo(map);
    });
    map.on('mouseleave', 'poles', () => {
      map.getCanvas().style.cursor = '';
      tip.remove();
    });
  }

  return {
    /** Puts the poles returned by `Timetable.poles()` on the map. */
    setPoles(poles) {
      data = {
        type: 'FeatureCollection',
        features: poles.map((pole) => ({
          type: 'Feature',
          geometry: { type: 'Point', coordinates: [pole.lon, pole.lat] },
          properties: { group: pole.group, kind: pole.kind ?? 'none', name: pole.name, code: pole.code ?? '' },
        })),
      };
      draw();
    },

    /** Rings the poles of `group` and flies to it; `null` clears the ring. */
    select(group) {
      selected = group?.id ?? -1;
      if (map.getLayer('selected')) map.setFilter('selected', ['==', ['get', 'group'], selected]);
      if (group) map.flyTo({ center: [group.lon, group.lat], zoom: Math.max(map.getZoom(), 15.5) });
    },

    /** Marks where the rider is. */
    showYou(lat, lon) {
      if (!you) {
        const dot = document.createElement('div');
        dot.className = 'you';
        dot.title = 'You';
        you = new maplibregl.Marker({ element: dot });
      }
      you.setLngLat([lon, lat]).addTo(map);
    },

    /** Shows all of `points` ({lat, lon}). */
    fit(points) {
      const lons = points.map((p) => p.lon);
      const lats = points.map((p) => p.lat);
      const bounds = [
        [Math.min(...lons), Math.min(...lats)],
        [Math.max(...lons), Math.max(...lats)],
      ];
      map.fitBounds(bounds, { padding: 48, maxZoom: 16.5 });
    },
  };
}
