//! Where disk space goes: one walk of the system drive, totals per category, and the largest apps,
//! games and folders.
//!
//! The walk uses a shared queue of folders listed by a few threads (no recursion, so deep trees can't
//! overflow). It never follows links or junctions, so nothing is counted twice and loops are impossible,
//! and it works out each directory's category and "thing" once, from its path, rather than per file.
//! Memory is bounded by the number of things (apps, games, user folders), not by the number of files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};

pub const CATEGORIES: [&str; 6] = ["Games", "Apps", "Windows and system", "Your files", "App data", "Other"];
const GAMES: usize = 0;
const APPS: usize = 1;
const SYSTEM: usize = 2;
const YOURS: usize = 3;
const APP_DATA: usize = 4;
const OTHER: usize = 5;
const LARGEST: usize = 5;

/// Folders under a game library whose children are individual games.
const GAME_LIBRARIES: [&str; 5] = ["epic games", "xboxgames", "gog games", "riot games", "ea games"];
const USER_FOLDERS: [&str; 7] = ["desktop", "documents", "downloads", "music", "pictures", "videos", "onedrive"];
const SYSTEM_FILES: [&str; 3] = ["pagefile.sys", "hiberfil.sys", "swapfile.sys"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub name: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub name: String,
    pub path: String,
    pub category: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Scan {
    pub root: String,
    pub scanned_at_ms: i64,
    pub duration_ms: u64,
    pub files: u64,
    pub bytes: u64,
    /// Folders that couldn't be read; their contents are missing from the totals.
    pub unreadable: u64,
    /// Largest first.
    pub categories: Vec<Category>,
    pub largest: Vec<Item>,
}

/// Live counters while a scan runs.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
}

/// What a directory is, worked out once from its path below the drive root.
#[derive(Clone)]
struct Class {
    category: usize,
    /// The app, game or folder this directory belongs to: (display name, full path).
    thing: Option<Arc<(String, String)>>,
}

/// Classifies `rel` (path components below the root, lower-cased) given the parent's class. Children
/// inherit a thing once one is set, so only the first few levels are ever examined.
fn classify(parent: &Class, rel: &[String], original: &str, full: &Path) -> Class {
    if parent.thing.is_some() {
        return parent.clone();
    }
    let depth = rel.len();
    let name = rel[depth - 1].as_str();
    let thing = || Some(Arc::new((original.to_string(), full.display().to_string())));
    let parent_name = if depth >= 2 { rel[depth - 2].as_str() } else { "" };
    // Launchers and libraries are containers, not things: their games are counted one by one below.
    if name == "steam" || GAME_LIBRARIES.contains(&name) {
        return Class { category: if name == "steam" { APPS } else { GAMES }, thing: None };
    }
    // Games first: they often sit inside Program Files.
    if (depth >= 2 && parent_name == "common" && depth >= 3 && rel[depth - 3] == "steamapps") || GAME_LIBRARIES.contains(&parent_name) {
        return Class { category: GAMES, thing: thing() };
    }
    if rel.iter().any(|c| c == "steamapps" || GAME_LIBRARIES.contains(&c.as_str())) {
        return Class { category: GAMES, thing: None };
    }
    match (rel[0].as_str(), depth) {
        ("windows", 1) => Class { category: SYSTEM, thing: thing() },
        ("program files" | "program files (x86)" | "programdata", 1) => Class { category: APPS, thing: None },
        ("program files" | "program files (x86)" | "programdata", 2) => Class { category: APPS, thing: thing() },
        ("users", 1 | 2) => Class { category: YOURS, thing: None },
        ("users", 3) if name == "appdata" => Class { category: APP_DATA, thing: None },
        ("users", 3) if USER_FOLDERS.contains(&name) => Class { category: YOURS, thing: thing() },
        ("users", 3) => Class { category: YOURS, thing: thing() },
        ("users", 4) if rel[2] == "appdata" => Class { category: APP_DATA, thing: None },
        ("users", 5) if rel[2] == "appdata" => Class { category: APP_DATA, thing: thing() },
        _ => Class { category: parent.category, thing: None },
    }
}

