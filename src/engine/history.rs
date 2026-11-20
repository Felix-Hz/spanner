//! History queries: one pass over the stored range, reduced to a fixed number of time buckets
//! (min / max / mean each) plus a fixed-bin histogram per metric for exact-to-the-bin percentiles.
//! Cost is O(records × metrics) time and O(buckets × metrics) memory, independent of range length.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::config::{TrackedApp, app_id};
use super::store::{self, Record, flag};
use crate::error::{Error, Result};

/// NVML clock-event reasons that mean the GPU is being held back (power or heat), not just idle.
pub const GPU_LIMIT_MASK: u16 = 0x04 | 0x08 | 0x20 | 0x40 | 0x80;

pub struct Metric {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub group: &'static str,
    read: fn(&Record) -> Option<f32>,
    /// Histogram lower bound, bin width and bin count; values outside are clamped into the end bins.
    bins: (f32, f32, usize),
}

impl Metric {
    pub fn read(&self, r: &Record) -> Option<f32> {
        (self.read)(r)
    }

    fn bin(&self, v: f32) -> usize {
        let (lo, width, n) = self.bins;
        (((v - lo) / width).floor().max(0.0) as usize).min(n - 1)
    }

    fn bin_value(&self, i: usize) -> f32 {
        let (lo, width, _) = self.bins;
        lo + i as f32 * width
    }
}

pub const METRICS: &[Metric] = &[
    Metric {
        id: "cpu_temp",
        label: "CPU temperature",
        unit: "°C",
        group: "Processor",
        read: |r| r.cpu_temp_c.map(f32::from),
        bins: (0.0, 1.0, 121),
    },
    Metric {
        id: "cpu_util",
        label: "CPU load",
        unit: "%",
        group: "Processor",
        read: |r| r.cpu_util_x10.map(|v| f32::from(v) / 10.0),
        bins: (0.0, 1.0, 101),
    },
    Metric {
        id: "cpu_mhz",
        label: "CPU clock",
        unit: "MHz",
        group: "Processor",
        read: |r| r.cpu_mhz.map(f32::from),
        bins: (0.0, 25.0, 241),
    },
    Metric {
        id: "gpu_temp",
        label: "GPU temperature",
        unit: "°C",
        group: "Graphics",
        read: |r| r.gpu_temp_c.map(f32::from),
        bins: (0.0, 1.0, 121),
    },
    Metric {
        id: "gpu_util",
        label: "GPU load",
        unit: "%",
        group: "Graphics",
        read: |r| r.gpu_util_pct.map(f32::from),
        bins: (0.0, 1.0, 101),
    },
    Metric {
        id: "gpu_power",
        label: "GPU power",
        unit: "W",
        group: "Graphics",
        read: |r| r.gpu_power_dw.map(|v| f32::from(v) / 10.0),
        bins: (0.0, 0.5, 401),
    },
    Metric {
        id: "gpu_mhz",
        label: "GPU clock",
        unit: "MHz",
        group: "Graphics",
        read: |r| r.gpu_mhz.map(f32::from),
        bins: (0.0, 15.0, 201),
    },
    Metric {
        id: "gpu_vram",
        label: "GPU memory used",
        unit: "MB",
        group: "Graphics",
        read: |r| r.gpu_vram_mb.map(f32::from),
        bins: (0.0, 32.0, 257),
    },
    Metric {
        id: "gpu_limited",
        label: "GPU held back",
        unit: "% of time",
        group: "Graphics",
        read: |r| r.gpu_throttle.map(|t| if t & GPU_LIMIT_MASK != 0 { 100.0 } else { 0.0 }),
        bins: (0.0, 100.0, 2),
    },
    Metric {
        id: "cpu_fan",
        label: "CPU fan",
        unit: "RPM",
        group: "Cooling",
        read: |r| r.cpu_fan_rpm.map(f32::from),
        bins: (0.0, 50.0, 201),
    },
    Metric {
        id: "gpu_fan",
        label: "GPU fan",
        unit: "RPM",
        group: "Cooling",
        read: |r| r.gpu_fan_rpm.map(f32::from),
        bins: (0.0, 50.0, 201),
    },
    Metric {
        id: "battery",
        label: "Battery charge",
        unit: "%",
        group: "Power",
        read: |r| r.battery_pct.map(f32::from),
        bins: (0.0, 1.0, 101),
    },
    Metric {
        id: "mem_avail",
        label: "Free memory",
        unit: "MB",
        group: "Power",
        read: |r| r.mem_avail_mb.map(|v| v as f32),
        bins: (0.0, 256.0, 257),
    },
];

