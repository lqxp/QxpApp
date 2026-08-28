//! `ipinfo.io` (tokenless) geolocation provider.

use super::{get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://ipinfo.io/{ip}/json"),
        None => "https://ipinfo.io/json".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    // Tokenless failures come back as `{"error": …}`.
    if v.get("error").is_some() {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;

    // `loc` is a single "lat,lng" string.
    let (latitude, longitude) = v
        .get("loc")
        .and_then(|l| l.as_str())
        .and_then(|loc| {
            let mut parts = loc.splitn(2, ',');
            let lat = parts.next()?.trim().parse::<f64>().ok()?;
            let lng = parts.next()?.trim().parse::<f64>().ok()?;
            Some((lat, lng))
        })
        .map(|(lat, lng)| (Some(lat), Some(lng)))
        .unwrap_or((None, None));

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("country")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude,
        longitude,
        org: v
            .get("org")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
    })
}
