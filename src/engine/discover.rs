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
    [
        "vmware", "virtualbox", "vethernet", "hyper-v", "virtual", "vpn", "tap-", "tun", "wsl",
        "docker", "loopback", "bluetooth", "wintun", "npcap", "tailscale", "zerotier",
    ]
    .iter()
    .any(|p| s.contains(p))
}

fn usable(ip: &Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() || ip.is_multicast())
}

pub fn discover() -> Vec<Found> {
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
        let gateway = itf.gateway.as_ref().and_then(|g| g.ipv4.first().copied());
        let virt = looks_virtual(&name) || looks_virtual(&desc);
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
        out.push(Found {
            key: itf.name.clone(),
            name,
            net: NetInfo {
                ip,
                if_index: itf.index,
                gateway,
                kind,
                description: desc,
                present: itf.is_up() && gateway.is_some(),
            },
        });
    }
    out
}
