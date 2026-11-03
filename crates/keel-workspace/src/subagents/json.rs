//! Bounded file cursors recover recent work immediately and backfill older child links gradually.
use crate::Real;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::io::{BufRead, Read, Seek};

pub(super) const ID_LIMIT: usize = 200;
pub(super) const LABEL_LIMIT: usize = 200;
pub(super) const READ_BYTES: u64 = 2 * 1024 * 1024;
pub(super) const HEADER_BYTES: u64 = 256 * 1024;
pub(super) const HISTORY_LIMIT: usize = 64;
pub(super) const CANDIDATE_LIMIT: usize = 256;
pub(super) const OUTPUT_BYTES: usize = 128 * 1024;
pub(super) const OUTPUT_LINES: usize = 256;

#[derive(Default)]
pub(super) struct Cursor {
    next: u64,
    backfill: u64,
    recent_start: u64,
    initialized: bool,
    pub recent_count: usize,
    replayed: bool,
    initial_end: u64,
}
impl Cursor {
    pub fn read(&mut self, path: &Utf8Path) -> Option<(Vec<Value>, bool)> {
        let size = std::fs::metadata(path).ok()?.len();
        if self.next > size {
            *self = Self::default();
        }
        if !self.initialized {
            self.recent_start = size.saturating_sub(READ_BYTES);
            self.initial_end = size;
            self.next = self.recent_start;
            self.initialized = true;
        }
        let (mut values, next) = records(path, self.next, READ_BYTES)?;
        self.next = next;
        self.recent_count = values.len();
        if self.backfill < self.recent_start {
            let (older, end) = records(
                path,
                self.backfill,
                READ_BYTES.min(self.recent_start - self.backfill),
            )?;
            self.backfill = end;
            values.splice(0..0, older);
        }
        if self.recent_start > 0 && self.backfill >= self.recent_start && !self.replayed {
            let (recent, _) = records(
                path,
                self.recent_start,
                self.initial_end - self.recent_start,
            )?;
            self.recent_count += recent.len();
            values.extend(recent);
            self.replayed = true;
        }
        Some((values, self.backfill < self.recent_start || next < size))
    }
}

pub(super) fn records(path: &Utf8Path, from: u64, limit: u64) -> Option<(Vec<Value>, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let aligned = if from == 0 {
        true
    } else {
        file.seek(std::io::SeekFrom::Start(from - 1)).ok()?;
        let mut byte = [0];
        file.read_exact(&mut byte).ok()?;
        byte[0] == b'\n'
    };
    file.seek(std::io::SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let start = if aligned {
        0
    } else {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(end, |i| i + 1)
    };
    let values = bytes[start.min(end)..end]
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    // An overlong incomplete line must not pin discovery forever.
    Some((
        values,
        from + if end == 0 && bytes.len() as u64 == limit {
            limit
        } else {
            end as u64
        },
    ))
}

pub(super) fn header(path: &Utf8Path) -> Option<Value> {
    let mut line = String::new();
    std::io::BufReader::new(std::fs::File::open(path).ok()?.take(HEADER_BYTES))
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_LIMIT
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

pub(super) fn confined(path: &Utf8Path, root: &Utf8Path) -> Option<Utf8PathBuf> {
    let real = path.real().ok()?;
    real.starts_with(root.real().ok()?).then_some(real)
}
