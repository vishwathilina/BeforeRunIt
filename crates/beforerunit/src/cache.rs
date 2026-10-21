//! AI verdict cache: an in-memory LRU in front of a small redb file, so
//! repeated commands get their review instantly, even after a restart.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use beforerunit_core::merge::AiVerdict;
use beforerunit_core::Level;
use redb::{Database, ReadableTable, TableDefinition};

const TABLE: TableDefinition<u64, &str> = TableDefinition::new("verdicts");
const TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MEM_CAP: u64 = 4096;

pub fn key(redacted_line: &str, cwd_name: &str, root: bool, model: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in [redacted_line.trim(), cwd_name, if root { "1" } else { "0" }, model] {
        for b in part.bytes().chain(std::iter::once(0)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn encode(v: &AiVerdict) -> String {
    format!("{}\t{}\t{:.3}\t{}", now(), v.level.as_str(), v.confidence, v.reason.replace(['\t', '\n'], " "))
}

fn decode(s: &str) -> Option<AiVerdict> {
    let mut it = s.splitn(4, '\t');
    let ts: u64 = it.next()?.parse().ok()?;
    if now().saturating_sub(ts) > TTL.as_secs() {
        return None;
    }
    let level = Level::parse(it.next()?)?;
    let confidence = it.next()?.parse().ok()?;
    Some(AiVerdict { level, confidence, reason: it.next()?.to_string() })
}

pub struct Cache {
    mem: moka::sync::Cache<u64, AiVerdict>,
    db: Option<Database>,
}

impl Cache {
    pub fn open() -> Cache {
        let mem = moka::sync::Cache::builder().max_capacity(MEM_CAP).time_to_live(TTL).build();
        let dir = beforerunit_core::store::cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let db = Database::create(dir.join("ai.redb")).ok();
        let c = Cache { mem, db };
        c.preload();
        c
    }

    fn preload(&self) {
        let Some(db) = &self.db else { return };
        let Ok(r) = db.begin_read() else { return };
        let Ok(t) = r.open_table(TABLE) else { return };
        let Ok(iter) = t.iter() else { return };
        for (k, v) in iter.flatten().take(MEM_CAP as usize) {
            if let Some(v) = decode(v.value()) {
                self.mem.insert(k.value(), v);
            }
        }
    }

    pub fn get_mem(&self, k: u64) -> Option<AiVerdict> {
        self.mem.get(&k)
    }

    pub fn get(&self, k: u64) -> Option<AiVerdict> {
        if let Some(v) = self.mem.get(&k) {
            return Some(v);
        }
        let db = self.db.as_ref()?;
        let r = db.begin_read().ok()?;
        let t = r.open_table(TABLE).ok()?;
        let v = decode(t.get(k).ok()??.value())?;
        self.mem.insert(k, v.clone());
        Some(v)
    }

    pub fn put(&self, k: u64, v: &AiVerdict) {
        self.mem.insert(k, v.clone());
        let Some(db) = &self.db else { return };
        let Ok(w) = db.begin_write() else { return };
        if let Ok(mut t) = w.open_table(TABLE) {
            let _ = t.insert(k, encode(v).as_str());
        }
        let _ = w.commit();
    }

    pub fn len(&self) -> u64 {
        self.mem.entry_count()
    }

    pub fn clear(&self) {
        self.mem.invalidate_all();
        if let Some(db) = &self.db {
            if let Ok(w) = db.begin_write() {
                let _ = w.delete_table(TABLE);
                let _ = w.commit();
            }
        }
    }
}
