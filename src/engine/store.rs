//! Append-only history on disk: one file per UTC day, each a header followed by fixed-size records in
//! timestamp order. Fixed records allow a binary search to the start of a range; the rest of the query is
//! one sequential read, so memory stays constant however long the range is.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::time::{civil_from_days, day_of, days_from_civil};
use crate::error::{Error, Result};

const MAGIC: [u8; 8] = *b"SPANNER1";
const VERSION: u16 = 1;
const HEADER_LEN: u64 = 16;
pub const RECORD_LEN: usize = 40;

/// Bit flags stored with every sample.
pub mod flag {
    pub const ON_AC: u8 = 1 << 0;
    pub const TRACKED_APP: u8 = 1 << 1;
    pub const MAX_COOLING: u8 = 1 << 2;
}

/// One sample. `None` means the source was unavailable; it is stored as the type's maximum value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub ts_ms: i64,
    pub cpu_temp_c: Option<u8>,
    pub gpu_temp_c: Option<u8>,
    pub gpu_util_pct: Option<u8>,
    pub battery_pct: Option<u8>,
    pub flags: u8,
    /// Tenths of a percent, so the chart keeps sub-percent detail at low load.
    pub cpu_util_x10: Option<u16>,
    pub cpu_mhz: Option<u16>,
    /// Tenths of a watt.
    pub gpu_power_dw: Option<u16>,
    pub gpu_mhz: Option<u16>,
    pub gpu_vram_mb: Option<u16>,
    /// NVML clock-event reason bits (all defined bits fit in 16).
    pub gpu_throttle: Option<u16>,
    pub cpu_fan_rpm: Option<u16>,
    pub gpu_fan_rpm: Option<u16>,
    pub mem_avail_mb: Option<u32>,
    /// `config::app_id` of the tracked app running at the time; 0 if none.
    pub app: u32,
}

impl Record {
    pub fn has(&self, f: u8) -> bool {
        self.flags & f != 0
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        fn u8o(v: Option<u8>) -> u8 {
            v.map_or(u8::MAX, |v| v.min(u8::MAX - 1))
        }
        fn u16o(v: Option<u16>) -> [u8; 2] {
            v.map_or(u16::MAX, |v| v.min(u16::MAX - 1)).to_le_bytes()
        }
        let mut b = [0u8; RECORD_LEN];
        b[0..8].copy_from_slice(&self.ts_ms.to_le_bytes());
        b[8] = u8o(self.cpu_temp_c);
        b[9] = u8o(self.gpu_temp_c);
        b[10] = u8o(self.gpu_util_pct);
        b[11] = u8o(self.battery_pct);
        b[12] = self.flags;
        // b[13..16] reserved
        b[16..18].copy_from_slice(&u16o(self.cpu_util_x10));
        b[18..20].copy_from_slice(&u16o(self.cpu_mhz));
        b[20..22].copy_from_slice(&u16o(self.gpu_power_dw));
        b[22..24].copy_from_slice(&u16o(self.gpu_mhz));
        b[24..26].copy_from_slice(&u16o(self.gpu_vram_mb));
        b[26..28].copy_from_slice(&u16o(self.gpu_throttle));
        b[28..30].copy_from_slice(&u16o(self.cpu_fan_rpm));
        b[30..32].copy_from_slice(&u16o(self.gpu_fan_rpm));
        let mem = self.mem_avail_mb.map_or(u32::MAX, |v| v.min(u32::MAX - 1));
        b[32..36].copy_from_slice(&mem.to_le_bytes());
        b[36..40].copy_from_slice(&self.app.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; RECORD_LEN]) -> Self {
        fn u8o(v: u8) -> Option<u8> {
            (v != u8::MAX).then_some(v)
        }
        fn u16o(b: &[u8]) -> Option<u16> {
            let v = u16::from_le_bytes([b[0], b[1]]);
            (v != u16::MAX).then_some(v)
        }
        let mem = u32::from_le_bytes([b[32], b[33], b[34], b[35]]);
        Self {
            ts_ms: i64::from_le_bytes(b[0..8].try_into().unwrap()),
            cpu_temp_c: u8o(b[8]),
            gpu_temp_c: u8o(b[9]),
            gpu_util_pct: u8o(b[10]),
            battery_pct: u8o(b[11]),
            flags: b[12],
            cpu_util_x10: u16o(&b[16..18]),
            cpu_mhz: u16o(&b[18..20]),
            gpu_power_dw: u16o(&b[20..22]),
            gpu_mhz: u16o(&b[22..24]),
            gpu_vram_mb: u16o(&b[24..26]),
            gpu_throttle: u16o(&b[26..28]),
            cpu_fan_rpm: u16o(&b[28..30]),
            gpu_fan_rpm: u16o(&b[30..32]),
            mem_avail_mb: (mem != u32::MAX).then_some(mem),
            app: u32::from_le_bytes([b[36], b[37], b[38], b[39]]),
        }
    }
}

