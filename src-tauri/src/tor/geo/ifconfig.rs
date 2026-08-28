//! `ifconfig.co` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://ifconfig.co/json?ip={ip}"),
        None => "https://ifconfig.co/json".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    if v.get("error").is_some() {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;

    let org = match (
        v.get("asn").and_then(|s| s.as_str()),
        v.get("asn_org").and_then(|s| s.as_str()),
    ) {
        (Some(asn), Some(name)) if !name.starts_with(asn) => Some(format!("{asn} {name}")),
        (_, Some(name)) => Some(name.to_string()),
        (Some(asn), None) => Some(asn.to_string()),
        (None, None) => None,
    };

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("country_iso")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        org,
    })
}
