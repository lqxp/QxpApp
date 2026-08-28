//! `freeipapi.com` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://freeipapi.com/api/json/{ip}"),
        None => "https://freeipapi.com/api/json/".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    let raw_ip = v.get("ipAddress").and_then(|s| s.as_str())?;
    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("countryCode")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        // freeipapi.com does not expose AS/org in its free payload.
        org: None,
    })
}
