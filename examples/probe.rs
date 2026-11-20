//! Developer probe: print what the vendor driver reads, optionally apply settings through the same
//! validated path as the app, then read again after the fans have had time to respond.
//!
//! Needs an elevated shell: `cargo run --example probe -- [--out FILE] [id=value ...] [--wait SECS]`

use std::fmt::Write as _;
use std::time::Duration;

use spanner::engine::sampler::Sampler;
use spanner::engine::settings::{self, Change};
use spanner::platform::{Native, Paths, System};

fn snapshot(s: &mut Sampler<Native>, out: &mut String) {
    let sample = s.sample();
    let r = &sample.record;
    let v = &sample.vendor;
    let _ = writeln!(
        out,
        "cpu {:?} °C · gpu {:?} °C · fans cpu {:?} gpu {:?} RPM · max cooling {:?} · status {:?}",
        r.cpu_temp_c, r.gpu_temp_c, v.cpu_fan_rpm, v.gpu_fan_rpm, v.max_cooling, v.status
    );
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let take = |args: &mut Vec<String>, flag: &str| {
        let i = args.iter().position(|a| a == flag)?;
        args.remove(i);
        (i < args.len()).then(|| args.remove(i))
    };
    let out_path = take(&mut args, "--out");
    let wait: u64 = take(&mut args, "--wait").and_then(|w| w.parse().ok()).unwrap_or(10);
    let changes: Vec<Change> = args
        .iter()
        .filter_map(|a| a.split_once('=').map(|(id, v)| Change { id: id.into(), value: v.parse().expect("integer value") }))
        .collect();

    let mut out = String::new();
    let config = System::config_file().expect("config path");
    let mut s = Sampler::<Native>::new(Default::default());
    let _ = writeln!(out, "driver: {}", s.vendor().map_or("none".into(), |v| v.describe()));
    for v in settings::views(&s.platform, s.vendor(), &config) {
        let _ = writeln!(out, "  {:<16} current {:?} default {:?} {}", v.id, v.current, v.default, v.unavailable.unwrap_or_default());
    }
    let _ = write!(out, "before: ");
    snapshot(&mut s, &mut out);

    if !changes.is_empty() {
        match settings::apply(&s.platform, s.vendor(), &config, &changes) {
            Ok(outcomes) => {
                for o in outcomes {
                    let _ = writeln!(out, "apply {} -> ok={} {}", o.id, o.ok, o.error.unwrap_or_default());
                }
            }
            Err(e) => {
                let _ = writeln!(out, "apply rejected: {e}");
            }
        }
        std::thread::sleep(Duration::from_secs(wait));
        let _ = write!(out, "after {wait}s: ");
        snapshot(&mut s, &mut out);
    }

    match out_path {
        Some(p) => std::fs::write(p, out).expect("write output"),
        None => print!("{out}"),
    }
}
