//! Drafts: unsaved work written to `~/.local/share/omavim/drafts/` a moment
//! after typing stops, so a crash doesn't lose it. Each Omavim writes its
//! own (named for the file and the process), removes it on a save or a clean
//! close, and at start offers back one left by an Omavim that's gone.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// The file it's a draft of (None: never saved).
    pub path: Option<PathBuf>,
    /// The Omavim that wrote it.
    pub pid: u32,
    /// When, in seconds since 1970.
    pub written: u64,
    pub final_newline: bool,
    pub text: String,
}

/// Where drafts go.
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("omavim/drafts"))
}

/// A name for a file's drafts that stays the same from one version of
/// Omavim to the next (FNV-1a of its path).
fn key(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "untitled".into();
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn file(dir: &Path, path: Option<&Path>, pid: u32) -> PathBuf {
    dir.join(format!("{}-{pid}.toml", key(path)))
}

/// Write this Omavim's draft of a file.
pub fn write(dir: &Path, draft: &Draft) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let to = file(dir, draft.path.as_deref(), draft.pid);
    let text = toml::to_string(draft).map_err(std::io::Error::other)?;
    // Whole or not at all: written aside, then moved into place.
    let tmp = to.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, to)
}

/// Remove this Omavim's draft of a file.
pub fn remove(dir: &Path, path: Option<&Path>, pid: u32) {
    let _ = std::fs::remove_file(file(dir, path, pid));
}

/// Drafts of a file left by an Omavim that's no longer running, newest
/// first, with where each is.
pub fn orphans(dir: &Path, path: Option<&Path>) -> Vec<(PathBuf, Draft)> {
    let prefix = format!("{}-", key(path));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, Draft)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "toml")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
        })
        .filter_map(|p| {
            let draft: Draft = toml::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            (draft.path.as_deref() == path && !running(draft.pid)).then_some((p, draft))
        })
        .collect();
    found.sort_by_key(|(_, d)| std::cmp::Reverse(d.written));
    found
}

/// Is that process an Omavim that's still running?
fn running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .is_ok_and(|name| name.trim_end().starts_with("omavim"))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omavim-drafts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn draft(path: Option<&str>, pid: u32, written: u64) -> Draft {
        Draft {
            path: path.map(PathBuf::from),
            pid,
            written,
            final_newline: true,
            text: format!("from {pid}"),
        }
    }

    #[test]
    fn only_drafts_of_omavims_that_are_gone_are_offered() {
        let dir = temp("orphans");
        // (Pids past the kernel's limit are never running.)
        let gone = [
            draft(Some("/a.md"), 4_000_001, 10),
            draft(Some("/a.md"), 4_000_002, 20),
        ];
        for d in &gone {
            write(&dir, d).unwrap();
        }
        write(&dir, &draft(Some("/a.md"), std::process::id(), 30)).unwrap();
        write(&dir, &draft(Some("/b.md"), 4_000_003, 40)).unwrap();
        write(&dir, &draft(None, 4_000_004, 50)).unwrap();
        let found: Vec<Draft> = orphans(&dir, Some(Path::new("/a.md")))
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        assert_eq!(
            found,
            [gone[1].clone(), gone[0].clone()],
            "newest first, not ours"
        );
        assert_eq!(orphans(&dir, None).len(), 1);
        remove(&dir, Some(Path::new("/b.md")), 4_000_003);
        assert!(orphans(&dir, Some(Path::new("/b.md"))).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_key_is_stable() {
        assert_eq!(
            key(Some(Path::new("/home/me/notes.md"))),
            "85630058fc46d7fd"
        );
    }
}
