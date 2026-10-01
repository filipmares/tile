//! Persistent diagnostics.
//!
//! Tile is a windowless tray app: in a release build on Windows there is no
//! console, so anything written only to stderr is lost. Every log record is
//! therefore also appended to a size-capped, rotating file in the per-user data
//! directory, so a launch that never happened, a silent exit, or a panic leaves
//! a trail that can be read afterwards.
//!
//! Alongside the log, a small session marker is written while Tile runs and
//! removed on every orderly exit (tray Quit, Windows sign-out/shutdown, an
//! updater handoff). Finding it at startup means the previous process ended
//! without passing through any of those — it was killed, it crashed, or the
//! machine lost power — and that is logged as a warning.

use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::ProjectDirs;

use crate::build_kind::BuildKind;

/// The file currently being written.
pub const LOG_FILE_NAME: &str = "tile.log";
/// Rotate once the current file would grow past this.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// How many rotated files (`tile.1.log` … `tile.N.log`) to keep.
const KEPT_ROTATIONS: usize = 4;
/// Present while a Tile process is running; see the module docs.
const SESSION_MARKER: &str = "session.running";

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where the logs of this kind of build live.
///
/// Development builds log beside their own config, never into the installed
/// copy's log, for the same reason they keep a separate config.
pub fn log_dir(kind: BuildKind) -> Option<PathBuf> {
    ProjectDirs::from("dev", "Tile", kind.project_app_name())
        .map(|dirs| dirs.data_dir().join("logs"))
}

/// The directory logs are being written to, once [`init`] has succeeded.
pub fn active_log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

/// Installs the global logger and the panic hook. Must run before anything
/// else logs. Never fails: without a writable log directory Tile still logs to
/// stderr.
///
/// The level defaults to `info` and can be overridden with `RUST_LOG`.
pub fn init(kind: BuildKind) {
    let file = log_dir(kind).and_then(|dir| match RotatingFile::open(&dir) {
        Ok(file) => {
            let _ = LOG_DIR.set(dir);
            Some(file)
        }
        Err(err) => {
            eprintln!(
                "tile: could not open a log file in {}: {err}",
                dir.display()
            );
            None
        }
    });

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder
        .format(|buf, record| {
            let thread = std::thread::current();
            writeln!(
                buf,
                "{} {:<5} [{}:{}] {}: {}",
                buf.timestamp_millis(),
                record.level(),
                std::process::id(),
                thread.name().unwrap_or("?"),
                record.target(),
                record.args()
            )
        })
        .write_style(env_logger::WriteStyle::Never)
        .target(env_logger::Target::Pipe(Box::new(Sink { file })));
    // Logging must never take down the app; ignore a double-init.
    let _ = builder.try_init();

    install_panic_hook();
}

/// Records that this process is now the running Tile, and reports whether the
/// previous one ended without an orderly exit.
///
/// Call only from the instance that actually owns the app (after the
/// single-instance handoff), or a short-lived second launch would mistake the
/// running copy's marker for a crash.
pub fn begin_session() {
    let Some(dir) = active_log_dir() else {
        return;
    };
    let marker = dir.join(SESSION_MARKER);
    if let Ok(previous) = fs::read_to_string(&marker) {
        log::warn!(
            "the previous Tile session ({}) did not exit cleanly: it was killed (for example \
             by an installer), crashed, or the machine lost power. Its last lines are at the \
             end of this file or of tile.1.log",
            previous.trim()
        );
    }
    let contents = format!(
        "pid={} version={}\n",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );
    if let Err(err) = fs::write(&marker, contents) {
        log::warn!("could not write the session marker: {err}");
    }
}

/// Marks the current session as having exited in an orderly way and flushes
/// the log. Safe to call more than once.
pub fn end_session(reason: &str) {
    log::info!("Tile is exiting: {reason}");
    if let Some(dir) = active_log_dir() {
        let _ = fs::remove_file(dir.join(SESSION_MARKER));
    }
    log::logger().flush();
}

/// Logs every panic with its thread and a backtrace before the default hook
/// runs. The release profile aborts on panic, so this is the only record a
/// panic leaves.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let backtrace = Backtrace::force_capture();
        log::error!("panic on thread '{name}': {info}\nbacktrace:\n{backtrace}");
        log::logger().flush();
        previous(info);
    }));
}

/// The logger's output: the rotating file when there is one, and stderr
/// always (a no-op in a windowless release build, the terminal in development).
struct Sink {
    file: Option<RotatingFile>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(file) = &mut self.file {
            if let Err(err) = file.write_record(buf) {
                eprintln!("tile: log file write failed: {err}");
            }
        }
        let _ = io::stderr().write_all(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            let _ = file.file.flush();
        }
        let _ = io::stderr().flush();
        Ok(())
    }
}

