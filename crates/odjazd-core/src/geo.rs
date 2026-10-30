/// Mean radius of the Earth.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Walking pace, 4.7 km/h.
const WALKING_SPEED: f64 = 1.3;

/// Streets don't run as the crow flies: a walk is about this much longer than the
/// straight line.
const DETOUR: f64 = 1.3;

/// A point on the map, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    /// Metres to `other` in a straight line. Treats the Earth as flat around the two
    /// points, which within a city is off by centimetres.
    pub fn distance(self, other: LatLon) -> f64 {
        let (east, north) = self.offset(other);
        east.hypot(north)
    }

    /// Where `other` is from here, in metres east and north.
    pub(crate) fn offset(self, other: LatLon) -> (f64, f64) {
        let mid_lat = ((self.lat + other.lat) / 2.0).to_radians();
        let east = (other.lon - self.lon).to_radians() * mid_lat.cos();
        let north = (other.lat - self.lat).to_radians();
        (east * EARTH_RADIUS_M, north * EARTH_RADIUS_M)
    }
}

/// Seconds it takes to walk between two points `metres` apart in a straight line.
pub fn walking_time(metres: f64) -> u32 {
    (metres * DETOUR / WALKING_SPEED).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_distances_within_a_city() {
        // Metro Świętokrzyska to Pl. Narutowicza: 2.4 km as the crow flies.
        let swietokrzyska = LatLon::new(52.235107, 21.007898);
        let narutowicza = LatLon::new(52.219530, 20.983290);
        let distance = swietokrzyska.distance(narutowicza);
        assert!((distance - 2_400.0).abs() < 50.0, "{distance}");
        assert_eq!(walking_time(130.0), 130);
    }
}