pub fn metric(id: &str) -> Option<&'static Metric> {
    METRICS.iter().find(|m| m.id == id)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerFilter {
    Any,
    Ac,
    Battery,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub from_ms: i64,
    pub to_ms: i64,
    pub buckets: u32,
    pub metrics: Vec<String>,
    pub power: PowerFilter,
    pub tracked_only: bool,
    /// Only samples taken while one of these executables was running (any case). Empty: no app filter.
    #[serde(default)]
    pub apps: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Stats {
    pub samples: u64,
    pub min: f32,
    pub avg: f32,
    pub p95: f32,
    pub max: f32,
}

#[derive(Debug, Serialize)]
pub struct Series {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub group: &'static str,
    /// Per bucket `[min, max, mean]`, or `null` where there is no data (drawn as a gap).
    pub buckets: Vec<Option<[f32; 3]>>,
    pub stats: Option<Stats>,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub from_ms: i64,
    pub to_ms: i64,
    pub bucket_ms: f64,
    pub samples: u64,
    pub series: Vec<Series>,
    /// Samples per tracked app in the range, ignoring the app filter, so the picker can show them.
    pub apps: Vec<AppUsage>,
}

#[derive(Debug, Serialize)]
pub struct AppUsage {
    pub exe: String,
    pub name: Option<String>,
    pub samples: u64,
}

const MAX_BUCKETS: u32 = 2_000;
const MAX_SPAN_MS: i64 = 400 * super::time::MS_PER_DAY;

#[derive(Clone, Copy)]
struct Agg {
    min: f32,
    max: f32,
    sum: f64,
    n: u32,
}

impl Agg {
    const EMPTY: Self = Self { min: f32::INFINITY, max: f32::NEG_INFINITY, sum: 0.0, n: 0 };

    fn add(&mut self, v: f32) {
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        self.sum += f64::from(v);
        self.n += 1;
    }
}

fn validate(q: &Query) -> Result<Vec<&'static Metric>> {
    if q.to_ms <= q.from_ms {
        return Err(Error::msg("The end of the range must be after its start."));
    }
    if q.to_ms - q.from_ms > MAX_SPAN_MS {
        return Err(Error::msg("Ranges are limited to 400 days."));
    }
    if q.metrics.is_empty() || q.metrics.len() > METRICS.len() {
        return Err(Error::msg("Choose at least one metric."));
    }
    q.metrics.iter().map(|id| metric(id).ok_or_else(|| Error(format!("Unknown metric '{id}'.")))).collect()
}

fn power_matches(p: PowerFilter, r: &Record) -> bool {
    match p {
        PowerFilter::Any => true,
        PowerFilter::Ac => r.has(flag::ON_AC),
        PowerFilter::Battery => !r.has(flag::ON_AC),
    }
}

fn keep(q: &Query, apps: &[u32], r: &Record) -> bool {
    (apps.is_empty() || apps.contains(&r.app)) && power_matches(q.power, r) && (!q.tracked_only || r.has(flag::TRACKED_APP))
}

