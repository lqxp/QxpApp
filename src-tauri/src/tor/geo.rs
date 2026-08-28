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
//! allocations) or hit free-tier rate limits. We therefore try a chain of
//! several key-free HTTPS providers and return the first success, falling back
//! gracefully to `None` instead of failing the whole map.

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

/// Parses an `ipwho.is` response. Returns `None` on a failed lookup (bogon,
/// rate limit, unknown IP, …).
fn parse_ipwhois(v: &serde_json::Value) -> Option<GeoPoint> {
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
        latitude: v.get("latitude").and_then(|n| n.as_f64()),
        longitude: v.get("longitude").and_then(|n| n.as_f64()),
        org: v
            .get("connection")
            .and_then(|c| c.get("org"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
    })
}

/// Parses an `ipapi.co` response. Returns `None` on a failed lookup.
fn parse_ipapi(v: &serde_json::Value) -> Option<GeoPoint> {
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
        latitude: v.get("latitude").and_then(|n| n.as_f64()),
        longitude: v.get("longitude").and_then(|n| n.as_f64()),
        org,
    })
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Parses an `ipinfo.io` tokenless response. Returns `None` on failure.
fn parse_ipinfo(v: &serde_json::Value) -> Option<GeoPoint> {
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

/// Parses an `ipapi.is` response. Returns `None` on failure.
fn parse_ipapi_is(v: &serde_json::Value) -> Option<GeoPoint> {
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
        latitude: location.get("latitude").and_then(|n| n.as_f64()),
        longitude: location.get("longitude").and_then(|n| n.as_f64()),
        org,
    })
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

/// Parses an `ifconfig.co` response. Returns `None` on failure.
fn parse_ifconfig(v: &serde_json::Value) -> Option<GeoPoint> {
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

/// Parses a `geojs.io` response. Returns `None` on failure.
fn parse_geojs(v: &serde_json::Value) -> Option<GeoPoint> {
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

/// Parses a `freeipapi.com` response. Returns `None` on failure. This provider
/// does not expose AS/org in its free payload.
fn parse_freeipapi(v: &serde_json::Value) -> Option<GeoPoint> {
    let raw_ip = v.get("ipAddress").and_then(|s| s.as_str())?;
    Some(GeoPoint {
        ip: mask_ip(raw_ip),
        country_code: v
            .get("countryCode")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        latitude: v.get("latitude").and_then(as_f64_flex),
        longitude: v.get("longitude").and_then(as_f64_flex),
        org: None,
    })
}

/// Parses an `ipwhois.app` response. Returns `None` on failure.
fn parse_ipwhois_app(v: &serde_json::Value) -> Option<GeoPoint> {
    if !v.get("success").and_then(|s| s.as_bool()).unwrap_or(false) {
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

/// Parses an `extreme-ip-lookup.com` response. Returns `None` on failure.
fn parse_extreme_ip_lookup(v: &serde_json::Value) -> Option<GeoPoint> {
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

/// Looks up the client (`ip == None` → self endpoint) or a specific public IP
/// across providers, returning the first successful result.
async fn lookup_geo(client: &reqwest::Client, ip: Option<&str>) -> Option<GeoPoint> {
    // 1) ipwho.is (generous free tier, HTTPS, no key).
    let ipwhois_url = match ip {
        Some(ip) => format!("https://ipwho.is/{ip}"),
        None => "https://ipwho.is/".to_string(),
    };
    if let Some(json) = get_json(client, &ipwhois_url).await {
        if let Some(point) = parse_ipwhois(&json) {
            return Some(point);
        }
    }

    // 2) ipapi.co (HTTPS, no key, stricter free tier).
    let ipapi_url = match ip {
        Some(ip) => format!("https://ipapi.co/{ip}/json/"),
        None => "https://ipapi.co/json/".to_string(),
    };
    if let Some(json) = get_json(client, &ipapi_url).await {
        if let Some(point) = parse_ipapi(&json) {
            return Some(point);
        }
    }

    // 3) ipinfo.io (tokenless free tier).
    let ipinfo_url = match ip {
        Some(ip) => format!("https://ipinfo.io/{ip}/json"),
        None => "https://ipinfo.io/json".to_string(),
    };
    if let Some(json) = get_json(client, &ipinfo_url).await {
        if let Some(point) = parse_ipinfo(&json) {
            return Some(point);
        }
    }

    // 4) ipapi.is (HTTPS, no key).
    let ipapi_is_url = match ip {
        Some(ip) => format!("https://api.ipapi.is/?q={ip}"),
        None => "https://api.ipapi.is/".to_string(),
    };
    if let Some(json) = get_json(client, &ipapi_is_url).await {
        if let Some(point) = parse_ipapi_is(&json) {
            return Some(point);
        }
    }

    // 5) ifconfig.co (HTTPS, no key).
    let ifconfig_url = match ip {
        Some(ip) => format!("https://ifconfig.co/json?ip={ip}"),
        None => "https://ifconfig.co/json".to_string(),
    };
    if let Some(json) = get_json(client, &ifconfig_url).await {
        if let Some(point) = parse_ifconfig(&json) {
            return Some(point);
        }
    }

    // 6) geojs.io (HTTPS, no key).
    let geojs_url = match ip {
        Some(ip) => format!("https://get.geojs.io/v1/ip/geo/{ip}.json"),
        None => "https://get.geojs.io/v1/ip/geo.json".to_string(),
    };
    if let Some(json) = get_json(client, &geojs_url).await {
        if let Some(point) = parse_geojs(&json) {
            return Some(point);
        }
    }

    // 7) freeipapi.com (HTTPS, no key; no AS/org in free payload).
    let freeipapi_url = match ip {
        Some(ip) => format!("https://freeipapi.com/api/json/{ip}"),
        None => "https://freeipapi.com/api/json/".to_string(),
    };
    if let Some(json) = get_json(client, &freeipapi_url).await {
        if let Some(point) = parse_freeipapi(&json) {
            return Some(point);
        }
    }

    // 8) ipwhois.app (HTTPS, no key).
    let ipwhois_app_url = match ip {
        Some(ip) => format!("https://ipwhois.app/json/{ip}"),
        None => "https://ipwhois.app/json/".to_string(),
    };
    if let Some(json) = get_json(client, &ipwhois_app_url).await {
        if let Some(point) = parse_ipwhois_app(&json) {
            return Some(point);
        }
    }

    // 9) extreme-ip-lookup.com (HTTPS, no key; stricter free tier).
    let extreme_url = match ip {
        Some(ip) => format!("https://extreme-ip-lookup.com/json/{ip}"),
        None => "https://extreme-ip-lookup.com/json/".to_string(),
    };
    if let Some(json) = get_json(client, &extreme_url).await {
        if let Some(point) = parse_extreme_ip_lookup(&json) {
            return Some(point);
        }
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
