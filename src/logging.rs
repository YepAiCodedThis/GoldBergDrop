//! File logger → `%APPDATA%\GoldbergDrop\GoldbergDrop\data\debug.log`
//!
//! Always on at Debug so failures are reconstructible without a rebuild.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const LOG_FILE: &str = "debug.log";
const LOG_PREV: &str = "debug.prev.log";
const MAX_BYTES: u64 = 2 * 1024 * 1024;

struct FileLogger {
    file: Mutex<Option<File>>,
}

static LOGGER: FileLogger = FileLogger {
    file: Mutex::new(None),
};

pub fn log_path() -> Option<PathBuf> {
    crate::settings::app_data_dir()
        .ok()
        .map(|d| d.join(LOG_FILE))
}

/// Initialize global logger + panic hook. Safe to call once at process start.
pub fn init() {
    let path = match crate::settings::app_data_dir() {
        Ok(dir) => dir.join(LOG_FILE),
        Err(_) => return,
    };
    rotate_if_huge(&path);

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();

    if let Ok(mut slot) = LOGGER.file.lock() {
        *slot = file;
    }

    let _ = log::set_logger(&LOGGER);
    log::set_max_level(LevelFilter::Debug);

    let panic_path = path.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("PANIC: {info}");
        let _ = append_raw(&panic_path, &format!("{} [PANIC] {}\n", timestamp(), msg));
        prev_hook(info);
    }));

    log::info!(
        "=== GoldbergDrop {} start pid={} exe={:?} args={:?} ===",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_exe().ok(),
        std::env::args().collect::<Vec<_>>(),
    );
    log::info!("log file: {}", path.display());
}

fn rotate_if_huge(path: &PathBuf) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < MAX_BYTES {
        return;
    }
    let prev = path.with_file_name(LOG_PREV);
    let _ = fs::remove_file(&prev);
    let _ = fs::rename(path, &prev);
}

fn timestamp() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLocalTime(lp_system_time: *mut SystemTime);
        }
        let mut st = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        unsafe {
            GetLocalTime(&mut st);
        }
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            st.year, st.month, st.day, st.hour, st.minute, st.second, st.milliseconds
        );
    }
    #[cfg(not(windows))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{secs}")
    }
}

fn append_raw(path: &PathBuf, line: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} [{:<5}] {}:{}: {}\n",
            timestamp(),
            record.level(),
            record.module_path().unwrap_or("?"),
            record.line().unwrap_or(0),
            record.args()
        );
        if let Ok(mut slot) = self.file.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
                return;
            }
        }
        // Fallback if init failed half-way
        if let Some(path) = log_path() {
            append_raw(&path, &line);
        }
    }

    fn flush(&self) {
        if let Ok(mut slot) = self.file.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = f.flush();
            }
        }
    }
}
