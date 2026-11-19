use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct NetInfo {
    pub ip: Ipv4Addr,
    pub if_index: u32,
    pub gateway: Option<Ipv4Addr>,
    pub kind: LinkKind,
    pub description: String,
    pub present: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Ethernet,
    Wifi,
    Virtual,
    Other,
}

#[derive(Default)]
struct Quota {
    used: u64,
    reserved: u64,
}

pub struct Link {
    pub key: String,
    pub slot: usize,
    pub name: RwLock<String>,
    pub net: RwLock<NetInfo>,
    pub enabled: AtomicBool,

    pub limit: AtomicU64,

    pub speed_cap: AtomicU64,
    quota: Mutex<Quota>,

    pub tick_bytes: AtomicU64,

    pub rate: Mutex<f64>,
    pub total_session: AtomicU64,
    pace_next: Mutex<Instant>,
}

impl Link {
    pub fn new(key: String, slot: usize, name: String, net: NetInfo) -> Self {
        Link {
            key,
            slot,
            name: RwLock::new(name),
            net: RwLock::new(net),
            enabled: AtomicBool::new(true),
            limit: AtomicU64::new(0),
            speed_cap: AtomicU64::new(0),
            quota: Mutex::new(Quota::default()),
            tick_bytes: AtomicU64::new(0),
            rate: Mutex::new(0.0),
            total_session: AtomicU64::new(0),
            pace_next: Mutex::new(Instant::now()),
        }
    }

    pub fn net(&self) -> NetInfo {
        self.net.read().unwrap().clone()
    }
    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn is_present(&self) -> bool {
        self.net.read().unwrap().present
    }

    pub fn used(&self) -> u64 {
        self.quota.lock().unwrap().used
    }
    pub fn set_used(&self, v: u64) {
        self.quota.lock().unwrap().used = v;
    }
    pub fn reset_quota(&self) {
        let mut q = self.quota.lock().unwrap();
        q.used = 0;
    }

    pub fn exhausted(&self) -> bool {
        let limit = self.limit.load(Ordering::Relaxed);
        limit != 0 && self.used() >= limit
    }

    pub fn reserve(&self, want: u64) -> u64 {
        let limit = self.limit.load(Ordering::Relaxed);
        let mut q = self.quota.lock().unwrap();
        let grant = if limit == 0 {
            want
        } else {
            want.min(limit.saturating_sub(q.used + q.reserved))
        };
        q.reserved += grant;
        grant
    }

    pub fn commit(&self, n: u64) {
        {
            let mut q = self.quota.lock().unwrap();
            q.used += n;
            q.reserved = q.reserved.saturating_sub(n);
        }
        self.tick_bytes.fetch_add(n, Ordering::Relaxed);
        self.total_session.fetch_add(n, Ordering::Relaxed);
    }

    pub fn release(&self, n: u64) {
        let mut q = self.quota.lock().unwrap();
        q.reserved = q.reserved.saturating_sub(n);
    }

    pub async fn pace(&self, n: usize) {
        let cap = self.speed_cap.load(Ordering::Relaxed);
        if cap == 0 {
            return;
        }
        let wait_until = {
            let mut next = self.pace_next.lock().unwrap();
            let now = Instant::now();
            let burst = Duration::from_millis(150);
            let start = if *next + burst < now { now - burst } else { *next };
            let end = start + Duration::from_secs_f64(n as f64 / cap as f64);
            *next = end;
            end
        };
        let now = Instant::now();
        if wait_until > now + Duration::from_millis(1) {
            tokio::time::sleep(wait_until - now).await;
        }
    }

    pub fn current_rate(&self) -> f64 {
        *self.rate.lock().unwrap()
    }
}
