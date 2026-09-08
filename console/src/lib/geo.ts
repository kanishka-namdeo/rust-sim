/**
 * WGS84 geodesy for the Operator Map (ADR-0017).
 *
 * TypeScript port of the Rust `GeoOrigin` (sim's sitsim-env / fleet-core's
 * geo.rs — same repo, proven live against PX4 SITL): NED <-> geodetic via
 * ECEF with Bowring's closed form, exact to well under a millimetre over
 * the ~100 m geofence scale the map works at.
 */

const WGS84_A = 6378137.0
const WGS84_F = 1 / 298.257223563
const WGS84_E2 = 0.0066943799901413165
const WGS84_B = WGS84_A * (1 - WGS84_F)
const WGS84_EP2 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B)

/** The PX4 test field — the same constant the Rust side pins. */
export const DEFAULT_ORIGIN: GeoOrigin = { lat_deg: 47.397770, lon_deg: 8.545580, alt_m: 500.0 }

export interface GeoOrigin {
  lat_deg: number
  lon_deg: number
  alt_m: number
}

export interface LatLng {
  lat: number
  lng: number
}

function geodeticToEcef(latDeg: number, lonDeg: number, altM: number): [number, number, number] {
  const lat = (latDeg * Math.PI) / 180
  const lon = (lonDeg * Math.PI) / 180
  const sl = Math.sin(lat)
  const cl = Math.cos(lat)
  const so = Math.sin(lon)
  const co = Math.cos(lon)
  const n = WGS84_A / Math.sqrt(1 - WGS84_E2 * sl * sl)
  const nx = (n + altM) * cl
  return [nx * co, nx * so, (n * (1 - WGS84_E2) + altM) * sl]
}

function ecefToGeodetic(p: [number, number, number]): [number, number, number] {
  const [x, y, z] = p
  const lon = Math.atan2(y, x)
  const pr = Math.hypot(x, y)
  const theta = Math.atan2(z * WGS84_A, pr * WGS84_B)
  const st = Math.sin(theta)
  const ct = Math.cos(theta)
  const num = z + WGS84_EP2 * WGS84_B * st * st * st
  const den = pr - WGS84_E2 * WGS84_A * ct * ct * ct
  const lat = Math.atan2(num, den)
  const s2 = Math.sin(lat) ** 2
  const n = WGS84_A / Math.sqrt(1 - WGS84_E2 * s2)
  const alt =
    Math.abs(Math.cos(lat)) > 1e-12 ? pr / Math.cos(lat) - n : z / Math.sin(lat) - n * (1 - WGS84_E2)
  return [(lat * 180) / Math.PI, (lon * 180) / Math.PI, alt]
}

/**
 * NED metres (n, e, d — home-relative, d positive down) -> geodetic
 * (lat, lon, MSL altitude) relative to `origin`.
 */
export function nedToGeodetic(origin: GeoOrigin, ned: [number, number, number]): LatLng & { alt_m: number } {
  const p0 = geodeticToEcef(origin.lat_deg, origin.lon_deg, origin.alt_m)
  const lat = (origin.lat_deg * Math.PI) / 180
  const lon = (origin.lon_deg * Math.PI) / 180
  const [sl, cl] = [Math.sin(lat), Math.cos(lat)]
  const [so, co] = [Math.sin(lon), Math.cos(lon)]
  const [n, e, d] = ned
  const p: [number, number, number] = [
    p0[0] - sl * co * n - so * e - cl * co * d,
    p0[1] - sl * so * n + co * e - cl * so * d,
    p0[2] + cl * n - sl * d,
  ]
  const [latDeg, lonDeg, altM] = ecefToGeodetic(p)
  return { lat: latDeg, lng: lonDeg, alt_m: altM }
}

/**
 * Geodetic (degrees, MSL metres) -> NED metres relative to `origin`.
 * Inverse of {@link nedToGeodetic}.
 */
export function geodeticToNed(origin: GeoOrigin, latDeg: number, lonDeg: number, altM: number): [number, number, number] {
  const p0 = geodeticToEcef(origin.lat_deg, origin.lon_deg, origin.alt_m)
  const p = geodeticToEcef(latDeg, lonDeg, altM)
  const d = [p[0] - p0[0], p[1] - p0[1], p[2] - p0[2]]
  const lat = (origin.lat_deg * Math.PI) / 180
  const lon = (origin.lon_deg * Math.PI) / 180
  const [sl, cl] = [Math.sin(lat), Math.cos(lat)]
  const [so, co] = [Math.sin(lon), Math.cos(lon)]
  return [
    -sl * co * d[0] - sl * so * d[1] + cl * d[2],
    -so * d[0] + co * d[1],
    -cl * co * d[0] - cl * so * d[1] - sl * d[2],
  ]
}

/**
 * NED polygon ([n, e] pairs) -> geo polygon, for drawing the geofence on
 * the map (the fence rides the fleet frame in NED, ADR-0017).
 */
export function nedPolygonToGeo(origin: GeoOrigin, pts: [number, number][]): LatLng[] {
  return pts.map(([n, e]) => {
    const g = nedToGeodetic(origin, [n, e, 0])
    return { lat: g.lat, lng: g.lng }
  })
}
