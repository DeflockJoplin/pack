//! GPS privacy zone: suppress wardriving CSV while inside configured radius.

use crate::config::HomeGeoConfig;
use crate::geo::haversine_m;

/// True when a valid fix lies inside the configured home geofence (wardriving rows should be suppressed).
#[must_use]
pub fn home_zone_suppresses_wigle(home: &HomeGeoConfig, lat: f64, lon: f64) -> bool {
    let (hlat, hlon) = match (home.lat, home.lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => (a, b),
        _ => return false,
    };
    let r = home.radius_or_default() as f64;
    haversine_m(hlat, hlon, lat, lon) <= r
}
