//! `ipwho.is` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://ipwho.is/{ip}"),
        None => "https://ipwho.is/".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
    if !success {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("country_code")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        org: v
            .get("connection")
            .and_then(|c| c.get("org"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
    })
}
