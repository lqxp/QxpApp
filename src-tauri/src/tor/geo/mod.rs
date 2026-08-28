//! Client/server geo-location for the Tor map.
//!
//! Queries public, key-free geolocation services over the *direct* network (NOT
//! through Tor) to find the client's own public IP + coarse location + AS org,
//! and the same for the QxChat server (qxch.at). The public IP is masked (last
//! octet / hextet redacted) before it ever leaves Rust, so the frontend only
//! sees location + AS, never the full address.
//!
//! A single provider is not reliable enough: free geo-IP databases are
//! approximate and frequently have gaps (CGNAT/mobile ranges, IPv6, brand-new
//! allocations) or hit free-tier rate limits. Each provider lives in its own
//! module and is tried in order by [`lookup_geo`], which returns the first
//! success.

mod extreme_ip_lookup;
mod freeipapi;
mod geojs;
mod ifconfig;
mod ipapi_co;
mod ipapi_is;
mod ipinfo;
mod ipwhois;
mod ipwhois_app;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoPoint {
    /// Masked public IP (e.g. "203.0.113.xxx" / "2606:4700::xxx").
    pub ip: String,
    pub country_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// AS + organization name (e.g. "AS24940 Hetzner Online GmbH").
    pub org: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoInfo {
    pub client: Option<GeoPoint>,
    pub server: Option<GeoPoint>,
}

const SERVER_HOST: &str = "qxch.at";

/// Masks the last octet (IPv4) or last hextet group (IPv6) of a public IP so
/// the full address never reaches the UI.
fn mask_ip(ip: &str) -> String {
    if ip.contains(':') {
        // IPv6: replace the final hextet(s) with "xxx".
        let idx = ip.rfind(':').map(|i| i + 1).unwrap_or(0);
        format!("{}xxx", &ip[..idx])
    } else {
        // IPv4: replace the last octet.
        match ip.rfind('.') {
            Some(i) => format!("{}xxx", &ip[..i + 1]),
            None => ip.to_string(),
        }
    }
}

/// Builds a client with no proxy (we intentionally query over the direct
/// network, since we want the *real* client IP, not the Tor exit IP).
fn direct_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("QxChat/1.0 (+https://qxch.at)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("client: {e}"))
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Reads a numeric value that may be encoded as a JSON number *or* a
/// string (several free geo APIs return `"latitude": "37.386"`).
fn as_f64_flex(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Looks up the client (`ip == None` → self endpoint) or a specific public IP
/// across providers, returning the first successful result.
async fn lookup_geo(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    if let Some(point) = ipwhois::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = ipapi_co::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = ipinfo::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = ipapi_is::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = ifconfig::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = geojs::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = freeipapi::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = ipwhois_app::lookup(client, ip).await {
        return Some(point);
    }
    if let Some(point) = extreme_ip_lookup::lookup(client, ip).await {
        return Some(point);
    }
    None
}

/// Resolves a hostname to an IPv4/IPv6 address via Google's DNS-over-HTTPS
/// (no system resolver dependency, works cross-platform).
async fn resolve_host(client: &reqwest::Client, host: &str) -> Option<String> {
    let url = format!("https://dns.google/resolve?name={host}&type=A");
    let json = get_json(client, &url).await?;
    json.get("Answer")
        .and_then(|a| a.as_array())
        .and_then(|answers| answers.first())
        .and_then(|a| a.get("data"))
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
}

/// Fetches client + server geolocation for the Tor map. Never fails the whole
/// call: an unresolvable point simply becomes `None`.
pub async fn fetch_geo() -> Result<GeoInfo, String> {
    let client = direct_client()?;

    let client_point = lookup_geo(&client, None).await;

    let server_point = match resolve_host(&client, SERVER_HOST).await {
        Some(host) => lookup_geo(&client, Some(&host)).await,
        None => None,
    };

    Ok(GeoInfo {
        client: client_point,
        server: server_point,
    })
}

/// Geolocates a single public IP (e.g. a Tor relay hop) through the provider
/// fallback chain. Returns `None` when every provider fails to resolve it.
pub async fn lookup_ip(ip: &str) -> Result<Option<GeoPoint>, String> {
    let client = direct_client()?;
    Ok(lookup_geo(&client, Some(ip)).await)
}