fn header() -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    h[0..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_le_bytes());
    h[10..12].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    h
}

fn check_header(file: &mut File, path: &Path) -> Result<()> {
    let mut h = [0u8; HEADER_LEN as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut h)?;
    if h != header() {
        return Err(Error(format!("{} is not a v{VERSION} history file", path.display())));
    }
    Ok(())
}

pub fn segment_path(dir: &Path, day: i64) -> PathBuf {
    let (y, m, d) = civil_from_days(day);
    dir.join(format!("{y:04}-{m:02}-{d:02}.bin"))
}

fn parse_segment_day(name: &str) -> Option<i64> {
    let stem = name.strip_suffix(".bin")?;
    let mut it = stem.splitn(3, '-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    (stem.len() == 10).then(|| days_from_civil(y, m, d))
}

fn record_count(len: u64) -> u64 {
    len.saturating_sub(HEADER_LEN) / RECORD_LEN as u64
}

fn read_record_at(file: &mut File, index: u64) -> Result<Record> {
    let mut b = [0u8; RECORD_LEN];
    file.seek(SeekFrom::Start(HEADER_LEN + index * RECORD_LEN as u64))?;
    file.read_exact(&mut b)?;
    Ok(Record::decode(&b))
}

/// Appends samples, rolling to a new file at each UTC midnight.
pub struct Writer {
    dir: PathBuf,
    open: Option<(i64, File)>,
    last_ts: i64,
}

impl Writer {
    pub fn new(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self { dir: dir.to_path_buf(), open: None, last_ts: i64::MIN })
    }

    /// Samples not newer than the last one are dropped, keeping every file sorted for the binary search.
    /// After a backwards clock correction this pauses recording until the clock catches up.
    pub fn append(&mut self, r: &Record) -> Result<bool> {
        let day = day_of(r.ts_ms);
        if self.open.as_ref().is_none_or(|(d, _)| *d != day) {
            let (file, last) = self.open_segment(day)?;
            self.last_ts = self.last_ts.max(last);
            self.open = Some((day, file));
        }
        if r.ts_ms <= self.last_ts {
            return Ok(false);
        }
        let (_, file) = self.open.as_mut().expect("segment opened above");
        file.write_all(&r.encode())?;
        self.last_ts = r.ts_ms;
        Ok(true)
    }

    fn open_segment(&self, day: i64) -> Result<(File, i64)> {
        let path = segment_path(&self.dir, day);
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            file.write_all(&header())?;
            return Ok((file, i64::MIN));
        }
        check_header(&mut file, &path)?;
        // A crash mid-write can leave a partial record; drop it so every later record stays aligned.
        let n = record_count(len);
        let aligned = HEADER_LEN + n * RECORD_LEN as u64;
        if aligned != len {
            file.set_len(aligned)?;
        }
        let last = if n > 0 { read_record_at(&mut file, n - 1)?.ts_ms } else { i64::MIN };
        file.seek(SeekFrom::End(0))?;
        Ok((file, last))
    }
}

/// Calls `visit` for every record with `from_ms <= ts <= to_ms`, oldest first.
pub fn scan(dir: &Path, from_ms: i64, to_ms: i64, mut visit: impl FnMut(&Record)) -> Result<()> {
    for day in day_of(from_ms)..=day_of(to_ms) {
        let path = segment_path(dir, day);
        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if check_header(&mut file, &path).is_err() {
            continue;
        }
        let n = record_count(file.metadata()?.len());
        let start = lower_bound(&mut file, n, from_ms)?;
        file.seek(SeekFrom::Start(HEADER_LEN + start * RECORD_LEN as u64))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut b = [0u8; RECORD_LEN];
        for _ in start..n {
            reader.read_exact(&mut b)?;
            let r = Record::decode(&b);
            if r.ts_ms > to_ms {
                return Ok(());
            }
            visit(&r);
        }
    }
    Ok(())
}

