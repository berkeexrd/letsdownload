use super::link::{LinkKind, NetInfo};
use netdev::interface::types::InterfaceType;
use std::net::Ipv4Addr;

pub struct Found {
    pub key: String,
    pub name: String,
    pub net: NetInfo,
}

fn looks_virtual(s: &str) -> bool {
    let s = s.to_lowercase();
    if s.contains("virtualbox")
        || s.contains("vmware")
        || s.contains("vethernet")
        || s.contains("hyper-v")
    {
        return true;
    }
    [
        "vpn", "tap-", "tun", "wsl", "docker", "loopback", "bluetooth", "wintun", "npcap",
        "tailscale", "zerotier", "virtual adapter", "virtual network", "virtual ethernet",
        "miniport",
    ]
    .iter()
    .any(|p| s.contains(p))
}

fn looks_mobile_or_vpn(s: &str) -> bool {
    let s = s.to_lowercase();
    [
        "rndis", "ndis", "usb", "tether", "mobile", "cellular", "wwan", " 5g", " 4g",
        "iphone", "android", "wireguard", "tun", "tap-", "wintun", "ppp", "wan miniport",
    ]
    .iter()
    .any(|p| s.contains(p))
}

fn usable(ip: &Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() || ip.is_multicast())
}

pub fn discover() -> Vec<Found> {
    let default_index = netdev::get_default_interface().ok().map(|i| i.index);
    let default_gw = netdev::get_default_gateway()
        .ok()
        .and_then(|g| g.ipv4.first().copied());
    let mut out = Vec::new();
    for itf in netdev::get_interfaces() {
        if itf.is_loopback() {
            continue;
        }
        let ip = itf.ipv4.iter().map(|n| n.addr()).find(usable);
        let name = itf.friendly_name.clone().unwrap_or_else(|| itf.name.clone());
        let desc = itf.description.clone().unwrap_or_default();
        let ip = match ip {
            Some(ip) => ip,
            None => continue,
        };
        let is_default = default_index == Some(itf.index);
        let gateway = itf
            .gateway
            .as_ref()
            .and_then(|g| g.ipv4.first().copied())
            .or_else(|| if is_default { default_gw } else { None });
        let virt = looks_virtual(&name) || looks_virtual(&desc);
        let mobile_or_vpn = looks_mobile_or_vpn(&name) || looks_mobile_or_vpn(&desc);
        let kind = if virt {
            LinkKind::Virtual
        } else {
            match itf.if_type {
                InterfaceType::Wireless80211 => LinkKind::Wifi,
                InterfaceType::Ethernet
                | InterfaceType::GigabitEthernet
                | InterfaceType::FastEthernetT
                | InterfaceType::FastEthernetFx
                | InterfaceType::Ethernet3Megabit => LinkKind::Ethernet,
                _ => LinkKind::Other,
            }
        };
        let up = itf.is_up() || itf.is_running() || is_default || gateway.is_some();
        let has_route = gateway.is_some() || is_default || (!virt && up) || (mobile_or_vpn && up);
        out.push(Found {
            key: itf.name.clone(),
            name,
            net: NetInfo {
                ip,
                if_index: itf.index,
                gateway,
                kind,
                description: desc,
                present: up && has_route,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_detection_distinguishes_real_and_virtual_adapters() {
        assert!(looks_virtual("VMware Network Adapter VMnet1"));
        assert!(looks_virtual("vEthernet (Default Switch)"));
        assert!(looks_virtual("Tailscale Tunnel"));
        assert!(!looks_virtual("Realtek PCIe GbE Family Controller"));
        assert!(!looks_virtual("Intel(R) Wi-Fi 6 AX200 160MHz"));
    }

    #[test]
    fn usable_ipv4_filters_non_routable_addresses() {
        assert!(!usable(&Ipv4Addr::LOCALHOST));
        assert!(!usable(&Ipv4Addr::UNSPECIFIED));
        assert!(!usable(&Ipv4Addr::new(169, 254, 1, 5)));
        assert!(usable(&Ipv4Addr::new(192, 168, 1, 15)));
        assert!(usable(&Ipv4Addr::new(10, 0, 0, 2)));
    }

    #[test]
    fn mobile_or_vpn_detection_recognizes_tethering_and_tunnels() {
        assert!(looks_mobile_or_vpn("Remote NDIS based Internet Sharing Device"));
        assert!(looks_mobile_or_vpn("Apple Mobile Device Ethernet"));
        assert!(looks_mobile_or_vpn("WireGuard Tunnel"));
        assert!(!looks_mobile_or_vpn("Realtek PCIe GbE Family Controller"));
    }
}
