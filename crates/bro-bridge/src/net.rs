//! Access URLs advertised to clients (`server.urls`) and the pairing QR code.

use qrcode::{Color, QrCode};
use serde_json::{Value, json};
use std::net::IpAddr;

/// Quiet-zone width (modules) around the pairing QR code.
const QR_QUIET_ZONE: usize = 2;

/// Non-loopback IPv4 addresses per interface, sorted by interface name.
pub(crate) fn network_interface_addresses() -> Vec<(String, IpAddr)> {
    let mut interfaces: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| {
            let address = interface.ip();
            (address.is_ipv4() && !address.is_loopback() && !address.is_unspecified())
                .then_some((interface.name, address))
        })
        .collect();
    interfaces.sort_by(|left, right| {
        left.0
            .to_lowercase()
            .cmp(&right.0.to_lowercase())
            .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
    });
    interfaces
}

/// `ServerAccessUrl[]`: the local URL first (no token needed on loopback),
/// then one tokenized URL per LAN interface when bound to a wildcard address,
/// or the bound address itself.
pub(crate) fn server_access_urls(
    host: IpAddr,
    port: u16,
    token: &str,
    interfaces: &[(String, IpAddr)],
) -> Vec<Value> {
    let mut urls = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |label: &str, address: IpAddr, scope: &str| {
        let address_text = address.to_string();
        let uri_host = match address {
            IpAddr::V4(_) => address_text.clone(),
            IpAddr::V6(_) => format!("[{address_text}]"),
        };
        let token_required = scope == "network";
        let url = if token_required {
            format!("http://{uri_host}:{port}/?token={token}")
        } else {
            format!("http://{uri_host}:{port}/")
        };
        if seen.insert(url.clone()) {
            urls.push(json!({
                "label": label,
                "address": address_text,
                "url": url,
                "scope": scope,
                "tokenRequired": token_required
            }));
        }
    };
    add("Local", IpAddr::from([127, 0, 0, 1]), "local");
    if host.is_unspecified() {
        for (label, address) in interfaces {
            add(label, *address, "network");
        }
    } else if !host.is_loopback() {
        add("Bound host", host, "network");
    }
    urls
}

/// Renders `data` as a QR code in unicode half blocks, two module rows per
/// text row, with a quiet zone. Light modules are drawn as ink (`█▀▄`), dark
/// modules as spaces: draw it light-on-dark (the usual terminal look, same as
/// `qrencode -t UTF8`) so the result scans as a normal dark-on-light code.
pub fn render_qr(data: &str) -> Option<Vec<String>> {
    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + QR_QUIET_ZONE * 2;
    let light = |x: usize, y: usize| -> bool {
        if x < QR_QUIET_ZONE || y < QR_QUIET_ZONE {
            return true;
        }
        let (mx, my) = (x - QR_QUIET_ZONE, y - QR_QUIET_ZONE);
        if mx >= width || my >= width {
            return true;
        }
        colors[my * width + mx] == Color::Light
    };
    let mut rows = Vec::with_capacity(size.div_ceil(2));
    let mut y = 0;
    while y < size {
        let row: String = (0..size)
            .map(|x| {
                let top = light(x, y);
                // An odd final row pairs with quiet zone below it.
                let bottom = y + 1 >= size || light(x, y + 1);
                match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                }
            })
            .collect();
        rows.push(row);
        y += 2;
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_host_advertises_every_ipv4_interface_with_authenticated_urls() {
        let interfaces = vec![
            ("Ethernet".into(), "192.168.86.20".parse().unwrap()),
            ("Tailscale".into(), "100.107.170.47".parse().unwrap()),
            ("Duplicate".into(), "192.168.86.20".parse().unwrap()),
        ];
        let urls = server_access_urls("0.0.0.0".parse().unwrap(), 10001, "safe_token", &interfaces);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0]["scope"], "local");
        assert_eq!(urls[0]["tokenRequired"], false);
        assert_eq!(urls[0]["url"], "http://127.0.0.1:10001/");
        assert_eq!(urls[1]["label"], "Ethernet");
        assert_eq!(
            urls[1]["url"],
            "http://192.168.86.20:10001/?token=safe_token"
        );
        assert_eq!(urls[2]["label"], "Tailscale");
        assert_eq!(urls[2]["tokenRequired"], true);
    }

    #[test]
    fn explicit_host_advertises_only_the_bound_network_address() {
        let interfaces = vec![("Ethernet".into(), "192.168.86.20".parse().unwrap())];
        let urls = server_access_urls(
            "10.20.30.40".parse().unwrap(),
            10009,
            "safe_token",
            &interfaces,
        );
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[1]["label"], "Bound host");
        assert_eq!(urls[1]["url"], "http://10.20.30.40:10009/?token=safe_token");
        let loopback = server_access_urls("127.0.0.1".parse().unwrap(), 1, "t", &interfaces);
        assert_eq!(loopback.len(), 1);
    }

    #[test]
    fn qr_uses_half_blocks_with_a_quiet_zone() {
        let rows =
            render_qr("http://192.168.1.2:10001/?token=abcdefghijklmnopqrstuvwxyz012345").unwrap();
        let code = QrCode::new(b"http://192.168.1.2:10001/?token=abcdefghijklmnopqrstuvwxyz012345")
            .unwrap();
        let size = code.width() + QR_QUIET_ZONE * 2;
        assert_eq!(rows.len(), size.div_ceil(2));
        assert!(rows.iter().all(|row| row.chars().count() == size));
        assert!(
            rows.iter()
                .flat_map(|row| row.chars())
                .all(|c| matches!(c, '█' | '▀' | '▄' | ' '))
        );
        // The quiet zone is light on every side.
        assert!(rows[0].chars().all(|c| c == '█'));
        assert!(
            rows.iter()
                .all(|row| row.starts_with("██") && row.ends_with("██"))
        );
        // The top-left finder pattern starts dark right after the quiet zone.
        assert_eq!(rows[1].chars().nth(QR_QUIET_ZONE), Some(' '));
    }
}
