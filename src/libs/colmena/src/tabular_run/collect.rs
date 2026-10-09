//! Reading back what the model's code wrote to `/out`, on the trusted side.
//!
//! Everything in the output volume was written by untrusted code, which also
//! chose every name, and may have left links, pipes, devices, hard links,
//! sparse files and more entries than anyone expects. The reader follows the six
//! requirements of the run-mounts review note:
//!
//! 1. the caller reads BEFORE the volume is unmounted, once the child has been
//!    killed (see `local`);
//! 2. the walk starts from ONE directory descriptor and every entry is opened
//!    with `openat(O_NOFOLLOW | O_NONBLOCK | O_NOCTTY | O_CLOEXEC)`, never by
//!    path, so a link is not followed and a pipe does not block;
//! 3. every opened entry is `fstat`ed and only a regular file with one link is
//!    kept: links, pipes, sockets, devices, directories and hard links are not;
//! 4. the number of entries, the number of files, the name length and the
//!    LOGICAL size are capped (a sparse file's allocated size says nothing about
//!    what a read returns, so only `st_size` is used);
//! 5. names are raw bytes, checked against a fixed charset before they are
//!    used or shown, and a name that fails is never echoed;
//! 6. the output budget is reserved before the volume exists (`stage_call`).
//!
//! Nothing is executed and nothing is read here: the result holds open,
//! checked descriptors, which the caller streams within the call's lifetime.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Most files kept from one call (the design's cap).
pub const OUT_MAX_FILES: usize = 8;
/// Largest file kept, by logical size. An estimate, with the total, until the
/// instance is measured (spike item 5); the volume itself is `OUT_MIB` MiB.
pub const OUT_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Most bytes kept from one call.
pub const OUT_TOTAL_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// Most directory entries looked at; a volume with more keeps nothing.
pub const OUT_MAX_ENTRIES: usize = 64;
/// Longest file name kept, in bytes.
pub const OUT_NAME_MAX: usize = 64;

/// What one collection may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectLimits {
    pub max_files: usize,
    pub file_bytes: u64,
    pub total_bytes: u64,
    pub max_entries: usize,
}

impl Default for CollectLimits {
    fn default() -> Self {
        Self {
            max_files: OUT_MAX_FILES,
            file_bytes: OUT_FILE_MAX_BYTES,
            total_bytes: OUT_TOTAL_MAX_BYTES,
            max_entries: OUT_MAX_ENTRIES,
        }
    }
}

/// The two formats an output may have, told from the name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutFormat {
    Csv,
    Parquet,
}

impl OutFormat {
    fn of(name: &str) -> Option<Self> {
        match name.rsplit_once('.')?.1 {
            "csv" => Some(Self::Csv),
            "parquet" => Some(Self::Parquet),
            _ => None,
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Csv => "text/csv",
            Self::Parquet => "application/vnd.apache.parquet",
        }
    }
}

/// Why an entry was not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The name is not `[A-Za-z0-9._-]{1,64}` starting with a letter or digit
    /// and ending in `.csv` or `.parquet`.
    BadName,
    /// A link, pipe, socket, device or directory.
    NotARegularFile,
    /// More than one name for the file.
    HardLinked,
    TooLarge,
    /// Past the file count.
    OverFileCount,
    /// Past the total size.
    OverTotal,
    Unreadable,
}

/// An entry that was not kept. `name` is set only when the name passed the
/// charset check, so it is safe to show; a name that failed is never kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub name: Option<String>,
    pub reason: RejectReason,
}

/// A checked output: a name that passed, a regular file with one link, and its
/// logical size. Holds the open descriptor, so the file is the one that was
/// checked whatever the directory turns into afterwards.
#[derive(Debug)]
pub struct OutFile {
    pub name: String,
    pub format: OutFormat,
    /// Logical size, from `fstat` of the descriptor.
    pub size: u64,
    file: File,
}

impl OutFile {
    /// The descriptor, to read at most `size` bytes from.
    pub fn into_file(self) -> File {
        self.file
    }
}

/// What a collection found.
#[derive(Debug, Default)]
pub struct Collected {
    pub files: Vec<OutFile>,
    pub rejected: Vec<Rejection>,
    /// The volume holds more entries than `max_entries`: nothing was kept.
    pub too_many_entries: bool,
}

