use super::link::Link;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use tokio_util::sync::CancellationToken;

pub const MIN_STEAL: u64 = 256 * 1024;
pub const MAP_CELLS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Probing,
    Downloading,
    Paused,

    Waiting,
    Completed,
    Error,
}

#[derive(Clone, Debug)]
pub struct Chunk {
    #[allow(dead_code)]
    pub start: u64,

    pub cur: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoneRange {
    pub start: u64,
    pub end: u64,
    pub slot: u8,
}

#[derive(Default, Clone)]
pub struct LinkFail {
    pub consecutive: u32,
    pub dead: bool,
    pub dead_at: Option<std::time::Instant>,
    pub msg: Option<String>,
}

pub struct JobInner {
    pub status: Status,
    pub error: Option<String>,
    pub note: Option<String>,
    pub url: String,
    pub final_url: String,
    pub dir: PathBuf,
    pub name: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub ranges: bool,
    pub probed: bool,
    pub validator: Option<String>,
    pub pending: VecDeque<(u64, u64)>,
    pub active: HashMap<u32, Chunk>,
    pub done: Vec<DoneRange>,
    pub downloaded: u64,
    pub per_link: HashMap<u8, u64>,
    pub fails: HashMap<String, LinkFail>,
    pub speed: f64,
    pub created: u64,
    pub next_wid: u32,
}

pub struct Job {
    pub id: u64,
    pub inner: Mutex<JobInner>,
    pub tick: AtomicU64,
    pub cancel: Mutex<Option<CancellationToken>>,
}

pub enum Claim {
    Got { start: u64, end: u64, reserved: u64 },
    Done,
    Wait,
    Quota,
}

#[derive(Serialize, Deserialize)]
pub struct JobFile {
    pub id: u64,
    pub url: String,
    pub final_url: String,
    pub dir: String,
    pub name: String,
    pub dest: String,
    pub size: Option<u64>,
    pub ranges: bool,
    pub probed: bool,
    pub validator: Option<String>,
    pub done: Vec<DoneRange>,
    pub per_link: Vec<(u8, u64)>,
    pub completed: bool,
    pub error: Option<String>,
    pub created: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Cell {
    pub slot: u8,
    pub fill: f32,
    pub active: bool,
}

impl JobInner {
    pub fn complete(&self) -> bool {
        matches!(self.size, Some(s) if self.downloaded >= s)
    }

    pub fn rebuild_pending(&mut self) {
        self.pending.clear();
        self.active.clear();
        let size = match self.size {
            Some(s) => s,
            None => return,
        };
        let mut cur = 0u64;
        for r in &self.done {
            if r.start > cur {
                self.pending.push_back((cur, r.start));
            }
            cur = cur.max(r.end);
        }
        if cur < size {
            self.pending.push_back((cur, size));
        }
    }

    pub fn insert_done(&mut self, s: u64, e: u64, slot: u8) -> u64 {
        if e <= s {
            return 0;
        }
        let v = &mut self.done;
        let mut gaps: Vec<(u64, u64)> = Vec::new();
        let mut cur = s;
        let mut k = v.partition_point(|r| r.end <= s);
        while k < v.len() && v[k].start < e {
            if v[k].start > cur {
                gaps.push((cur, v[k].start));
            }
            cur = cur.max(v[k].end);
            k += 1;
        }
        if cur < e {
            gaps.push((cur, e));
        }
        let mut added = 0;
        for (gs, ge) in gaps {
            added += ge - gs;
            let mut pos = v.partition_point(|r| r.start < gs);
            v.insert(pos, DoneRange { start: gs, end: ge, slot });
            if pos + 1 < v.len() && v[pos].end == v[pos + 1].start && v[pos].slot == v[pos + 1].slot {
                v[pos].end = v[pos + 1].end;
                v.remove(pos + 1);
            }
            if pos > 0 && v[pos - 1].end == v[pos].start && v[pos - 1].slot == v[pos].slot {
                v[pos - 1].end = v[pos].end;
                v.remove(pos);
                pos -= 1;
            }
            let _ = pos;
        }
        self.downloaded += added;
        *self.per_link.entry(slot).or_default() += added;
        added
    }

    pub fn map(&self) -> Vec<Cell> {
        let mut cells = vec![Cell::default(); MAP_CELLS];
        let size = match self.size {
            Some(s) if s > 0 => s,
            _ => return cells,
        };
        let cs = size.div_ceil(MAP_CELLS as u64).max(1);
        let mut acc = vec![[0u64; 8]; MAP_CELLS];
        for r in &self.done {
            let c0 = (r.start / cs) as usize;
            let c1 = ((r.end - 1) / cs) as usize;
            for c in c0..=c1.min(MAP_CELLS - 1) {
                let cstart = c as u64 * cs;
                let cend = cstart + cs;
                let ov = r.end.min(cend).saturating_sub(r.start.max(cstart));
                acc[c][(r.slot as usize) % 8] += ov;
            }
        }
        for (c, cell) in cells.iter_mut().enumerate() {
            let cstart = c as u64 * cs;
            let cend = (cstart + cs).min(size);
            let width = (cend - cstart).max(1);
            let total: u64 = acc[c].iter().sum();
            let (slot, _) = acc[c].iter().enumerate().max_by_key(|(_, b)| **b).unwrap();
            cell.slot = slot as u8;
            cell.fill = (total as f32 / width as f32).min(1.0);
        }
        for ch in self.active.values() {
            if ch.end <= ch.cur {
                continue;
            }
            let c0 = (ch.cur / cs) as usize;
            let c1 = ((ch.end.min(size) - 1) / cs) as usize;
            for c in c0..=c1.min(MAP_CELLS - 1) {
                cells[c].active = true;
            }
        }
        cells
    }

