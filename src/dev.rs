use crate::engine::Engine;
use crate::engine::discover::Found;
use crate::engine::link::{LinkKind, NetInfo};
use std::net::Ipv4Addr;
use std::sync::Arc;

pub fn env(name: &str) -> Option<String> {
    #[cfg(debug_assertions)]
    {
        std::env::var(name).ok()
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = name;
        None
    }
}

pub fn fake_mode() -> bool {
    env("LETSDOWNLOAD_FAKE_LINKS").is_some()
}

pub fn is_fake(key: &str) -> bool {
    key.starts_with("fake-")
}

pub fn link_allowed(key: &str) -> bool {
    !fake_mode() || is_fake(key)
}

pub fn fake_links() -> Vec<Found> {
    if !fake_mode() {
        return Vec::new();
    }
    [
        ("fake-a", "Test Hat A", Ipv4Addr::new(127, 0, 0, 2), LinkKind::Ethernet),
        ("fake-b", "Test Hat B", Ipv4Addr::new(127, 0, 0, 3), LinkKind::Wifi),
    ]
    .into_iter()
    .map(|(key, name, ip, kind)| Found {
        key: key.into(),
        name: name.into(),
        net: NetInfo {
            ip,
            if_index: 0,
            gateway: Some(Ipv4Addr::LOCALHOST),
            kind,
            description: "test".into(),
            present: true,
        },
    })
    .collect()
}

pub fn window_size() -> Option<[f32; 2]> {
    let v = env("LETSDOWNLOAD_SIZE")?;
    let (a, b) = v.split_once('x')?;
    Some([a.parse().ok()?, b.parse().ok()?])
}

pub fn apply(eng: &Arc<Engine>) {
    if let Some(v) = env("LETSDOWNLOAD_LIMITS") {
        for part in v.split(',') {
            if let Some((k, mb)) = part.split_once(':') {
                eng.set_link_limit(k, mb.parse::<u64>().unwrap_or(0) * 1_000_000);
            }
        }
    }
}

pub fn run_cli(eng: &Arc<Engine>) -> bool {
    #[cfg(debug_assertions)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("--cli") {
            cli(eng, &args[1..]);
            return true;
        }
    }
    let _ = eng;
    false
}

#[cfg(debug_assertions)]
fn cli(eng: &Arc<Engine>, args: &[String]) {
    use crate::engine::job::Status;
    use std::time::Duration;

    for a in args {
        if let Some(rest) = a.strip_prefix("--limit=") {
            if let Some((k, mb)) = rest.split_once(':') {
                eng.set_link_limit(k, mb.parse::<u64>().unwrap_or(0) * 1_000_000);
            }
        } else if let Some(k) = a.strip_prefix("--off=") {
            eng.set_link_enabled(k, false);
        } else if let Some(dir) = a.strip_prefix("--dir=") {
            let mut s = eng.settings.lock().unwrap().clone();
            s.download_dir = dir.into();
            eng.set_settings(s);
        }
    }
    if fake_mode() {
        for l in eng.links.read().unwrap().iter() {
            if !is_fake(&l.key) {
                l.enabled.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    if args.iter().any(|a| a == "--resume") {
        eng.resume_all();
    }
    for u in args.iter().filter(|a| !a.starts_with("--")) {
        eng.add_urls(u);
    }
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let v = eng.view();
        let mut line = String::new();
        for l in v.links.iter().filter(|l| l.present && l.enabled) {
            line += &format!(
                "[{} {:.1}MB/s used {:.1}MB{}] ",
                l.name,
                l.rate / 1e6,
                l.used as f64 / 1e6,
                if l.exhausted { " FULL" } else { "" }
            );
        }
        for j in &v.jobs {
            line += &format!(
                "| {} {:?} {:.1}/{:.1}MB {:.1}MB/s {:?} {}",
                j.name,
                j.status,
                j.downloaded as f64 / 1e6,
                j.size.unwrap_or(0) as f64 / 1e6,
                j.speed / 1e6,
                j.per_link.iter().map(|(s, b)| (*s, b / 1_000_000)).collect::<Vec<_>>(),
                j.error.clone().or(j.note.clone()).unwrap_or_default()
            );
        }
        println!("{line}");
        let settled = v
            .jobs
            .iter()
            .all(|j| matches!(j.status, Status::Completed | Status::Error | Status::Waiting | Status::Paused));
        if settled && !v.jobs.is_empty() {
            break;
        }
    }
    eng.shutdown();
}