/// Index of the first record with `ts >= target` (or `n`), in O(log n) seeks.
fn lower_bound(file: &mut File, n: u64, target: i64) -> Result<u64> {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if read_record_at(file, mid)?.ts_ms < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// Most recent record across the newest segments, if any.
pub fn latest(dir: &Path, today: i64) -> Result<Option<Record>> {
    for day in [today, today - 1] {
        let path = segment_path(dir, day);
        let Ok(mut file) = File::open(&path) else { continue };
        if check_header(&mut file, &path).is_err() {
            continue;
        }
        let n = record_count(file.metadata()?.len());
        if n > 0 {
            return read_record_at(&mut file, n - 1).map(Some);
        }
    }
    Ok(None)
}

/// Deletes segments older than `keep_days` whole days before `today`. Returns how many were removed.
pub fn prune(dir: &Path, today: i64, keep_days: u32) -> Result<usize> {
    let oldest_kept = today - i64::from(keep_days) + 1;
    let mut removed = 0;
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(day) = name.to_str().and_then(parse_segment_day)
            && day < oldest_kept
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::time::MS_PER_DAY;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spanner_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn rec(ts_ms: i64) -> Record {
        Record { ts_ms, cpu_temp_c: Some((ts_ms % 100) as u8), cpu_mhz: Some(2100), ..Default::default() }
    }

    #[test]
    fn encode_decode_round_trip_keeps_none() {
        let r = Record {
            ts_ms: 1_700_000_000_123,
            cpu_temp_c: Some(95),
            gpu_temp_c: None,
            gpu_power_dw: Some(732),
            mem_avail_mb: Some(49_106),
            gpu_throttle: Some(0x24),
            cpu_fan_rpm: Some(5_783),
            app: 0xDEAD_BEEF,
            flags: flag::ON_AC | flag::TRACKED_APP,
            ..Default::default()
        };
        assert_eq!(Record::decode(&r.encode()), r);
    }

    #[test]
    fn values_never_collide_with_the_none_sentinel() {
        let r = Record { cpu_temp_c: Some(u8::MAX), cpu_mhz: Some(u16::MAX), ..Default::default() };
        let d = Record::decode(&r.encode());
        assert_eq!(d.cpu_temp_c, Some(u8::MAX - 1));
        assert_eq!(d.cpu_mhz, Some(u16::MAX - 1));
    }

    #[test]
    fn scan_returns_exact_range_across_days() {
        let dir = tmp("scan");
        let mut w = Writer::new(&dir).unwrap();
        let base = 20_000 * MS_PER_DAY;
        let step = 5_000;
        let all: Vec<i64> = (0..40_000).map(|i| base + i * step).collect(); // spans ~2.3 days
        for &t in &all {
            assert!(w.append(&rec(t)).unwrap());
        }
        let (from, to) = (base + MS_PER_DAY - 12_345, base + 2 * MS_PER_DAY + 7);
        let mut got = Vec::new();
        scan(&dir, from, to, |r| got.push(r.ts_ms)).unwrap();
        let want: Vec<i64> = all.iter().copied().filter(|t| (from..=to).contains(t)).collect();
        assert_eq!(got, want);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writer_drops_out_of_order_samples_and_resumes_after_restart() {
        let dir = tmp("order");
        let base = 20_000 * MS_PER_DAY;
        {
            let mut w = Writer::new(&dir).unwrap();
            assert!(w.append(&rec(base + 10)).unwrap());
            assert!(!w.append(&rec(base + 10)).unwrap());
            assert!(!w.append(&rec(base + 5)).unwrap());
        }
        let mut w = Writer::new(&dir).unwrap();
        assert!(!w.append(&rec(base + 9)).unwrap(), "last timestamp is recovered from disk");
        assert!(w.append(&rec(base + 11)).unwrap());
        assert_eq!(latest(&dir, day_of(base)).unwrap().unwrap().ts_ms, base + 11);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn torn_tail_is_truncated_on_reopen() {
        let dir = tmp("torn");
        let base = 20_000 * MS_PER_DAY;
        {
            let mut w = Writer::new(&dir).unwrap();
            w.append(&rec(base + 1)).unwrap();
        }
        let path = segment_path(&dir, day_of(base));
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&[1, 2, 3]).unwrap();
        let mut w = Writer::new(&dir).unwrap();
        w.append(&rec(base + 2)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), HEADER_LEN + 2 * RECORD_LEN as u64);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_keeps_the_last_n_days() {
        let dir = tmp("prune");
        fs::create_dir_all(&dir).unwrap();
        for day in 100..110 {
            fs::write(segment_path(&dir, day), header()).unwrap();
        }
        fs::write(dir.join("notes.txt"), b"kept").unwrap();
        assert_eq!(prune(&dir, 109, 3).unwrap(), 7);
        let left: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 4);
        fs::remove_dir_all(dir).unwrap();
    }
}