/// `tile.log`, rolled to `tile.1.log` (and so on) once it reaches
/// [`MAX_FILE_BYTES`]. Unbuffered, so a record is on disk as soon as it is
/// logged — which is the whole point when the process is about to vanish.
struct RotatingFile {
    dir: PathBuf,
    file: File,
    len: u64,
}

impl RotatingFile {
    fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOG_FILE_NAME);
        if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= MAX_FILE_BYTES {
            rotate(dir, KEPT_ROTATIONS)?;
        }
        let file = open_append(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            len,
        })
    }

    fn write_record(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.len > 0 && self.len + buf.len() as u64 > MAX_FILE_BYTES {
            rotate(&self.dir, KEPT_ROTATIONS)?;
            self.file = open_append(&self.dir.join(LOG_FILE_NAME))?;
            self.len = 0;
        }
        self.file.write_all(buf)?;
        self.len += buf.len() as u64;
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotated_name(index: usize) -> String {
    format!("tile.{index}.log")
}

/// Shifts `tile.log` → `tile.1.log` → … → `tile.{keep}.log`, dropping the
/// oldest. Works while the current file is still open: Rust opens files with
/// `FILE_SHARE_DELETE` on Windows, which is what permits the rename.
fn rotate(dir: &Path, keep: usize) -> io::Result<()> {
    if keep == 0 {
        return match fs::remove_file(dir.join(LOG_FILE_NAME)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    let _ = fs::remove_file(dir.join(rotated_name(keep)));
    for index in (1..keep).rev() {
        let from = dir.join(rotated_name(index));
        if from.exists() {
            fs::rename(&from, dir.join(rotated_name(index + 1)))?;
        }
    }
    let current = dir.join(LOG_FILE_NAME);
    if current.exists() {
        fs::rename(&current, dir.join(rotated_name(1)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("tile-log-test-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn read(&self, name: &str) -> Option<String> {
            fs::read_to_string(self.0.join(name)).ok()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rotation_shifts_every_file_and_drops_the_oldest() {
        let dir = TempDir::new();
        fs::write(dir.0.join(LOG_FILE_NAME), "current").unwrap();
        fs::write(dir.0.join("tile.1.log"), "one").unwrap();
        fs::write(dir.0.join("tile.2.log"), "two").unwrap();

        rotate(&dir.0, 2).unwrap();

        assert_eq!(dir.read(LOG_FILE_NAME), None);
        assert_eq!(dir.read("tile.1.log").as_deref(), Some("current"));
        assert_eq!(dir.read("tile.2.log").as_deref(), Some("one"));
        assert_eq!(dir.read("tile.3.log"), None, "beyond `keep` is dropped");
    }

    #[test]
    fn rotation_tolerates_missing_files() {
        let dir = TempDir::new();
        rotate(&dir.0, KEPT_ROTATIONS).unwrap();
        fs::write(dir.0.join(LOG_FILE_NAME), "only").unwrap();
        rotate(&dir.0, KEPT_ROTATIONS).unwrap();
        assert_eq!(dir.read("tile.1.log").as_deref(), Some("only"));
    }

    #[test]
    fn a_full_file_rolls_over_before_the_record_that_would_overflow_it() {
        let dir = TempDir::new();
        let mut file = RotatingFile::open(&dir.0).unwrap();
        let big = vec![b'a'; MAX_FILE_BYTES as usize - 1];
        file.write_record(&big).unwrap();
        file.write_record(b"next\n").unwrap();

        assert_eq!(dir.read(LOG_FILE_NAME).as_deref(), Some("next\n"));
        assert_eq!(
            fs::metadata(dir.0.join("tile.1.log")).unwrap().len(),
            MAX_FILE_BYTES - 1
        );
    }

    #[test]
    fn an_oversized_file_from_a_previous_run_is_rotated_on_open() {
        let dir = TempDir::new();
        fs::write(
            dir.0.join(LOG_FILE_NAME),
            vec![b'a'; MAX_FILE_BYTES as usize],
        )
        .unwrap();
        let file = RotatingFile::open(&dir.0).unwrap();
        assert_eq!(file.len, 0);
        assert!(dir.0.join("tile.1.log").exists());
    }

    #[test]
    fn a_single_record_larger_than_the_cap_is_still_written() {
        let dir = TempDir::new();
        let mut file = RotatingFile::open(&dir.0).unwrap();
        let huge = vec![b'b'; MAX_FILE_BYTES as usize + 10];
        file.write_record(&huge).unwrap();
        assert_eq!(file.len, MAX_FILE_BYTES + 10);
    }
}