/// A name the model may choose: ASCII letters, digits, `.`, `_`, `-`, 1 to
/// [`OUT_NAME_MAX`] bytes, not starting with `.` or `-`, ending in a known
/// extension. Raw bytes in, so a name that is not UTF-8 is simply refused.
fn checked_name(raw: &[u8]) -> Option<(String, OutFormat)> {
    if raw.is_empty() || raw.len() > OUT_NAME_MAX {
        return None;
    }
    if !raw
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    if matches!(raw[0], b'.' | b'-') {
        return None;
    }
    let name = String::from_utf8(raw.to_vec()).ok()?;
    let format = OutFormat::of(&name)?;
    Some((name, format))
}

/// The names in a directory, as raw bytes, from one descriptor (never by path).
/// Stops one past `limit`, so the caller can tell "too many" from "exactly".
fn list(dir: &File, limit: usize) -> io::Result<Vec<Vec<u8>>> {
    // `fdopendir` takes the descriptor it is given, so it gets a copy.
    let copy = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let e = io::Error::last_os_error();
        drop(unsafe { OwnedFd::from_raw_fd(copy) });
        return Err(e);
    }
    let mut names = vec![];
    loop {
        // SAFETY: `stream` is a live directory stream; `readdir` returns null at
        // the end, or on error (which this walk treats as the end).
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(name.to_vec());
        if names.len() > limit {
            break;
        }
    }
    unsafe { libc::closedir(stream) };
    names.sort();
    Ok(names)
}

/// Opens one entry of `dir` without following a link or blocking on a pipe.
fn open_entry(dir: &File, name: &[u8]) -> io::Result<File> {
    let cname = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let flags =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: a relative open under a descriptor, with a NUL-terminated name.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), cname.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened and nothing else owns it.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Walks `out_dir` once and keeps what passes. `out_dir` is the call's own
