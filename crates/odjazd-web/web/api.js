// The odjazd-web API, and the server's clock: countdowns use it, not the phone's.

let skew = 0;

/** The server's time in Unix seconds. */
export const now = () => (Date.now() + skew) / 1000;

export class ApiError extends Error {
  /** `kind` is `offline`, `loading` (the server is still reading the timetable) or `error`. */
  constructor(kind, message) {
    super(message ?? kind);
    this.kind = kind;
  }
}

export async function get(path, params = {}) {
  const query = new URLSearchParams(params).toString();
  const sent = Date.now();
  let response;
  try {
    response = await fetch(query ? `${path}?${query}` : path, { cache: 'no-store' });
  } catch {
    throw new ApiError('offline');
  }
  if (response.status === 503) throw new ApiError('loading');
  const body = await response.json().catch(() => null);
  if (!response.ok || !body) throw new ApiError('error', body?.error ?? `HTTP ${response.status}`);
  if (typeof body.now === 'number') {
    // `now` is whole seconds, so on average half a second behind.
    skew = body.now * 1000 + 500 - (sent + Date.now()) / 2;
  }
  return body;
}
