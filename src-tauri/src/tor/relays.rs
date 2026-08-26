//! Onionoo relay directory lookup, routed through the local Tor SOCKS5 proxy.
//!
//! When Tor is enabled, the directory lookup itself is tunnelled through Tor so
//! that even consulting the public relay directory does not leak a client-side
//! DNS/connection to the WebView. The WebView never talks to Onionoo directly.

use serde::Deserialize;
use serde::Serialize;

/// A single relay as presented to the frontend (mirrors `TorRelay` in
/// `client/src/calls/torRelays.ts`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorRelayInfo {
    pub fingerprint: String,
    pub nickname: String,
    pub address: String,
    pub country: String,
    pub country_name: String,
    pub as_number: String,
    pub as_name: String,
    pub flags: Vec<String>,
    pub running: bool,
    pub contact: String,
    pub consensus_weight_fraction: f64,
}

#[derive(Debug, Deserialize)]
struct RelayDetails {
    relays: Vec<RawRelay>,
}

#[derive(Debug, Deserialize)]
struct RawRelay {
    fingerprint: Option<String>,
    nickname: Option<String>,
    or_addresses: Option<Vec<String>>,
    country: Option<String>,
    country_name: Option<String>,
    #[serde(rename = "as")]
    as_number: Option<String>,
    as_name: Option<String>,
    flags: Option<Vec<String>>,
    running: Option<bool>,
    contact: Option<String>,
    consensus_weight_fraction: Option<f64>,
}

fn first_address(addrs: &[String]) -> String {
    addrs
        .iter()
        .find(|a| a.contains('.'))
        .or_else(|| addrs.first())
        .cloned()
        .unwrap_or_default()
}

/// Fetches running relays from the official Onionoo API through the Tor SOCKS5
/// proxy on `127.0.0.1:{port}`. `socks5h` = DNS resolved by Tor (no local leak).
pub async fn fetch_relays(port: u16, limit: usize) -> Result<Vec<TorRelayInfo>, String> {
    let proxy = format!("socks5h://127.0.0.1:{port}");

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy).map_err(|e| format!("proxy: {e}"))?)
        .user_agent("QxChat/1.0 (+https://qxch.at)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("client: {e}"))?;

    let url = format!(
        "https://onionoo.torproject.org/details?fields=fingerprint,nickname,or_addresses,country,country_name,as,as_name,flags,running,contact,consensus_weight_fraction&running=true&order=-consensus_weight&limit={limit}"
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("request via {proxy}: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("onionoo HTTP {status}"));
    }

    let details: RelayDetails = resp
        .json()
        .await
        .map_err(|e| format!("parse: {e}"))?;

    let relays = details
        .relays
        .into_iter()
        .map(|r| TorRelayInfo {
            fingerprint: r.fingerprint.unwrap_or_default(),
            nickname: r.nickname.unwrap_or_else(|| "Unnamed".into()),
            address: first_address(&r.or_addresses.unwrap_or_default()),
            country: r.country.unwrap_or_default(),
            country_name: r.country_name.unwrap_or_default(),
            as_number: r.as_number.unwrap_or_default(),
            as_name: r.as_name.unwrap_or_default(),
            flags: r.flags.unwrap_or_default(),
            running: r.running.unwrap_or(false),
            contact: r.contact.unwrap_or_default(),
            consensus_weight_fraction: r.consensus_weight_fraction.unwrap_or(0.0),
        })
        .filter(|r| !r.fingerprint.is_empty())
        .collect();

    Ok(relays)
}