/// output directory, a path the trusted side built; it is opened without
/// following a link, and nothing below it is ever reached by path.
pub fn collect_out(out_dir: &Path, limits: CollectLimits) -> io::Result<Collected> {
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(out_dir)?;
    let names = list(&dir, limits.max_entries)?;
    let mut done = Collected::default();
    if names.len() > limits.max_entries {
        done.too_many_entries = true;
        return Ok(done);
    }
    let mut total = 0u64;
    for raw in names {
        let Some((name, format)) = checked_name(&raw) else {
            done.rejected.push(Rejection {
                name: None,
                reason: RejectReason::BadName,
            });
            continue;
        };
        let reject = |done: &mut Collected, reason| {
            done.rejected.push(Rejection {
                name: Some(name.clone()),
                reason,
            });
        };
        let file = match open_entry(&dir, &raw) {
            Ok(f) => f,
            // A link (ELOOP), a socket or a device node (ENXIO, EOPNOTSUPP...).
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ELOOP | libc::ENXIO | libc::EOPNOTSUPP)
                ) =>
            {
                reject(&mut done, RejectReason::NotARegularFile);
                continue;
            }
            Err(_) => {
                reject(&mut done, RejectReason::Unreadable);
                continue;
            }
        };
        let Ok(meta) = file.metadata() else {
            reject(&mut done, RejectReason::Unreadable);
            continue;
        };
        if !meta.file_type().is_file() {
            reject(&mut done, RejectReason::NotARegularFile);
        } else if meta.nlink() != 1 {
            reject(&mut done, RejectReason::HardLinked);
        } else if meta.size() > limits.file_bytes {
            reject(&mut done, RejectReason::TooLarge);
        } else if done.files.len() >= limits.max_files {
            reject(&mut done, RejectReason::OverFileCount);
        } else if total.saturating_add(meta.size()) > limits.total_bytes {
            reject(&mut done, RejectReason::OverTotal);
        } else {
            total += meta.size();
            done.files.push(OutFile {
                name,
                format,
                size: meta.size(),
                file,
            });
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;

    fn put(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    fn names(c: &Collected) -> Vec<&str> {
        c.files.iter().map(|f| f.name.as_str()).collect()
    }

    fn reasons(c: &Collected) -> Vec<RejectReason> {
        c.rejected.iter().map(|r| r.reason).collect()
    }

    #[test]
    fn regular_files_with_good_names_are_kept_sorted_with_their_formats_and_sizes() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "b.parquet", b"1234");
        put(d.path(), "a.csv", b"x,y\n");
        put(d.path(), "empty.csv", b"");
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert_eq!(names(&c), ["a.csv", "b.parquet", "empty.csv"]);
        assert_eq!(c.files[0].format, OutFormat::Csv);
        assert_eq!(c.files[1].format, OutFormat::Parquet);
        assert_eq!(
            c.files.iter().map(|f| f.size).collect::<Vec<_>>(),
            [4, 4, 0]
        );
        assert!(c.rejected.is_empty() && !c.too_many_entries);
        let mut text = String::new();
        c.files
            .into_iter()
            .next()
            .unwrap()
            .into_file()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "x,y\n");
        assert_eq!(OutFormat::Csv.mime(), "text/csv");
    }

    /// The name is chosen by the code: anything outside the charset is refused
    /// and never echoed, whatever it holds.
    #[test]
    fn a_name_outside_the_charset_is_refused_and_never_echoed() {
        let d = tempfile::tempdir().unwrap();
        let bad: Vec<Vec<u8>> = vec![
            b"has space.csv".to_vec(),
            b"semi;colon.csv".to_vec(),
            b".hidden.csv".to_vec(),
            b"-flag.csv".to_vec(),
            b"UPPER.CSV".to_vec(),
            b"noext".to_vec(),
            b"script.sh".to_vec(),
            b"x.csv.exe".to_vec(),
            b"\xff\xfe.csv".to_vec(),
            "caf\u{e9}.csv".as_bytes().to_vec(),
            b"line\nbreak.csv".to_vec(),
            [b"a".repeat(OUT_NAME_MAX - 3), b".csv".to_vec()].concat(),
        ];
        // Some file systems (macOS's) refuse a name that is not UTF-8 outright.
        let mut made = 0;
        for raw in &bad {
            if std::fs::write(d.path().join(OsStr::from_bytes(raw)), b"x").is_ok() {
                made += 1;
            }
        }
        assert!(
            made >= bad.len() - 1,
            "only the non-UTF-8 name may be unmakeable"
        );
        put(d.path(), "ok.csv", b"x");
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert_eq!(names(&c), ["ok.csv"]);
        assert_eq!(c.rejected.len(), made);
        assert!(c
            .rejected
            .iter()
            .all(|r| r.name.is_none() && r.reason == RejectReason::BadName));
        // Exactly at the length limit is fine.
        let d = tempfile::tempdir().unwrap();
        put(
            d.path(),
            &format!("{}.csv", "a".repeat(OUT_NAME_MAX - 4)),
            b"x",
        );
        assert_eq!(
            collect_out(d.path(), Default::default())
                .unwrap()
                .files
                .len(),
            1
        );
    }

    /// A link is never followed: its target is not read, whatever it points at.
    #[test]
    fn a_link_is_refused_and_its_target_is_never_opened() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let secret = elsewhere.path().join("secret");
        std::fs::write(&secret, b"top secret").unwrap();
        std::os::unix::fs::symlink(&secret, d.path().join("leak.csv")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), d.path().join("dir.parquet")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", d.path().join("dangling.csv")).unwrap();
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert!(c.files.is_empty());
        assert_eq!(reasons(&c), [RejectReason::NotARegularFile; 3]);
        let shown: Vec<_> = c.rejected.iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(shown, ["dangling.csv", "dir.parquet", "leak.csv"]);
    }

    /// A pipe would block a read forever: it is opened without blocking, found
    /// not to be a regular file, and dropped.
    #[test]
    fn a_pipe_is_refused_without_blocking() {
        let d = tempfile::tempdir().unwrap();
        let fifo = CString::new(d.path().join("pipe.csv").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        put(d.path(), "ok.csv", b"x");
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert_eq!(names(&c), ["ok.csv"]);
        assert_eq!(reasons(&c), [RejectReason::NotARegularFile]);
    }

    #[test]
    fn a_directory_is_refused_and_not_entered() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub.csv")).unwrap();
        put(&d.path().join("sub.csv"), "inner.csv", b"x");
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert!(c.files.is_empty());
        assert_eq!(reasons(&c), [RejectReason::NotARegularFile]);
    }

    /// Two names for one file: neither is kept, so a hard link cannot make a
    /// file count twice or point at something outside what was checked.
    #[test]
    fn a_hard_linked_file_is_refused_under_every_name() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "one.csv", b"x");
        std::fs::hard_link(d.path().join("one.csv"), d.path().join("two.csv")).unwrap();
        put(d.path(), "solo.csv", b"x");
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert_eq!(names(&c), ["solo.csv"]);
        assert_eq!(reasons(&c), [RejectReason::HardLinked; 2]);
    }

    /// Only the logical size counts: a sparse file allocates nothing and still
    /// returns this many bytes on a read.
    #[test]
    fn a_sparse_file_is_judged_by_its_logical_size() {
        let d = tempfile::tempdir().unwrap();
        let f = File::create(d.path().join("sparse.parquet")).unwrap();
        f.set_len(2 * 1024 * 1024 * 1024).unwrap();
        put(d.path(), "small.csv", b"x");
        let limits = CollectLimits {
            file_bytes: 1024 * 1024,
            ..CollectLimits::default()
        };
        let c = collect_out(d.path(), limits).unwrap();
        assert_eq!(names(&c), ["small.csv"]);
        assert_eq!(reasons(&c), [RejectReason::TooLarge]);
        assert_eq!(c.rejected[0].name.as_deref(), Some("sparse.parquet"));
    }

    #[test]
    fn the_file_and_total_limits_are_exact() {
        let d = tempfile::tempdir().unwrap();
        for (name, len) in [("a.csv", 40), ("b.csv", 40), ("c.csv", 41), ("d.csv", 100)] {
            put(d.path(), name, &vec![b'x'; len]);
        }
        let limits = CollectLimits {
            max_files: 8,
            file_bytes: 100,
            total_bytes: 121,
            max_entries: 64,
        };
        let c = collect_out(d.path(), limits).unwrap();
        // 40 + 40 + 41 = 121 fits exactly; the 100-byte file would reach 221.
        assert_eq!(names(&c), ["a.csv", "b.csv", "c.csv"]);
        assert_eq!(reasons(&c), [RejectReason::OverTotal]);
        let limits = CollectLimits {
            file_bytes: 99,
            ..limits
        };
        let c = collect_out(d.path(), limits).unwrap();
        assert_eq!(reasons(&c), [RejectReason::TooLarge]);
    }

    #[test]
    fn files_past_the_count_are_refused_by_name_and_the_rest_kept() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..10 {
            put(d.path(), &format!("f{i}.csv"), b"x");
        }
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert_eq!(c.files.len(), OUT_MAX_FILES);
        assert_eq!(reasons(&c), [RejectReason::OverFileCount; 2]);
        assert_eq!(c.rejected[0].name.as_deref(), Some("f8.csv"));
    }

    /// More entries than the cap: nothing is kept, and the walk stopped one past
    /// the cap instead of reading the whole directory.
    #[test]
    fn a_volume_with_too_many_entries_keeps_nothing() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..=OUT_MAX_ENTRIES {
            put(d.path(), &format!("f{i}.csv"), b"x");
        }
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert!(c.too_many_entries && c.files.is_empty());
        // Exactly at the cap is not too many.
        std::fs::remove_file(d.path().join("f0.csv")).unwrap();
        let c = collect_out(d.path(), CollectLimits::default()).unwrap();
        assert!(!c.too_many_entries);
        assert_eq!(c.files.len(), OUT_MAX_FILES);
        assert_eq!(c.rejected.len(), OUT_MAX_ENTRIES - OUT_MAX_FILES);
    }

    /// The directory itself is opened without following a link, and a missing
    /// one is an error, not an empty answer.
    #[test]
    fn the_directory_is_not_followed_if_it_is_a_link_and_must_exist() {
        let real = tempfile::tempdir().unwrap();
        put(real.path(), "a.csv", b"x");
        let holder = tempfile::tempdir().unwrap();
        let link = holder.path().join("out");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        assert!(collect_out(&link, CollectLimits::default()).is_err());
        assert!(collect_out(&holder.path().join("missing"), CollectLimits::default()).is_err());
        let file = holder.path().join("plain");
        std::fs::write(&file, b"x").unwrap();
        assert!(
            collect_out(&file, CollectLimits::default()).is_err(),
            "not a directory"
        );
    }

    /// What is held is the descriptor that was checked: swapping the name for a
    /// link afterwards changes nothing about what the caller reads.
    #[test]
    fn a_kept_file_is_the_one_that_was_checked_even_if_the_name_is_swapped() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "a.csv", b"checked");
        let mut c = collect_out(d.path(), CollectLimits::default()).unwrap();
        std::fs::remove_file(d.path().join("a.csv")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", d.path().join("a.csv")).unwrap();
        let mut text = String::new();
        c.files
            .remove(0)
            .into_file()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "checked");
    }

    #[test]
    fn the_limits_are_the_designs() {
        let l = CollectLimits::default();
        assert_eq!(l.max_files, 8);
        assert_eq!(l.file_bytes, 64 * 1024 * 1024);
        assert_eq!(l.total_bytes, 2 * l.file_bytes);
        assert_eq!(l.max_entries, 64);
    }
}
