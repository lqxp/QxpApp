//! `ipapi.co` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://ipapi.co/{ip}/json/"),
        None => "https://ipapi.co/json/".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    // ipapi.co reports failures/rate-limits as `{"error": true, "reason": …}`.
    if v.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;

    let country_code = v
        .get("country_code")
        .and_then(|s| s.as_str())
        .or_else(|| v.get("country").and_then(|s| s.as_str()))
        .map(|s| s.to_string());

    let org = v
        .get("org")
        .and_then(|s| s.as_str())
        .map(|name| match v.get("asn").and_then(|a| a.as_str()) {
            Some(asn) if !name.starts_with(asn) => format!("{asn} {name}"),
            _ => name.to_string(),
        });

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code,
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        org,
    })
}