pub fn run(dir: &Path, tracked: &[TrackedApp], q: &Query) -> Result<Response> {
    let metrics = validate(q)?;
    let wanted: Vec<u32> = q.apps.iter().map(|a| app_id(a)).collect();
    let tracked_ids: Vec<u32> = tracked.iter().map(|a| app_id(&a.exe)).collect();
    let mut usage = vec![0u64; tracked.len()];
    let nb = q.buckets.clamp(10, MAX_BUCKETS) as usize;
    let span = (q.to_ms - q.from_ms) as i128;
    let mut aggs = vec![Agg::EMPTY; nb * metrics.len()];
    let mut hists: Vec<Vec<u32>> = metrics.iter().map(|m| vec![0; m.bins.2]).collect();
    let mut totals = vec![Agg::EMPTY; metrics.len()];
    let mut samples = 0u64;

    store::scan(dir, q.from_ms, q.to_ms, |r| {
        // Usage respects the time range and power filter, not the app filter, so every app shows its total.
        if r.app != 0
            && power_matches(q.power, r)
            && let Some(i) = tracked_ids.iter().position(|&id| id == r.app)
        {
            usage[i] += 1;
        }
        if !keep(q, &wanted, r) {
            return;
        }
        samples += 1;
        let b = ((((r.ts_ms - q.from_ms) as i128) * nb as i128 / span) as usize).min(nb - 1);
        for (mi, m) in metrics.iter().enumerate() {
            if let Some(v) = m.read(r) {
                aggs[b * metrics.len() + mi].add(v);
                totals[mi].add(v);
                hists[mi][m.bin(v)] += 1;
            }
        }
    })?;

    let series = metrics
        .iter()
        .enumerate()
        .map(|(mi, m)| Series {
            id: m.id,
            label: m.label,
            unit: m.unit,
            group: m.group,
            buckets: (0..nb)
                .map(|b| {
                    let a = aggs[b * metrics.len() + mi];
                    (a.n > 0).then(|| [a.min, a.max, (a.sum / f64::from(a.n)) as f32])
                })
                .collect(),
            stats: stats(m, &totals[mi], &hists[mi]),
        })
        .collect();

    let apps = tracked.iter().zip(usage).map(|(a, samples)| AppUsage { exe: a.exe.clone(), name: a.name.clone(), samples }).collect();
    Ok(Response { from_ms: q.from_ms, to_ms: q.to_ms, bucket_ms: span as f64 / nb as f64, samples, series, apps })
}

