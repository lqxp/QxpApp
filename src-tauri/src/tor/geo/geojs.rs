//! `geojs.io` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://get.geojs.io/v1/ip/geo/{ip}.json"),
        None => "https://get.geojs.io/v1/ip/geo.json".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;
    if raw_ip.is_empty() {
        return None;
    }

    let org = v
        .get("organization")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .or_else(|| v.get("asn").and_then(|s| s.as_str()).map(|s| s.to_string()));

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("country_code")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        org,
    })
}
