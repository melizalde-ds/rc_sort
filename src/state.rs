use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where everything lives. Tests point `home` and `skel` at temp dirs.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub skel: PathBuf,
    /// System fish `conf.d` folders; they load in name order together with the user's.
    pub fish_vendor_conf: Vec<PathBuf>,
}

impl Paths {
    pub fn rc_dir(&self) -> PathBuf {
        self.home.join(".bashrc.d")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".local/state/rc-sort")
    }
}

/// Exclusive lock shared by the service and the prompt. Released when the File drops.
pub fn lock(p: &Paths) -> Result<File> {
    let dir = p.state_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("lock");
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    f.lock().with_context(|| format!("locking {}", path.display()))?;
    Ok(f)
}

/// Writes via a temp file + rename so readers never see half a file.
/// Follows symlinks so a dotfiles-manager link stays a link; keeps the file's mode.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = real.parent().context("path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = real.file_name().context("path has no file name")?.to_string_lossy();
    let tmp = dir.join(format!("{name}.rc-sort.tmp"));
    let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(content.as_bytes())
        .and_then(|_| f.sync_all())
        .with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = fs::metadata(&real) {
        fs::set_permissions(&tmp, meta.permissions())
            .with_context(|| format!("setting mode on {}", tmp.display()))?;
    }
    fs::rename(&tmp, &real).with_context(|| format!("replacing {}", real.display()))
}

/// Saves `content` to backups/<file without leading dot>.<unix secs>[.<n>]. Never overwrites.
pub fn backup(p: &Paths, file: &str, content: &str) -> Result<()> {
    let dir = p.state_dir().join("backups");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let base = format!("{}.{secs}", file.trim_start_matches('.').replace('/', "_"));
    for n in 0.. {
        let path = if n == 0 { dir.join(&base) } else { dir.join(format!("{base}.{n}")) };
        match File::create_new(&path) {
            Ok(mut f) => {
                return f
                    .write_all(content.as_bytes())
                    .and_then(|_| f.sync_all())
                    .with_context(|| format!("writing backup {}", path.display()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating backup {}", path.display())),
        }
    }
    unreachable!()
}

/// Stable content hash (FNV-1a 64) of a block, ignoring trailing spaces and outer blank lines.
pub fn hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in normalize(text).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Lines with trailing whitespace trimmed, outer blank lines removed, joined by '\n'.
pub fn normalize(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

/// A block waiting for the user's answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    pub hash: u64,
    pub file: String,
    pub text: String,
    /// Empty, or why it's unsure (e.g. a likely duplicate).
    pub note: String,
}

#[derive(Default, Debug, PartialEq)]
pub struct State {
    pub pending: Vec<Pending>,
    pub kept: HashSet<u64>,
    pub messages: Vec<String>,
}

impl State {
    /// Missing files count as empty. Caller holds the lock.
    pub fn load(p: &Paths) -> Result<State> {
        let dir = p.state_dir();
        let pending = match read_opt(&dir.join("pending.json"))? {
            Some(s) if !s.trim().is_empty() => {
                serde_json::from_str(&s).context("parsing pending.json")?
            }
            _ => Vec::new(),
        };
        let kept = load_kept(p)?;
        let messages = read_opt(&dir.join("messages"))?
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        Ok(State { pending, kept, messages })
    }

    /// Caller holds the lock.
    pub fn save(&self, p: &Paths) -> Result<()> {
        let dir = p.state_dir();
        atomic_write(&dir.join("pending.json"), &serde_json::to_string_pretty(&self.pending)?)?;
        let mut kept: Vec<u64> = self.kept.iter().copied().collect();
        kept.sort_unstable();
        atomic_write(&dir.join("kept"), &kept.iter().map(|h| format!("{h}\n")).collect::<String>())?;
        atomic_write(&dir.join("messages"), &self.messages.iter().map(|m| format!("{m}\n")).collect::<String>())
    }
}

/// Hashes of blocks the user chose to keep in place. Read-only; no lock needed.
pub fn load_kept(p: &Paths) -> Result<HashSet<u64>> {
    read_opt(&p.state_dir().join("kept"))?
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse::<u64>().with_context(|| format!("bad hash in kept: {l}")))
        .collect()
}

/// Reads a file, or `None` if it doesn't exist.
pub fn read_opt(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn tmp_home(name: &str) -> Paths {
        let home = std::env::temp_dir().join(format!("rc-sort-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        Paths { home, skel: PathBuf::from("/nonexistent"), fish_vendor_conf: Vec::new() }
    }

    #[test]
    fn hash_ignores_trailing_whitespace_and_outer_blanks() {
        assert_eq!(hash("\nalias a=b  \n"), hash("alias a=b"));
        assert_ne!(hash("alias a=b"), hash("alias a=c"));
    }

    #[test]
    fn state_roundtrip() {
        let p = tmp_home("roundtrip");
        let mut s = State::default();
        s.pending.push(Pending { hash: 7, file: ".bashrc".into(), text: "x y".into(), note: String::new() });
        s.kept.insert(42);
        s.messages = vec!["one".into(), "two".into()];
        s.save(&p).unwrap();
        assert_eq!(State::load(&p).unwrap(), s);
    }

    #[test]
    fn load_missing_is_empty() {
        let p = tmp_home("missing");
        assert_eq!(State::load(&p).unwrap(), State::default());
    }

    #[test]
    fn atomic_write_replaces_and_keeps_mode() {
        let p = tmp_home("mode");
        let f = p.home.join("file");
        fs::write(&f, "old").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write(&f, "new").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "new");
        assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_dir(&p.home).unwrap().count(), 1, "temp file left behind");
    }

    #[test]
    fn atomic_write_through_symlink() {
        let p = tmp_home("symlink");
        let real = p.home.join("real");
        let link = p.home.join("link");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        atomic_write(&link, "new").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }

    #[test]
    fn backup_twice_same_second_keeps_both() {
        let p = tmp_home("backup");
        backup(&p, ".bashrc", "a").unwrap();
        backup(&p, ".bashrc", "b").unwrap();
        assert_eq!(fs::read_dir(p.state_dir().join("backups")).unwrap().count(), 2);
    }

    #[test]
    fn lock_creates_state_dir() {
        let p = tmp_home("lock");
        let _guard = lock(&p).unwrap();
        assert!(p.state_dir().join("lock").exists());
    }
}