fn stats(m: &Metric, total: &Agg, hist: &[u32]) -> Option<Stats> {
    if total.n == 0 {
        return None;
    }
    // Smallest bin whose cumulative count reaches 95% (nearest-rank), capped by the true maximum.
    let rank = (u64::from(total.n) * 95).div_ceil(100);
    let mut seen = 0u64;
    let p95_bin = hist
        .iter()
        .position(|&c| {
            seen += u64::from(c);
            seen >= rank
        })
        .unwrap_or(hist.len() - 1);
    Some(Stats {
        samples: u64::from(total.n),
        min: total.min,
        avg: (total.sum / f64::from(total.n)) as f32,
        p95: m.bin_value(p95_bin).clamp(total.min, total.max),
        max: total.max,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::store::Writer;
    use crate::engine::time::MS_PER_DAY;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("spanner_hist_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn query(from_ms: i64, to_ms: i64, buckets: u32, metrics: &[&str]) -> Query {
        Query {
            from_ms,
            to_ms,
            buckets,
            metrics: metrics.iter().map(|s| s.to_string()).collect(),
            power: PowerFilter::Any,
            tracked_only: false,
            apps: Vec::new(),
        }
    }

    #[test]
    fn metric_ids_are_unique() {
        for (i, a) in METRICS.iter().enumerate() {
            assert!(METRICS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
    }

    #[test]
    fn buckets_and_stats_match_a_brute_force_reference() {
        let d = dir("agg");
        let base = 20_500 * MS_PER_DAY;
        let mut w = Writer::new(&d).unwrap();
        let temps: Vec<u8> = (0..1_000).map(|i| (40 + (i * 37) % 60) as u8).collect();
        for (i, &t) in temps.iter().enumerate() {
            let flags = if i % 2 == 0 { flag::ON_AC } else { 0 };
            w.append(&Record { ts_ms: base + i as i64 * 1_000, cpu_temp_c: Some(t), flags, ..Default::default() }).unwrap();
        }
        let r = run(&d, &[], &query(base, base + 1_000_000, 10, &["cpu_temp", "gpu_temp"])).unwrap();
        assert_eq!(r.samples, 1_000);

        let cpu = &r.series[0];
        for (b, bucket) in cpu.buckets.iter().enumerate() {
            let slice = &temps[b * 100..(b + 1) * 100];
            let [min, max, mean] = bucket.unwrap();
            assert_eq!(min, f32::from(*slice.iter().min().unwrap()));
            assert_eq!(max, f32::from(*slice.iter().max().unwrap()));
            let want = slice.iter().map(|&v| f64::from(v)).sum::<f64>() / 100.0;
            assert!((f64::from(mean) - want).abs() < 1e-4);
        }
        let mut sorted = temps.clone();
        sorted.sort_unstable();
        let st = cpu.stats.as_ref().unwrap();
        assert_eq!(st.p95, f32::from(sorted[949]), "nearest-rank p95");
        assert_eq!(st.max, 99.0);

        assert!(r.series[1].stats.is_none(), "no GPU data recorded");
        assert!(r.series[1].buckets.iter().all(Option::is_none));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn power_filter_selects_matching_samples() {
        let d = dir("filter");
        let base = 20_600 * MS_PER_DAY;
        let mut w = Writer::new(&d).unwrap();
        for i in 0..100 {
            let flags = if i < 30 { flag::ON_AC } else { 0 };
            w.append(&Record { ts_ms: base + i, battery_pct: Some(50), flags, ..Default::default() }).unwrap();
        }
        let mut q = query(base, base + 100, 10, &["battery"]);
        q.power = PowerFilter::Ac;
        assert_eq!(run(&d, &[], &q).unwrap().samples, 30);
        q.power = PowerFilter::Battery;
        assert_eq!(run(&d, &[], &q).unwrap().samples, 70);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn app_filter_and_usage() {
        let d = dir("apps");
        let base = 20_700 * MS_PER_DAY;
        let zoo = TrackedApp::new("PlanetZoo.exe", "Planet Zoo");
        let sek = TrackedApp::new("sekiro.exe", "Sekiro");
        let mut w = Writer::new(&d).unwrap();
        for i in 0..90 {
            let app = match i % 3 {
                0 => app_id("PlanetZoo.exe"),
                1 => app_id("SEKIRO.exe"),
                _ => 0,
            };
            let flags = if app != 0 { flag::TRACKED_APP } else { 0 };
            w.append(&Record { ts_ms: base + i, cpu_temp_c: Some(50), flags, app, ..Default::default() }).unwrap();
        }
        let mut q = query(base, base + 90, 10, &["cpu_temp"]);
        q.apps = vec!["planetzoo.exe".into()];
        let r = run(&d, &[zoo, sek], &q).unwrap();
        assert_eq!(r.samples, 30);
        assert_eq!(r.apps.iter().map(|a| a.samples).collect::<Vec<_>>(), [30, 30], "usage ignores the app filter");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn rejects_bad_queries() {
        let d = dir("bad");
        assert!(run(&d, &[], &query(10, 10, 100, &["cpu_temp"])).is_err());
        assert!(run(&d, &[], &query(0, 10, 100, &["nope"])).is_err());
        assert!(run(&d, &[], &query(0, 10, 100, &[])).is_err());
        assert!(run(&d, &[], &query(0, MAX_SPAN_MS + 1, 100, &["cpu_temp"])).is_err());
    }
}