#[cfg(windows)]
fn is_link(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: symbolic links, junctions, mount points, cloud placeholders.
    m.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

/// One worker's running totals; merged once at the end, so the hot loop never shares a lock.
#[derive(Default)]
struct Tally {
    categories: [u64; CATEGORIES.len()],
    things: HashMap<Arc<(String, String)>, (usize, u64)>,
    files: u64,
    bytes: u64,
    unreadable: u64,
}

type Job = (PathBuf, Vec<String>, Class);

/// Folders waiting to be listed, shared by the workers. `active` counts workers holding a folder, so an
/// empty queue only means "done" when nobody can add to it any more.
struct Queue {
    jobs: Mutex<(Vec<Job>, usize)>,
    ready: Condvar,
}

impl Queue {
    fn next(&self) -> Option<Job> {
        let mut g = self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(job) = g.0.pop() {
                g.1 += 1;
                return Some(job);
            }
            if g.1 == 0 {
                self.ready.notify_all();
                return None;
            }
            g = self.ready.wait(g).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn finish(&self, found: Vec<Job>) {
        let mut g = self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.1 -= 1;
        let woke = !found.is_empty() || g.1 == 0;
        g.0.extend(found);
        drop(g);
        if woke {
            self.ready.notify_all();
        }
    }
}

fn list(dir: &Path, rel: &[String], class: &Class, t: &mut Tally, progress: &Progress) -> Vec<Job> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        t.unreadable += 1;
        return found;
    };
    for entry in entries.flatten() {
        // On Windows the directory listing already carries size and attributes: no extra call.
        let Ok(meta) = entry.metadata() else { continue };
        if is_link(&meta) {
            continue;
        }
        let original = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            let mut child_rel = rel.to_vec();
            child_rel.push(original.to_lowercase());
            let path = entry.path();
            let child = classify(class, &child_rel, &original, &path);
            found.push((path, child_rel, child));
            continue;
        }
        let size = meta.len();
        let (category, thing) = if rel.is_empty() && SYSTEM_FILES.contains(&original.to_lowercase().as_str()) {
            (SYSTEM, Some(Arc::new((original.clone(), entry.path().display().to_string()))))
        } else {
            (class.category, class.thing.clone())
        };
        t.categories[category] += size;
        if let Some(th) = thing {
            t.things.entry(th).or_insert((category, 0)).1 += size;
        }
        t.files += 1;
        t.bytes += size;
        progress.files.fetch_add(1, Ordering::Relaxed);
        progress.bytes.fetch_add(size, Ordering::Relaxed);
    }
    found
}

/// Walks `root` and returns where its space goes. Folders are listed by several threads (an NVMe drive
/// answers many requests at once); `progress` is updated as files are counted.
pub fn scan(root: &Path, now_ms: i64, progress: &Progress) -> Scan {
    let started = std::time::Instant::now();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let queue = Queue { jobs: Mutex::new((vec![(root.to_path_buf(), Vec::new(), Class { category: OTHER, thing: None })], 0)), ready: Condvar::new() };
    let tallies: Vec<Tally> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                sc.spawn(|| {
                    let mut t = Tally::default();
                    while let Some((dir, rel, class)) = queue.next() {
                        let found = list(&dir, &rel, &class, &mut t, progress);
                        queue.finish(found);
                    }
                    t
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut total = Tally::default();
    for t in tallies {
        for (i, b) in t.categories.iter().enumerate() {
            total.categories[i] += b;
        }
        for (k, (c, b)) in t.things {
            total.things.entry(k).or_insert((c, 0)).1 += b;
        }
        total.files += t.files;
        total.bytes += t.bytes;
        total.unreadable += t.unreadable;
    }

    let mut cats: Vec<Category> = CATEGORIES
        .iter()
        .zip(total.categories)
        .filter(|(_, b)| *b > 0)
        .map(|(n, b)| Category { name: (*n).to_string(), bytes: b })
        .collect();
    cats.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    let mut largest: Vec<Item> = total
        .things
        .into_iter()
        .map(|(t, (c, b))| Item { name: t.0.clone(), path: t.1.clone(), category: CATEGORIES[c].to_string(), bytes: b })
        .collect();
    // Partial sort is enough: only the top few are kept.
    if largest.len() > LARGEST {
        largest.select_nth_unstable_by(LARGEST - 1, |a, b| b.bytes.cmp(&a.bytes));
        largest.truncate(LARGEST);
    }
    largest.sort_by_key(|i| std::cmp::Reverse(i.bytes));

    Scan {
        root: root.display().to_string(),
        scanned_at_ms: now_ms,
        duration_ms: started.elapsed().as_millis() as u64,
        files: total.files,
        bytes: total.bytes,
        unreadable: total.unreadable,
        categories: cats,
        largest,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn totals_categories_and_largest_things() {
        let root = std::env::temp_dir().join(format!("spanner-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(&root.join(r"Program Files (x86)\Steam\steamapps\common\Planet Zoo\data.pak"), 9000);
        write(&root.join(r"Program Files (x86)\Steam\steamapps\common\Sekiro\data.pak"), 5000);
        write(&root.join(r"Program Files\Docker\bin\docker.exe"), 4000);
        write(&root.join(r"Windows\System32\big.dll"), 3000);
        write(&root.join(r"Users\me\Downloads\film.mp4"), 2500);
        write(&root.join(r"Users\me\AppData\Local\Spotify\cache.bin"), 2000);
        write(&root.join(r"Users\me\AppData\Local\Temp\x.tmp"), 100);
        write(&root.join(r"stray\file.bin"), 700);
        write(&root.join("pagefile.sys"), 1500);

        let p = Progress::default();
        let s = scan(&root, 1, &p);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(s.files, 9);
        assert_eq!(s.bytes, 27_800);
        let cat = |n: &str| s.categories.iter().find(|c| c.name == n).map_or(0, |c| c.bytes);
        assert_eq!(cat("Games"), 14_000);
        assert_eq!(cat("Apps"), 4000);
        assert_eq!(cat("Windows and system"), 4500, "Windows folder plus the page file");
        assert_eq!(cat("Your files"), 2500);
        assert_eq!(cat("App data"), 2100);
        assert_eq!(cat("Other"), 700);
        assert_eq!(s.categories[0].name, "Games", "largest first");
        let names: Vec<&str> = s.largest.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Planet Zoo", "Sekiro", "Docker", "Windows", "Downloads"]);
        assert_eq!(p.files.load(Ordering::Relaxed), 9);
    }
}
