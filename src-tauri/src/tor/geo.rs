//! Client/server geo-location for the Tor map.
//!
//! Queries a public, key-free geolocation service (ipwho.is) over the *direct*
//! network (NOT through Tor) to find the client's own public IP + coarse
//! location + AS org, and the same for the QxChat server (qxch.at). The public
//! IP is masked (last octet / hextet redacted) before it ever leaves Rust, so
//! the frontend only sees location + AS, never the full address.

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

fn parse_geo(v: &serde_json::Value) -> Option<GeoPoint> {
    let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
    if !success {
        return None;
    }
    let raw_ip = v.get("ip").and_then(|s| s.as_str())?;
    let org = v
        .get("connection")
        .and_then(|c| c.get("org"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v.get("country_code").and_then(|s| s.as_str()).map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(|n| n.as_f64()),
        longitude: v.get("longitude").and_then(|n| n.as_f64()),
        org,
    })
}

/// Resolves a hostname to an IPv4/IPv6 address via Google's DNS-over-HTTPS
/// (no system resolver dependency, works cross-platform).
async fn resolve_host(client: &reqwest::Client, host: &str) -> Option<String> {
    let url = format!("https://dns.google/resolve?name={host}&type=A");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("Answer")
        .and_then(|a| a.as_array())
        .and_then(|answers| answers.first())
        .and_then(|a| a.get("data"))
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
}

/// Fetches client + server geolocation for the Tor map.
pub async fn fetch_geo() -> Result<GeoInfo, String> {
    let client = direct_client()?;

    // Client's own public IP + location (ipwho.is self endpoint).
    let client_point = (async {
        let resp = client
            .get("https://ipwho.is/")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(parse_geo(&json))
    })
    .await
    .unwrap_or(None);

    // Server (qxch.at) IP + location.
    let server_point = (async {
        let host = resolve_host(&client, SERVER_HOST).await.ok_or("resolve failed")?;
        let resp = client
            .get(&format!("https://ipwho.is/{host}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(parse_geo(&json))
    })
    .await
    .unwrap_or(None);

    Ok(GeoInfo {
        client: client_point,
        server: server_point,
    })
}
