use anyhow::{Result, anyhow, bail};
use std::net::{IpAddr, ToSocketAddrs};
use url::Url;

pub fn validate_web_url(
    url: &str,
    allow_plain_http: bool,
    allow_local_addresses: bool,
) -> Result<Url> {
    let parsed = Url::parse(url).map_err(|e| anyhow!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "https" => {}
        "http" if allow_plain_http => {}
        "http" => bail!("plain HTTP is disabled"),
        _ => bail!("unsupported scheme"),
    }

    if !allow_local_addresses {
        let host = parsed.host_str().ok_or_else(|| anyhow!("missing host"))?;

        if host.eq_ignore_ascii_case("localhost") || is_local_hostname(host) {
            bail!("loopback address is blocked");
        }

        if let Ok(ip) = host.parse::<IpAddr>()
            && is_blocked_ip(ip)
        {
            bail!("private/local/metadata addresses are blocked");
        }

        // Resolve DNS and reject any resolved private/local destination.
        // This blocks common SSRF patterns where a public-looking hostname maps to RFC1918/loopback.
        if let Ok(port) = parsed
            .port_or_known_default()
            .ok_or_else(|| anyhow!("missing port"))
        {
            let addr = format!("{host}:{port}");
            if let Ok(resolved) = addr.to_socket_addrs() {
                for socket in resolved {
                    if is_blocked_ip(socket.ip()) {
                        bail!("resolved host points to private/local/metadata addresses");
                    }
                }
            }
        }
    }

    Ok(parsed)
}

fn is_local_hostname(host: &str) -> bool {
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
}

pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.octets() == [169, 254, 169, 254]
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_file_and_data_schemes() {
        assert!(validate_web_url("file:///etc/passwd", false, false).is_err());
        assert!(validate_web_url("data:text/plain,hello", false, false).is_err());
    }

    #[test]
    fn rejects_localhost_by_default() {
        assert!(validate_web_url("https://localhost/test", false, false).is_err());
    }

    #[test]
    fn allows_https_external() {
        assert!(validate_web_url("https://example.com", false, false).is_ok());
    }

    #[test]
    fn rejects_local_hostname_suffixes() {
        assert!(validate_web_url("https://service.local/path", false, false).is_err());
        assert!(validate_web_url("https://service.internal/path", false, false).is_err());
    }
}
