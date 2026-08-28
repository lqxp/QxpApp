//! `extreme-ip-lookup.com` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://extreme-ip-lookup.com/json/{ip}"),
        None => "https://extreme-ip-lookup.com/json/".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    if !v
        .get("status")
        .and_then(|s| s.as_str())
        .map(|s| s == "success")
        .unwrap_or(false)
    {
        return None;
    }
    let raw_ip = v.get("query").and_then(|s| s.as_str())?;

    let org = v
        .get("as")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .or_else(|| v.get("org").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .or_else(|| v.get("isp").and_then(|s| s.as_str()).map(|s| s.to_string()));

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("countryCode")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("lat").and_then(as_f64_flex),
        longitude: v.get("lon").and_then(as_f64_flex),
        org,
    })
}