    pub fn to_file(&self, id: u64) -> JobFile {
        JobFile {
            id,
            url: self.url.clone(),
            final_url: self.final_url.clone(),
            dir: self.dir.to_string_lossy().into(),
            name: self.name.clone(),
            dest: self.dest.to_string_lossy().into(),
            size: self.size,
            ranges: self.ranges,
            probed: self.probed,
            validator: self.validator.clone(),
            done: self.done.clone(),
            per_link: self.per_link.iter().map(|(k, v)| (*k, *v)).collect(),
            completed: self.status == Status::Completed,
            error: self.error.clone(),
            created: self.created,
        }
    }
}

impl Job {
    pub fn new(id: u64, url: String, dir: PathBuf, name_guess: String) -> Self {
        let dest = dir.join(&name_guess);
        Job {
            id,
            tick: AtomicU64::new(0),
            cancel: Mutex::new(None),
            inner: Mutex::new(JobInner {
                status: Status::Paused,
                error: None,
                note: None,
                final_url: url.clone(),
                url,
                dir,
                name: name_guess,
                dest,
                size: None,
                ranges: false,
                probed: false,
                validator: None,
                pending: VecDeque::new(),
                active: HashMap::new(),
                done: Vec::new(),
                downloaded: 0,
                per_link: HashMap::new(),
                fails: HashMap::new(),
                speed: 0.0,
                created: now_secs(),
                next_wid: 0,
            }),
        }
    }

    pub fn from_file(f: JobFile) -> Self {
        let mut done = f.done;
        done.sort_by_key(|r| r.start);
        let downloaded = done.iter().map(|r| r.end - r.start).sum();
        let status = if f.completed {
            Status::Completed
        } else if f.error.is_some() {
            Status::Error
        } else {
            Status::Paused
        };
        let mut inner = JobInner {
            status,
            error: f.error,
            note: None,
            url: f.url,
            final_url: f.final_url,
            dir: PathBuf::from(f.dir),
            name: f.name,
            dest: PathBuf::from(f.dest),
            size: f.size,
            ranges: f.ranges,
            probed: f.probed,
            validator: f.validator,
            pending: VecDeque::new(),
            active: HashMap::new(),
            done,
            downloaded,
            per_link: f.per_link.into_iter().collect(),
            fails: HashMap::new(),
            speed: 0.0,
            created: f.created,
            next_wid: 0,
        };
        inner.rebuild_pending();
        Job {
            id: f.id,
            tick: AtomicU64::new(0),
            cancel: Mutex::new(None),
            inner: Mutex::new(inner),
        }
    }

    pub fn part_path(&self) -> PathBuf {
        let g = self.inner.lock().unwrap();
        part_of(&g.dest)
    }

    pub fn new_wid(&self) -> u32 {
        let mut g = self.inner.lock().unwrap();
        g.next_wid += 1;
        g.next_wid
    }

    pub fn claim(&self, wid: u32, link: &Link, chunk: u64) -> Claim {
        let mut g = self.inner.lock().unwrap();
        if g.complete() {
            return Claim::Done;
        }
        if g.status == Status::Error {
            return Claim::Done;
        }

        if !g.ranges {
            if !g.active.is_empty() {
                return Claim::Wait;
            }
            let end = g.size.unwrap_or(u64::MAX);
            let mut reserved = 0;
            if end != u64::MAX {
                reserved = link.reserve(end);
                if reserved < end {
                    link.release(reserved);
                    return Claim::Quota;
                }
            } else if link.exhausted() {
                return Claim::Quota;
            }
            g.done.clear();
            g.downloaded = 0;
            g.per_link.clear();
            g.pending.clear();
            g.active.insert(wid, Chunk { start: 0, cur: 0, end });
            return Claim::Got { start: 0, end, reserved };
        }

        let mut victim: Option<u32> = None;
        let (s, e) = if let Some(&(s, e)) = g.pending.front() {
            (s, e.min(s + chunk))
        } else {
            let mut best: Option<(u32, u64)> = None;
            for (id, ch) in g.active.iter() {
                let rem = ch.end.saturating_sub(ch.cur);
                if rem >= 2 * MIN_STEAL && best.map_or(true, |(_, r)| rem > r) {
                    best = Some((*id, rem));
                }
            }
            match best {
                Some((id, rem)) => {
                    let ch = &g.active[&id];
                    let mid = (ch.cur + rem / 2) & !4095u64;
                    victim = Some(id);
                    (mid.max(ch.cur + 4096), ch.end)
                }
                None => return Claim::Wait,
            }
        };
        let len = e - s;
        let got = link.reserve(len);
        if got == 0 {
            return Claim::Quota;
        }
        match victim {
            None => {
                let front = g.pending.front_mut().unwrap();
                if front.0 + got >= front.1 {
                    g.pending.pop_front();
                } else {
                    front.0 += got;
                }
            }
            Some(v) => {
                let old_end = g.active[&v].end;
                g.active.get_mut(&v).unwrap().end = s;
                if s + got < old_end {
                    g.pending.push_back((s + got, old_end));
                }
            }
        }
        g.active.insert(wid, Chunk { start: s, cur: s, end: s + got });
        Claim::Got { start: s, end: s + got, reserved: got }
    }

    pub fn finish_chunk(&self, wid: u32, success: bool) {
        let mut g = self.inner.lock().unwrap();
        if let Some(ch) = g.active.remove(&wid) {
            if !g.ranges {

                if success {
                    return;
                }
                g.done.clear();
                g.downloaded = 0;
                g.per_link.clear();
            } else if !success && ch.cur < ch.end && ch.end != u64::MAX {
                g.pending.push_front((ch.cur, ch.end));
            }
        }
    }
}

pub fn part_of(dest: &std::path::Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".ldpart");
    PathBuf::from(s)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
