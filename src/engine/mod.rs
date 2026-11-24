pub mod discover;
pub mod http;
pub mod job;
pub mod link;

use anyhow::Result;
use futures_util::FutureExt;
use http::{HttpClient, make_client};
use hyper::header;
use job::{Cell, Claim, Job, JobFile, Status, part_of};
use link::{Link, LinkKind, NetInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::net::Ipv4Addr;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const FLUSH: usize = 256 * 1024;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub download_dir: String,
    pub conns_per_link: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct LinkCfg {
    key: String,
    name: String,
    slot: usize,
    enabled: bool,
    limit: u64,
    used: u64,
    speed_cap: u64,
}

#[derive(Serialize, Deserialize)]
struct Config {
    settings: Settings,
    links: Vec<LinkCfg>,
}

fn default_download_dir() -> String {
    std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Downloads"))
        .unwrap_or_else(|_| ".".into())
}

#[derive(Clone)]
pub struct LinkView {
    pub key: String,
    pub slot: usize,
    pub name: String,
    pub ip: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub kind: LinkKind,
    pub desc: String,
    pub present: bool,
    pub enabled: bool,
    pub limit: u64,
    pub used: u64,
    pub speed_cap: u64,
    pub rate: f64,
    pub exhausted: bool,
}

#[derive(Clone)]
pub struct JobView {
    pub id: u64,
    pub name: String,
    pub status: Status,
    pub error: Option<String>,
    pub note: Option<String>,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub cells: Vec<Cell>,
    pub per_link: Vec<(u8, u64)>,
    pub dest: PathBuf,
    pub active: usize,
    pub dead: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct View {
    pub links: Vec<LinkView>,
    pub jobs: Vec<JobView>,
    pub total_rate: f64,
    pub settings: Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobAction {
    Pause,
    Resume,
    Remove,
}

pub struct Engine {
    pub rt: tokio::runtime::Handle,
    dir: PathBuf,
    pub settings: Mutex<Settings>,
    pub links: RwLock<Vec<Arc<Link>>>,
    pub jobs: RwLock<Vec<Arc<Job>>>,
    next_id: AtomicU64,
    dirty: AtomicBool,
    total_rate: Mutex<f64>,
}

enum ChunkErr {
    Cancelled,

    Stopped,
    Retry(String),
    Fatal(String),
    Disk(String),
}

impl Engine {
    pub fn new(rt: tokio::runtime::Handle) -> Arc<Engine> {
        let dir = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()))
            .join("LetsDownload");
        let _ = std::fs::create_dir_all(&dir);

        let cfg: Option<Config> = std::fs::read(dir.join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let settings = cfg
            .as_ref()
            .map(|c| c.settings.clone())
            .unwrap_or(Settings { download_dir: default_download_dir(), conns_per_link: 4 });

        let eng = Arc::new(Engine {
            rt,
            dir: dir.clone(),
            settings: Mutex::new(settings),
            links: RwLock::new(Vec::new()),
            jobs: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
            total_rate: Mutex::new(0.0),
        });

        if let Some(cfg) = cfg {
            let mut links = eng.links.write().unwrap();
            for c in cfg.links {
                let l = Link::new(
                    c.key,
                    c.slot,
                    c.name,
                    NetInfo {
                        ip: Ipv4Addr::UNSPECIFIED,
                        if_index: 0,
                        gateway: None,
                        kind: LinkKind::Other,
                        description: String::new(),
                        present: false,
                    },
                );
                l.enabled.store(c.enabled, Ordering::Relaxed);
                l.limit.store(c.limit, Ordering::Relaxed);
                l.speed_cap.store(c.speed_cap, Ordering::Relaxed);
                l.set_used(c.used);
                links.push(Arc::new(l));
            }
        }
        eng.refresh_links();

        if let Ok(bytes) = std::fs::read(dir.join("jobs.json")) {
            if let Ok(files) = serde_json::from_slice::<Vec<JobFile>>(&bytes) {
                let mut jobs = eng.jobs.write().unwrap();
                for f in files {
                    eng.next_id.fetch_max(f.id + 1, Ordering::Relaxed);
                    jobs.push(Arc::new(Job::from_file(f)));
                }
            }
        }

        let e2 = eng.clone();
        eng.rt.spawn(async move { e2.ticker().await });
        eng
    }

    pub fn refresh_links(&self) {
        let mut found = discover::discover();
        found.extend(crate::dev::fake_links());
        let mut links = self.links.write().unwrap();
        let mut changed = false;
        for f in &found {
            if let Some(l) = links.iter().find(|l| l.key == f.key) {
                let old = l.net();
                if old != f.net {
                    *l.net.write().unwrap() = f.net.clone();
                    changed = true;
                }
                *l.name.write().unwrap() = f.name.clone();
            } else {
                let slot = if f.net.kind == LinkKind::Virtual {
                    links.iter().map(|l| l.slot + 1).filter(|s| *s >= 100).max().unwrap_or(100)
                } else {
                    links.iter().map(|l| l.slot + 1).filter(|s| *s < 100).max().unwrap_or(0)
                };
                let l = Link::new(f.key.clone(), slot, f.name.clone(), f.net.clone());
                l.enabled.store(
                    f.net.kind != LinkKind::Virtual && crate::dev::link_allowed(&f.key),
                    Ordering::Relaxed,
                );
                links.push(Arc::new(l));
                changed = true;
            }
        }
        for l in links.iter() {
            if !found.iter().any(|f| f.key == l.key) && l.is_present() {
                l.net.write().unwrap().present = false;
                changed = true;
            }
        }
        let any_active = links.iter().any(|l| l.is_present() && l.is_enabled());
        if !any_active {
            for l in links.iter() {
                if l.is_present() && l.net().kind != LinkKind::Virtual && crate::dev::link_allowed(&l.key) {
                    l.enabled.store(true, Ordering::Relaxed);
                    changed = true;
                }
            }
        }
        let any_active = links.iter().any(|l| l.is_present() && l.is_enabled());
        if !any_active && !crate::dev::fake_mode() {
            if let Some(l) = links.iter().find(|l| l.is_present() && crate::dev::link_allowed(&l.key)) {
                l.enabled.store(true, Ordering::Relaxed);
                changed = true;
            } else if let Some(l) = links.iter().find(|l| l.key == "system-default") {
                if !l.is_present() || !l.is_enabled() {
                    l.net.write().unwrap().present = true;
                    l.enabled.store(true, Ordering::Relaxed);
                    changed = true;
                }
            } else {
                let slot = links.iter().map(|l| l.slot + 1).filter(|s| *s < 100).max().unwrap_or(0);
                let l = Link::new(
                    "system-default".into(),
                    slot,
                    "Sistem Ağ Bağlantısı".into(),
                    NetInfo {
                        ip: Ipv4Addr::UNSPECIFIED,
                        if_index: 0,
                        gateway: None,
                        kind: LinkKind::Other,
                        description: "Varsayılan sistem bağlantısı".into(),
                        present: true,
                    },
                );
                links.push(Arc::new(l));
                changed = true;
            }
        } else if found.iter().any(|f| f.net.present) {
            if let Some(l) = links.iter().find(|l| l.key == "system-default") {
                if l.is_present() {
                    l.net.write().unwrap().present = false;
                    changed = true;
                }
            }
        }
        if changed {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn link(&self, key: &str) -> Option<Arc<Link>> {
        self.links.read().unwrap().iter().find(|l| l.key == key).cloned()
    }

    pub fn set_link_enabled(&self, key: &str, on: bool) {
        if let Some(l) = self.link(key) {
            l.enabled.store(on, Ordering::Relaxed);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
    pub fn set_link_limit(&self, key: &str, bytes: u64) {
        if let Some(l) = self.link(key) {
            l.limit.store(bytes, Ordering::Relaxed);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
    pub fn set_link_speed_cap(&self, key: &str, bps: u64) {
        if let Some(l) = self.link(key) {
            l.speed_cap.store(bps, Ordering::Relaxed);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
    pub fn reset_quota(&self, key: &str) {
        if let Some(l) = self.link(key) {
            l.reset_quota();
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn set_settings(&self, s: Settings) {
        *self.settings.lock().unwrap() = s;
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn add_urls(self: &Arc<Self>, text: &str) -> usize {
        let dir = PathBuf::from(self.settings.lock().unwrap().download_dir.clone());
        let mut n = 0;
        for line in text.split_whitespace() {
            let mut u = line.trim().to_string();
            if !u.contains("://") {
                u = format!("https://{u}");
            }
            let parsed = match url::Url::parse(&u) {
                Ok(p) if matches!(p.scheme(), "http" | "https") && p.host().is_some() => p,
                _ => continue,
            };
            let guess = name_from_url(&parsed);
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let job = Arc::new(Job::new(id, u, dir.clone(), guess));
            self.jobs.write().unwrap().push(job.clone());
            self.start_job(&job);
            n += 1;
        }
        self.dirty.store(true, Ordering::Relaxed);
        n
    }

    fn is_running(job: &Job) -> bool {
        matches!(
            job.inner.lock().unwrap().status,
            Status::Probing | Status::Downloading | Status::Waiting
        )
    }

    pub fn start_job(self: &Arc<Self>, job: &Arc<Job>) {
        if Self::is_running(job) {
            return;
        }
        let token = CancellationToken::new();
        {
            let mut c = job.cancel.lock().unwrap();
            if let Some(old) = c.take() {
                old.cancel();
            }
            *c = Some(token.clone());
        }
        {
            let mut g = job.inner.lock().unwrap();
            g.status = if g.probed { Status::Downloading } else { Status::Probing };
            g.error = None;
            g.note = None;
            g.fails.clear();
        }
        let eng = self.clone();
        let job = job.clone();
        self.rt.spawn(async move {
            let run = AssertUnwindSafe(eng.clone().run_job(job.clone(), token)).catch_unwind();
            if run.await.is_err() {
                eng.set_error(&job, "Beklenmeyen iç hata".into());
            }
        });
    }

    pub fn pause_job(&self, job: &Arc<Job>) {
        if let Some(t) = job.cancel.lock().unwrap().as_ref() {
            t.cancel();
        }
        let mut g = job.inner.lock().unwrap();
        if matches!(g.status, Status::Probing | Status::Downloading | Status::Waiting) {
            g.status = Status::Paused;
            g.speed = 0.0;
            g.note = None;
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn job_action(self: &Arc<Self>, id: u64, action: JobAction) {
        let job = match self.jobs.read().unwrap().iter().find(|j| j.id == id).cloned() {
            Some(j) => j,
            None => return,
        };
        match action {
            JobAction::Pause => self.pause_job(&job),
            JobAction::Resume => self.start_job(&job),
            JobAction::Remove => {
                self.pause_job(&job);
                let (dest, completed) = {
                    let g = job.inner.lock().unwrap();
                    (g.dest.clone(), g.status == Status::Completed)
                };
                if !completed {
                    let part = part_of(&dest);
                    self.rt.spawn(async move {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        let _ = tokio::fs::remove_file(part).await;
                    });
                }
                self.jobs.write().unwrap().retain(|j| j.id != id);
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn pause_all(&self) {
        for j in self.jobs.read().unwrap().iter() {
            self.pause_job(j);
        }
    }
    pub fn resume_all(self: &Arc<Self>) {
        let jobs: Vec<_> = self.jobs.read().unwrap().clone();
        for j in jobs {
            let st = j.inner.lock().unwrap().status;
            if matches!(st, Status::Paused | Status::Error) {
                self.start_job(&j);
            }
        }
    }

    pub fn shutdown(&self) {
        self.pause_all();
        self.save();
    }

    pub fn view(&self) -> View {
        let links = self
            .links
            .read()
            .unwrap()
            .iter()
            .map(|l| {
                let net = l.net();
                LinkView {
                    key: l.key.clone(),
                    slot: l.slot,
                    name: l.name(),
                    ip: net.ip,
                    gateway: net.gateway,
                    kind: net.kind,
                    desc: net.description.clone(),
                    present: net.present,
                    enabled: l.is_enabled(),
                    limit: l.limit.load(Ordering::Relaxed),
                    used: l.used(),
                    speed_cap: l.speed_cap.load(Ordering::Relaxed),
                    rate: l.current_rate(),
                    exhausted: l.exhausted(),
                }
            })
            .collect();
        let jobs = self
            .jobs
            .read()
            .unwrap()
            .iter()
            .map(|j| {
                let g = j.inner.lock().unwrap();
                let remaining = g.size.map(|s| s.saturating_sub(g.downloaded));
                let eta = match (remaining, g.speed) {
                    (Some(r), s) if s > 1024.0 => Some((r as f64 / s) as u64),
                    _ => None,
                };
                let mut per_link: Vec<(u8, u64)> = g.per_link.iter().map(|(k, v)| (*k, *v)).collect();
                per_link.sort();
                JobView {
                    id: j.id,
                    name: g.name.clone(),
                    status: g.status,
                    error: g.error.clone(),
                    note: g.note.clone(),
                    size: g.size,
                    downloaded: g.downloaded,
                    speed: g.speed,
                    eta,
                    cells: g.map(),
                    per_link,
                    dest: g.dest.clone(),
                    active: g.active.len(),
                    dead: g
                        .fails
                        .iter()
                        .filter(|(_, f)| f.dead)
                        .map(|(k, f)| {
                            let name = self
                                .link(k)
                                .map(|l| l.name())
                                .unwrap_or_else(|| k.clone());
                            (name, f.msg.clone().unwrap_or_default())
                        })
                        .collect(),
                }
            })
            .collect();
        View {
            links,
            jobs,
            total_rate: *self.total_rate.lock().unwrap(),
            settings: self.settings.lock().unwrap().clone(),
        }
    }

    async fn ticker(self: Arc<Self>) {
        let mut last = Instant::now();
        let mut n: u64 = 0;
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        loop {
            iv.tick().await;
            n += 1;
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64().max(0.001);
            last = now;

            let mut total = 0.0;
            let mut any = false;
            for l in self.links.read().unwrap().iter() {
                let b = l.tick_bytes.swap(0, Ordering::Relaxed);
                if b > 0 {
                    any = true;
                }
                let inst = b as f64 / dt;
                let mut r = l.rate.lock().unwrap();
                *r = if inst > *r { *r * 0.5 + inst * 0.5 } else { *r * 0.8 + inst * 0.2 };
                if *r < 200.0 {
                    *r = 0.0;
                }
                total += *r;
            }
            *self.total_rate.lock().unwrap() = total;
            for j in self.jobs.read().unwrap().iter() {
                let b = j.tick.swap(0, Ordering::Relaxed);
                let inst = b as f64 / dt;
                let mut g = j.inner.lock().unwrap();
                g.speed = if inst > g.speed { g.speed * 0.5 + inst * 0.5 } else { g.speed * 0.85 + inst * 0.15 };
                if g.speed < 200.0 {
                    g.speed = 0.0;
                }
            }
            if any {
                self.dirty.store(true, Ordering::Relaxed);
            }
            if n % 12 == 0 {
                let e = self.clone();
                let _ = tokio::task::spawn_blocking(move || e.refresh_links()).await;
            }
            if n % 8 == 0 && self.dirty.swap(false, Ordering::Relaxed) {
                let e = self.clone();
                let _ = tokio::task::spawn_blocking(move || e.save()).await;
            }
        }
    }

    pub fn save(&self) {
        let cfg = Config {
            settings: self.settings.lock().unwrap().clone(),
            links: self
                .links
                .read()
                .unwrap()
                .iter()
                .map(|l| LinkCfg {
                    key: l.key.clone(),
                    name: l.name(),
                    slot: l.slot,
                    enabled: l.is_enabled(),
                    limit: l.limit.load(Ordering::Relaxed),
                    used: l.used(),
                    speed_cap: l.speed_cap.load(Ordering::Relaxed),
                })
                .filter(|c| !crate::dev::is_fake(&c.key) && c.key != "system-default")
                .collect(),
        };
        let files: Vec<JobFile> = self
            .jobs
            .read()
            .unwrap()
            .iter()
            .map(|j| j.inner.lock().unwrap().to_file(j.id))
            .collect();
        write_atomic(&self.dir.join("config.json"), &serde_json::to_vec_pretty(&cfg).unwrap());
        write_atomic(&self.dir.join("jobs.json"), &serde_json::to_vec(&files).unwrap());
    }

    fn set_error(&self, job: &Job, msg: String) {
        let mut g = job.inner.lock().unwrap();
        g.status = Status::Error;
        g.error = Some(msg);
        g.speed = 0.0;
        self.dirty.store(true, Ordering::Relaxed);
    }

    async fn run_job(self: Arc<Self>, job: Arc<Job>, token: CancellationToken) {
        let probed = job.inner.lock().unwrap().probed;
        if !probed {
            let r = tokio::select! {
                _ = token.cancelled() => return,
                r = self.probe(&job) => r,
            };
            if token.is_cancelled() {
                return;
            }
            if let Err(e) = r {
                self.set_error(&job, e.to_string());
                return;
            }
        }
        if let Err(e) = self.prepare_file(&job).await {
            self.set_error(&job, format!("Dosya hazırlanamadı: {e}"));
            return;
        }
        {
            let mut g = job.inner.lock().unwrap();
            g.rebuild_pending();
            g.status = Status::Downloading;
        }
        let part = job.part_path();

        let mut set: JoinSet<String> = JoinSet::new();
        let mut running: HashMap<String, u32> = HashMap::new();
        loop {
            while let Some(res) = set.try_join_next() {
                if let Ok(key) = res {
                    if let Some(c) = running.get_mut(&key) {
                        *c = c.saturating_sub(1);
                    }
                }
            }
            if token.is_cancelled() {
                while set.join_next().await.is_some() {}
                return;
            }

            let (ranges, complete, active_any) = {
                let g = job.inner.lock().unwrap();
                (g.ranges, g.complete(), !g.active.is_empty())
            };
            if complete {
                while set.join_next().await.is_some() {}
                self.finalize(&job).await;
                return;
            }

            let conns = self.settings.lock().unwrap().conns_per_link.clamp(1, 16);
            let links: Vec<Arc<Link>> = self.links.read().unwrap().clone();
            let active_link_count = links
                .iter()
                .filter(|l| l.is_enabled() && l.is_present())
                .count();
            let allow_fallback = active_link_count <= 1;
            {
                let mut g = job.inner.lock().unwrap();
                for f in g.fails.values_mut() {
                    if f.dead && f.dead_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(12)) {
                        f.dead = false;
                        f.dead_at = None;
                        f.consecutive = 0;
                    }
                }
            }
            let mut total_workers: u32 = running.values().sum();
            for l in &links {
                if !(l.is_enabled() && l.is_present()) {
                    continue;
                }
                if job.inner.lock().unwrap().fails.get(&l.key).is_some_and(|f| f.dead) {
                    continue;
                }
                let want = if ranges { conns } else { 1 };
                loop {
                    let have = *running.get(&l.key).unwrap_or(&0);
                    if have >= want || (!ranges && total_workers >= 1) || l.exhausted() {
                        break;
                    }
                    *running.entry(l.key.clone()).or_default() += 1;
                    total_workers += 1;
                    let e = self.clone();
                    let (j, lk, tk, p) = (job.clone(), l.clone(), token.clone(), part.clone());
                    set.spawn(async move {
                        let key = lk.key.clone();
                        let failed_job = j.clone();
                        match AssertUnwindSafe(e.worker(j, lk, tk, p, allow_fallback)).catch_unwind().await {
                            Ok(k) => k,
                            Err(_) => {
                                let mut g = failed_job.inner.lock().unwrap_or_else(|e| e.into_inner());
                                let f = g.fails.entry(key.clone()).or_default();
                                f.dead = true;
                                f.dead_at = Some(Instant::now());
                                f.msg = Some("iç hata".into());
                                key
                            }
                        }
                    });
                }
            }

            let mut fatal = false;
            {
                let mut g = job.inner.lock().unwrap();
                let usable: Vec<&Arc<Link>> = links
                    .iter()
                    .filter(|l| l.is_enabled() && l.is_present())
                    .collect();
                let alive: Vec<&&Arc<Link>> = usable
                    .iter()
                    .filter(|l| !g.fails.get(&l.key).is_some_and(|f| f.dead))
                    .collect();
                if active_any {
                    g.status = Status::Downloading;
                    g.note = None;
                } else if alive.is_empty() {
                    if usable.is_empty() {
                        g.status = Status::Waiting;
                        g.note = Some("Etkin internet bağlantısı yok".into());
                    } else {
                        let msg = g
                            .fails
                            .values()
                            .find_map(|f| f.msg.clone())
                            .unwrap_or_else(|| "bilinmeyen hata".into());
                        g.status = Status::Error;
                        g.error = Some(format!("Hiçbir bağlantı dosyayı indiremedi: {msg}"));
                        g.speed = 0.0;
                        fatal = true;
                    }
                } else if alive.iter().all(|l| l.exhausted()) {
                    g.status = Status::Waiting;
                    g.note = Some("Tüm bağlantıların kotası doldu".into());
                } else {
                    g.status = Status::Downloading;
                    g.note = None;
                }
            }
            if fatal {
                token.cancel();
                while set.join_next().await.is_some() {}
                self.dirty.store(true, Ordering::Relaxed);
                return;
            }

            tokio::select! {
                _ = token.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
    }

    async fn finalize(&self, job: &Arc<Job>) {
        let (part, dest) = {
            let g = job.inner.lock().unwrap();
            (part_of(&g.dest), g.dest.clone())
        };
        let mut dest = dest;
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
            let name = dest.file_name().unwrap().to_string_lossy().to_string();
            dest = unique_path(&dir, &name);
        }
        match tokio::fs::rename(&part, &dest).await {
            Ok(()) => {
                let mut g = job.inner.lock().unwrap();
                g.dest = dest;
                g.status = Status::Completed;
                g.speed = 0.0;
                g.note = None;
            }
            Err(e) => self.set_error(job, format!("Dosya yeniden adlandırılamadı: {e}")),
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    async fn prepare_file(&self, job: &Arc<Job>) -> std::io::Result<()> {
        let (part, dir, size, had_done) = {
            let g = job.inner.lock().unwrap();
            (part_of(&g.dest), g.dir.clone(), g.size, !g.done.is_empty())
        };
        tokio::fs::create_dir_all(&dir).await?;
        let exists_len = tokio::fs::metadata(&part).await.ok().map(|m| m.len());
        if had_done && exists_len.is_none() {
            let mut g = job.inner.lock().unwrap();
            g.done.clear();
            g.downloaded = 0;
            g.per_link.clear();
        }
        let f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&part)
            .await?;
        if let Some(sz) = size {
            if exists_len != Some(sz) {
                f.set_len(sz).await?;
            }
        }
        Ok(())
    }

    async fn probe(&self, job: &Arc<Job>) -> Result<()> {
        self.refresh_links();
        let url = job.inner.lock().unwrap().url.clone();
        let links: Vec<Arc<Link>> = self.links.read().unwrap().clone();
        let mut last: anyhow::Error = anyhow::anyhow!("Kullanılabilir internet bağlantısı yok");
        let mut candidates: Vec<Arc<Link>> = links
            .iter()
            .filter(|l| l.is_enabled() && l.is_present())
            .cloned()
            .collect();
        if candidates.is_empty() {
            candidates = links.iter().filter(|l| l.is_present()).cloned().collect();
        }
        if candidates.is_empty() && !crate::dev::fake_mode() {
            candidates.push(Arc::new(Link::new(
                "system-default".into(),
                0,
                "Sistem Ağ Bağlantısı".into(),
                NetInfo {
                    ip: Ipv4Addr::UNSPECIFIED,
                    if_index: 0,
                    gateway: None,
                    kind: LinkKind::Other,
                    description: "Varsayılan sistem bağlantısı".into(),
                    present: true,
                },
            )));
        }
        let allow_fallback = candidates.len() <= 1;
        for l in &candidates {
            let client = make_client(l.clone(), allow_fallback || l.key == "system-default");
            let mut attempt = http::get(&client, &url, Some((0, 0)), None).await;
            if let Ok(ref f) = attempt {
                let st = f.resp.status().as_u16();
                if matches!(st, 400 | 405 | 416 | 501) {
                    attempt = match http::head(&client, &url).await {
                        Ok(hf) if hf.resp.status().is_success() => Ok(hf),
                        _ => http::get(&client, &url, None, None).await,
                    };
                }
            }
            match attempt {
                Ok(f) => {
                    let status = f.resp.status();
                    let h = f.resp.headers();
                    let total_from_range = h
                        .get(header::CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.rsplit('/').next())
                        .and_then(|s| s.trim().parse::<u64>().ok());
                    let clen = h
                        .get(header::CONTENT_LENGTH)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok());
                    let accept = h
                        .get(header::ACCEPT_RANGES)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|s| s.to_lowercase().contains("bytes"));
                    let (size, ranges) = if status.as_u16() == 206 {
                        (total_from_range, total_from_range.is_some())
                    } else if status.is_success() {
                        (clen, accept && clen.is_some())
                    } else {
                        last = anyhow::anyhow!("Sunucu HTTP {} döndürdü", status.as_u16());
                        if status.is_client_error() && !matches!(status.as_u16(), 403 | 429) {
                            return Err(last);
                        }
                        continue;
                    };
                    let validator = h
                        .get(header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| !s.starts_with("W/"))
                        .map(String::from)
                        .or_else(|| {
                            h.get(header::LAST_MODIFIED)
                                .and_then(|v| v.to_str().ok())
                                .map(String::from)
                        });
                    let cd_name = h
                        .get(header::CONTENT_DISPOSITION)
                        .and_then(|v| v.to_str().ok())
                        .and_then(content_disposition_name);
                    let final_url = f.final_url.clone();
                    drop(f);

                    if !l.is_enabled() {
                        l.enabled.store(true, Ordering::Relaxed);
                        self.dirty.store(true, Ordering::Relaxed);
                    }
                    let mut g = job.inner.lock().unwrap();
                    let name = match cd_name {
                        Some(n) => sanitize(&n),
                        None => url::Url::parse(&final_url)
                            .map(|u| name_from_url(&u))
                            .unwrap_or_else(|_| g.name.clone()),
                    };
                    let dest = unique_path(&g.dir, &name);
                    g.name = dest.file_name().unwrap().to_string_lossy().to_string();
                    g.dest = dest;
                    g.size = size;
                    g.ranges = ranges;
                    g.validator = validator;
                    g.final_url = final_url;
                    g.probed = true;
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn chunk_size(&self, job: &Job) -> u64 {
        let size = job.inner.lock().unwrap().size.unwrap_or(0);
        (size / 24).clamp(512 * 1024, 8 * 1024 * 1024)
    }

    async fn sleep_or_cancel(token: &CancellationToken, ms: u64) {
        tokio::select! {
            _ = token.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
        }
    }

    async fn worker(
        self: Arc<Self>,
        job: Arc<Job>,
        link: Arc<Link>,
        token: CancellationToken,
        part: PathBuf,
        allow_fallback: bool,
    ) -> String {
        let key = link.key.clone();
        let slot = link.slot as u8;
        let wid = job.new_wid();
        let mut file = match tokio::fs::OpenOptions::new().write(true).open(&part).await {
            Ok(f) => f,
            Err(e) => {
                self.set_error(&job, format!("Dosya açılamadı: {e}"));
                return key;
            }
        };
        let client = make_client(link.clone(), allow_fallback || key == "system-default");
        let mut backoff = 500u64;
        loop {
            if token.is_cancelled() {
                break;
            }
            if !link.is_enabled() || !link.is_present() {
                if !job.inner.lock().unwrap().ranges {
                    break;
                }
                Self::sleep_or_cancel(&token, 400).await;
                continue;
            }
            if job.inner.lock().unwrap().fails.get(&key).is_some_and(|f| f.dead) {
                break;
            }
            let cs = self.chunk_size(&job);
            match job.claim(wid, &link, cs) {
                Claim::Done => break,
                Claim::Wait => Self::sleep_or_cancel(&token, 300).await,
                Claim::Quota => {
                    if !job.inner.lock().unwrap().ranges {
                        let mut g = job.inner.lock().unwrap();
                        let f = g.fails.entry(key.clone()).or_default();
                        f.dead = true;
                        f.dead_at = None;
                        f.msg = Some("Kota bu dosya için yeterli değil".into());
                        break;
                    }
                    Self::sleep_or_cancel(&token, 300).await
                }
                Claim::Got { start, end, reserved } => {
                    let mut got = 0u64;
                    let res = self
                        .download_chunk(&job, &link, &client, wid, slot, &token, &mut file, start, end, &mut got)
                        .await;
                    link.release(reserved.saturating_sub(got));
                    job.finish_chunk(wid, res.is_ok());
                    match res {
                        Ok(()) => {
                            backoff = 500;
                            if let Some(f) = job.inner.lock().unwrap().fails.get_mut(&key) {
                                f.consecutive = 0;
                                f.dead = false;
                                f.dead_at = None;
                            }
                        }
                        Err(ChunkErr::Cancelled) => break,
                        Err(ChunkErr::Stopped) => {}
                        Err(ChunkErr::Disk(m)) => {
                            self.set_error(&job, m);
                            break;
                        }
                        Err(ChunkErr::Fatal(m)) => {
                            let mut g = job.inner.lock().unwrap();
                            let f = g.fails.entry(key.clone()).or_default();
                            f.dead = true;
                            f.dead_at = None;
                            f.msg = Some(m);
                            break;
                        }
                        Err(ChunkErr::Retry(m)) => {
                            let dead = {
                                let mut g = job.inner.lock().unwrap();
                                let f = g.fails.entry(key.clone()).or_default();
                                if got > 0 {
                                    f.consecutive = 0;
                                } else {
                                    f.consecutive += 1;
                                }
                                f.msg = Some(m);
                                if f.consecutive >= 8 {
                                    f.dead = true;
                                    f.dead_at = Some(Instant::now());
                                }
                                f.dead
                            };
                            if dead {
                                break;
                            }
                            Self::sleep_or_cancel(&token, backoff).await;
                            backoff = (backoff * 2).min(8000);
                        }
                    }
                }
            }
        }
        key
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_chunk(
        &self,
        job: &Arc<Job>,
        link: &Arc<Link>,
        client: &HttpClient,
        wid: u32,
        slot: u8,
        token: &CancellationToken,
        file: &mut tokio::fs::File,
        start: u64,
        end: u64,
        got: &mut u64,
    ) -> Result<(), ChunkErr> {
        let (url, orig_url, validator, ranges) = {
            let g = job.inner.lock().unwrap();
            (g.final_url.clone(), g.url.clone(), g.validator.clone(), g.ranges)
        };
        let range = if ranges { Some((start, end - 1)) } else { None };
        let mut fetched = tokio::select! {
            _ = token.cancelled() => return Err(ChunkErr::Cancelled),
            r = http::get(client, &url, range, validator.as_deref()) => {
                r.map_err(|e| ChunkErr::Retry(e.to_string()))?
            }
        };
        if matches!(fetched.resp.status().as_u16(), 401 | 403 | 410) && url != orig_url {
            if let Ok(re) = tokio::select! {
                _ = token.cancelled() => return Err(ChunkErr::Cancelled),
                r = http::get(client, &orig_url, range, validator.as_deref()) => r,
            } {
                if re.resp.status().is_success() || re.resp.status().as_u16() == 206 {
                    job.inner.lock().unwrap().final_url = re.final_url.clone();
                }
                fetched = re;
            }
        }
        let status = fetched.resp.status().as_u16();
        let ok = if ranges { status == 206 } else { status == 200 };
        if !ok {
            return Err(match status {
                408 | 429 | 500 | 502 | 503 | 504 => ChunkErr::Retry(format!("HTTP {status}")),
                200 if ranges => ChunkErr::Fatal(
                    "Sunucu aralık isteğini reddetti veya dosya değişmiş".into(),
                ),
                _ => ChunkErr::Fatal(format!("HTTP {status} (bu bağlantıdan erişim reddedildi)")),
            });
        }
        if ranges {
            let cr_start = fetched
                .resp
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("bytes "))
                .and_then(|s| s.split('-').next())
                .and_then(|s| s.trim().parse::<u64>().ok());
            if cr_start != Some(start) {
                return Err(ChunkErr::Fatal("Sunucu yanlış aralık döndürdü".into()));
            }
        }

        use http_body_util::BodyExt;
        let mut body = fetched.resp.into_body();
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH + 64 * 1024);
        let mut pos = start;

        loop {
            let frame = tokio::select! {
                _ = token.cancelled() => {
                    let _ = flush(job, wid, slot, file, &mut pos, &mut buf).await;
                    return Err(ChunkErr::Cancelled);
                }
                r = tokio::time::timeout(Duration::from_secs(25), body.frame()) => r,
            };
            match frame {
                Err(_) => {
                    flush(job, wid, slot, file, &mut pos, &mut buf).await?;
                    return Err(ChunkErr::Retry("veri akışı durdu (zaman aşımı)".into()));
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    flush(job, wid, slot, file, &mut pos, &mut buf).await?;
                    return Err(ChunkErr::Retry(e.to_string()));
                }
                Ok(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        let n = data.len() as u64;
                        link.commit(n);
                        job.tick.fetch_add(n, Ordering::Relaxed);
                        *got += n;
                        buf.extend_from_slice(&data);
                        link.pace(data.len()).await;
                        if buf.len() >= FLUSH {
                            if flush(job, wid, slot, file, &mut pos, &mut buf).await? {
                                return Ok(());
                            }
                        }
                        if !link.is_enabled() || !link.is_present() {
                            flush(job, wid, slot, file, &mut pos, &mut buf).await?;
                            return Err(ChunkErr::Stopped);
                        }
                        if !ranges && end == u64::MAX && link.exhausted() {
                            flush(job, wid, slot, file, &mut pos, &mut buf).await?;
                            return Err(ChunkErr::Stopped);
                        }
                    }
                }
            }
        }
        let finished = flush(job, wid, slot, file, &mut pos, &mut buf).await?;
        if finished {
            return Ok(());
        }

        let mut g = job.inner.lock().unwrap();
        if end == u64::MAX && g.size.is_none() {
            g.size = Some(g.downloaded);
            return Ok(());
        }
        let cur_end = g.active.get(&wid).map(|c| c.end).unwrap_or(end);
        if pos >= cur_end {
            Ok(())
        } else {
            Err(ChunkErr::Retry("bağlantı erken kapandı".into()))
        }
    }
}

async fn flush(
    job: &Arc<Job>,
    wid: u32,
    slot: u8,
    file: &mut tokio::fs::File,
    pos: &mut u64,
    buf: &mut Vec<u8>,
) -> Result<bool, ChunkErr> {
    if buf.is_empty() {
        let g = job.inner.lock().unwrap();
        return match g.active.get(&wid) {
            Some(c) => Ok(*pos >= c.end),
            None => Err(ChunkErr::Cancelled),
        };
    }
    let allowed = {
        let mut g = job.inner.lock().unwrap();
        let ch = g.active.get_mut(&wid).ok_or(ChunkErr::Cancelled)?;
        let room = ch.end.saturating_sub(*pos);
        let a = (buf.len() as u64).min(room);
        ch.cur = *pos + a;
        a as usize
    };
    if allowed > 0 {
        file.seek(SeekFrom::Start(*pos))
            .await
            .map_err(|e| ChunkErr::Disk(format!("Disk hatası: {e}")))?;
        file.write_all(&buf[..allowed])
            .await
            .map_err(|e| ChunkErr::Disk(format!("Diske yazılamadı: {e}")))?;
        let mut g = job.inner.lock().unwrap();
        g.insert_done(*pos, *pos + allowed as u64, slot);
        *pos += allowed as u64;
    }
    let truncated = allowed < buf.len();
    buf.clear();
    let g = job.inner.lock().unwrap();
    match g.active.get(&wid) {
        Some(c) => Ok(truncated || *pos >= c.end),
        None => Err(ChunkErr::Cancelled),
    }
}

fn write_atomic(path: &Path, data: &[u8]) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    s = s.trim().trim_end_matches(['.', ' ']).to_string();
    let stem = s.split('.').next().unwrap_or("").to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|r| *r == stem)
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        s = format!("_{s}");
    }
    if s.is_empty() {
        s = "indirme".into();
    }
    s.chars().take(150).collect()
}

fn name_from_url(u: &url::Url) -> String {
    let seg = u
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("")
        .to_string();
    let dec = percent_encoding::percent_decode_str(&seg).decode_utf8_lossy().to_string();
    if dec.is_empty() {
        sanitize(u.host_str().unwrap_or("indirme"))
    } else {
        sanitize(&dec)
    }
}

fn content_disposition_name(v: &str) -> Option<String> {
    let lower = v.to_lowercase();
    if let Some(i) = lower.find("filename*=") {
        let rest = &v[i + 10..];
        let val = rest.split(';').next()?.trim().trim_matches('"');
        let enc = val.splitn(3, '\'').nth(2).unwrap_or(val);
        return Some(percent_encoding::percent_decode_str(enc).decode_utf8_lossy().to_string());
    }
    let i = lower.find("filename=")?;
    let rest = &v[i + 9..];
    let val = rest.split(';').next()?.trim().trim_matches('"');
    if val.is_empty() { None } else { Some(val.to_string()) }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || part_of(p).exists();
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !taken(&p) {
            return p;
        }
    }
    first
}
