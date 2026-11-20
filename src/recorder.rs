//! `spanner --record`: background recorder, started at logon.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use crate::engine::config::RecorderConfig;
use crate::engine::fan_policy::FullSpeedPolicy;
use crate::engine::sampler::Sampler;
use crate::engine::store::{self, Writer};
use crate::engine::time::{day_of, now_ms};
use crate::error::{Error, Result};
use crate::hw::Access;
use crate::platform::{Integration, Native, Paths, Platform, System};

pub fn run() -> Result<()> {
    let Some(_lock) = System::single_instance("spanner_recorder") else { return Ok(()) };
    System::lower_priority();

    let dir = System::history_dir()?;
    let cfg_path = System::config_file()?;
    let mut cfg = RecorderConfig::load(&cfg_path);
    let mut cfg_mtime = mtime(&cfg_path);
    let mut writer = Writer::new(&dir)?;
    let mut sampler = Sampler::<Native>::new(cfg.tracked_set());
    let mut pruned_day = i64::MIN;
    let mut policy = FullSpeedPolicy::default();

    // Fixed-origin schedule: sampling time never accumulates as drift.
    let mut origin = Instant::now();
    let mut tick: u32 = 0;
    loop {
        if mtime(&cfg_path) != cfg_mtime {
            cfg_mtime = mtime(&cfg_path);
            let next = RecorderConfig::load(&cfg_path);
            if next.interval_secs != cfg.interval_secs {
                (origin, tick) = (Instant::now(), 0);
            }
            sampler.tracked = next.tracked_set();
            cfg = next;
        }
        let today = day_of(now_ms());
        if today != pruned_day {
            store::prune(&dir, today, cfg.retention_days)?;
            pruned_day = today;
        }

        let sample = sampler.sample();
        writer.append(&sample.record).map_err(|e| Error(format!("writing history: {e}")))?;

        if cfg.auto_full_speed
            && let (Some(vendor), Some(on_now)) = (sampler.vendor(), sample.vendor.max_cooling)
        {
            let want = policy.decide(sample.record.cpu_temp_c, sample.record.gpu_temp_c, on_now, Instant::now());
            if want != on_now {
                // A failed switch is retried on the next sample; it never stops recording.
                let _ = vendor.set_max_cooling(Access { ec: sampler.platform.ec() }, want);
            }
        }

        let interval = Duration::from_secs(u64::from(cfg.interval_secs));
        tick += 1;
        let now = Instant::now();
        if origin + interval * tick <= now {
            // Behind (sleep, suspend): skip missed ticks rather than sample in a burst.
            tick = ((now - origin).as_secs() / interval.as_secs()) as u32 + 1;
        }
        if System::wait_for_stop(origin + interval * tick - now) {
            // Don't leave the fans pinned at full speed with nothing left to turn them off.
            if cfg.auto_full_speed
                && sample.vendor.max_cooling == Some(true)
                && let Some(vendor) = sampler.vendor()
            {
                let _ = vendor.set_max_cooling(Access { ec: sampler.platform.ec() }, false);
            }
            return Ok(());
        }
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}
