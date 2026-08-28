//! `ipapi.is` geolocation provider.

use super::{as_f64_flex, get_json, mask_ip, GeoPoint};

pub async fn lookup(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    let url = match ip {
        Some(ip) => format!("https://api.ipapi.is/?q={ip}"),
        None => "https://api.ipapi.is/".to_string(),
    };
    let json = get_json(client, &url).await?;
    parse(&json)
}

fn parse(v: &serde_json::Value) -> Option<GeoPoint> {
    // ipapi.is reports failures as `{"error": {…}}`.
    if v.get("error").is_some() {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;
    let location = v.get("location")?;

    let org_name = v
        .get("company")
        .and_then(|c| c.get("name"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let org = match (
        v.get("asn")
            .and_then(|a| a.get("asn"))
            .and_then(|n| n.as_i64()),
        org_name,
    ) {
        (Some(asn), Some(name)) => Some(format!("AS{asn} {name}")),
        (_, Some(name)) => Some(name),
        (Some(asn), None) => Some(format!("AS{asn}")),
        (None, None) => None,
    };

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: location
            .get("country_code")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: location.get("latitude").and_then(as_f64_flex),
        longitude: location.get("longitude").and_then(as_f64_flex),
        org,
    })
}
