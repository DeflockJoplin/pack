//! Boot-time upload phase and progress for `upload_on_boot`.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootUploadPhase {
    Skipped,
    SkippedOffline,
    Pending,
    InProgress,
    Complete,
    Failed,
}

impl BootUploadPhase {
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::SkippedOffline,
            2 => Self::Pending,
            3 => Self::InProgress,
            4 => Self::Complete,
            5 => Self::Failed,
            _ => Self::Skipped,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::SkippedOffline => "skipped_offline",
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

pub struct BootUploadStatus {
    phase: AtomicU8,
    pub started_ms: AtomicU64,
    pub finished_ms: AtomicU64,
    pub current_file: StdRwLock<Option<String>>,
    pub sessions_total: AtomicU32,
    pub sessions_done: AtomicU32,
    pub last_error: StdRwLock<Option<String>>,
}

impl BootUploadStatus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicU8::new(BootUploadPhase::Skipped as u8),
            started_ms: AtomicU64::new(0),
            finished_ms: AtomicU64::new(0),
            current_file: StdRwLock::new(None),
            sessions_total: AtomicU32::new(0),
            sessions_done: AtomicU32::new(0),
            last_error: StdRwLock::new(None),
        })
    }

    #[must_use]
    pub fn phase(&self) -> BootUploadPhase {
        BootUploadPhase::from_u8(self.phase.load(Ordering::Relaxed))
    }

    pub fn set_phase(&self, p: BootUploadPhase) {
        self.phase.store(p as u8, Ordering::Relaxed);
    }

    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.phase() == BootUploadPhase::InProgress
    }

    pub fn set_current_file(&self, name: Option<String>) {
        if let Ok(mut g) = self.current_file.write() {
            *g = name;
        }
    }

    pub fn set_error(&self, msg: Option<String>) {
        if let Ok(mut g) = self.last_error.write() {
            *g = msg;
        }
    }

    pub fn clear_progress(&self) {
        self.set_current_file(None);
        self.sessions_total.store(0, Ordering::Relaxed);
        self.sessions_done.store(0, Ordering::Relaxed);
        self.set_error(None);
    }
}

/// Set after monitor setup + capture threads are started.
pub struct CaptureLifecycle {
    pub capture_started: AtomicBool,
}

impl CaptureLifecycle {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            capture_started: AtomicBool::new(false),
        })
    }
}
