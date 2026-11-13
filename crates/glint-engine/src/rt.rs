// SPDX-License-Identifier: MIT
//! How the hot thread is scheduled (PLAN §5.6, ADR 0017): real-time priority through rtkit,
//! normal priority under Max, CPU pinning, and locking memory. The hot thread sets its own timer
//! slack (`hot.rs`); everything here runs on the controller, never on the hot thread.

use glint_core::settings::{CpuPin, Realtime};
use rustix::process::{Pid, Resource, Rlimit};
use rustix::thread::CpuSet;

const RTKIT: &str = "org.freedesktop.RealtimeKit1";
const RTKIT_PATH: &str = "/org/freedesktop/RealtimeKit1";

/// The SCHED_RR priority Glint asks for, at most rtkit's maximum (§10's "rtkit (rr 10)").
pub const PRIORITY: u32 = 10;

/// What the hot thread runs at.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Priority {
    /// Real-time (SCHED_RR) at this priority, granted by rtkit.
    Realtime(u32),
    /// Normal: real-time is off in the settings.
    Off,
    /// Normal: Max never runs real-time (§5.6).
    NormalForMax,
    /// Normal: real-time was asked for and not granted.
    Refused(String),
    /// Normal: dropped after the hot thread ran too long without a pause (SIGXCPU). It stays
    /// dropped until the engine restarts.
    Dropped,
}

impl Priority {
    /// How `state` and the UI say it.
    pub fn describe(&self) -> String {
        match self {
            Self::Realtime(p) => format!("rtkit (rr {p})"),
            Self::Off => "off".into(),
            Self::NormalForMax => "normal (Max never runs real-time)".into(),
            Self::Refused(why) => format!("off: {why}"),
            Self::Dropped => "off: dropped after the hot thread ran without a pause".into(),
        }
    }

    pub fn is_realtime(&self) -> bool {
        matches!(self, Self::Realtime(_))
    }
}

/// Whether the hot thread should run real-time: unless the settings say off, and never under
/// Max, whose thread never sleeps between checks (§5.6).
pub fn wants_realtime(setting: Realtime, max_speed: bool) -> bool {
    setting != Realtime::Off && !max_speed
}

/// A connection to rtkit on the system bus.
pub struct Rtkit {
    conn: zbus::blocking::Connection,
    max_priority: u32,
    rttime_max_us: u64,
}

impl Rtkit {
    pub fn connect() -> Result<Self, String> {
        let conn = zbus::blocking::Connection::system().map_err(|e| format!("no system bus ({e})"))?;
        let proxy = proxy(&conn)?;
        let max: i32 = proxy
            .get_property("MaxRealtimePriority")
            .map_err(|e| format!("rtkit isn't available ({e})"))?;
        let rttime: i64 = proxy
            .get_property("RTTimeUSecMax")
            .map_err(|e| format!("rtkit isn't available ({e})"))?;
        Ok(Self {
            conn,
            max_priority: u32::try_from(max).unwrap_or(0),
            rttime_max_us: u64::try_from(rttime).unwrap_or(0),
        })
    }

    /// Makes thread `tid` of this process real-time; returns the priority granted.
    pub fn realtime(&self, tid: i32) -> Result<u32, String> {
        let priority = PRIORITY.min(self.max_priority);
        if priority == 0 || self.rttime_max_us == 0 {
            return Err("rtkit allows no real-time priority".into());
        }
        // rtkit requires RLIMIT_RTTIME, at most RTTimeUSecMax: a real-time thread that runs that
        // long without blocking is killed. The soft limit, half of it, sends SIGXCPU first, and
        // the engine then drops the thread back to normal (ADR 0017).
        let hard = self.rttime_max_us;
        rustix::process::setrlimit(
            Resource::Rttime,
            Rlimit {
                current: Some(hard / 2),
                maximum: Some(hard),
            },
        )
        .map_err(|e| format!("couldn't set RLIMIT_RTTIME ({e})"))?;
        proxy(&self.conn)?
            .call::<_, _, ()>("MakeThreadRealtime", &(thread_id(tid)?, priority))
            .map_err(|e| format!("rtkit refused ({e})"))?;
        Ok(priority)
    }

    /// Puts thread `tid` back on the normal scheduler at nice 0: rtkit's MakeThreadHighPriority
    /// switches a thread to SCHED_OTHER and sets its nice level.
    pub fn normal(&self, tid: i32) -> Result<(), String> {
        proxy(&self.conn)?
            .call::<_, _, ()>("MakeThreadHighPriority", &(thread_id(tid)?, 0i32))
            .map_err(|e| format!("rtkit refused ({e})"))
    }
}

fn proxy(conn: &zbus::blocking::Connection) -> Result<zbus::blocking::Proxy<'static>, String> {
    zbus::blocking::Proxy::new(conn, RTKIT, RTKIT_PATH, RTKIT).map_err(|e| e.to_string())
}

fn thread_id(tid: i32) -> Result<u64, String> {
    u64::try_from(tid).map_err(|_| format!("bad thread id {tid}"))
}

/// Sets the calling thread's timer slack to 1 ns, so its sleeps end on time: by default Linux
/// lets them overshoot by 50 µs (§5.6). Real-time threads have no slack anyway.
pub fn tight_timer_slack() {
    let _ = rustix::thread::set_current_timer_slack(std::num::NonZeroU64::new(1));
}

/// The CPUs the engine may use, as it started (to undo a pin).
pub fn all_cpus() -> CpuSet {
    rustix::thread::sched_getaffinity(None).unwrap_or_default()
}

/// Pins thread `tid` as `cpu` says: `none` and `auto` let it run anywhere (Phase 0 measured no
/// benefit from pinning, §5.6), a core number pins it there. Returns how it's pinned.
pub fn pin(tid: i32, cpu: CpuPin, all: &CpuSet) -> Result<String, String> {
    let pid = Pid::from_raw(tid).ok_or_else(|| format!("bad thread id {tid}"))?;
    match cpu {
        CpuPin::None | CpuPin::Auto => {
            rustix::thread::sched_setaffinity(Some(pid), all).map_err(|e| e.to_string())?;
            Ok("any core".into())
        }
        CpuPin::Core(n) => {
            let core = usize::from(n);
            if core >= CpuSet::MAX_CPU || !all.is_set(core) {
                return Err(format!("core {n} isn't available"));
            }
            let mut one = CpuSet::new();
            one.set(core);
            rustix::thread::sched_setaffinity(Some(pid), &one).map_err(|e| e.to_string())?;
            Ok(format!("core {n}"))
        }
    }
}

/// Locks the engine's memory, so the hot thread never waits for a page to come back from swap
/// (§5.6). Only when the memory-lock limit is unlimited: with a finite limit, locking future
/// allocations could make a later one fail and end the engine (ADR 0017). Returns what it did.
pub fn lock_memory() -> String {
    use rustix::mm::MlockAllFlags;
    match rustix::process::getrlimit(Resource::Memlock).current {
        None => match rustix::mm::mlockall(MlockAllFlags::CURRENT | MlockAllFlags::FUTURE) {
            Ok(()) => "locked".into(),
            Err(e) => format!("not locked ({e})"),
        },
        Some(bytes) => format!("not locked: RLIMIT_MEMLOCK is {} KiB", bytes / 1024),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn real_time_unless_off_and_never_under_max() {
        assert!(wants_realtime(Realtime::Auto, false));
        assert!(wants_realtime(Realtime::Required, false));
        assert!(!wants_realtime(Realtime::Off, false));
        assert!(!wants_realtime(Realtime::Auto, true));
        assert!(!wants_realtime(Realtime::Required, true));
    }

    #[test]
    fn how_the_priority_reads() {
        assert_eq!(Priority::Realtime(10).describe(), "rtkit (rr 10)");
        assert_eq!(Priority::Off.describe(), "off");
        assert!(
            Priority::Refused("no system bus".into())
                .describe()
                .starts_with("off: ")
        );
        assert!(Priority::Realtime(10).is_realtime() && !Priority::Dropped.is_realtime());
    }

    #[test]
    fn the_timer_slack_is_one_nanosecond() {
        let slack = std::thread::spawn(|| {
            tight_timer_slack();
            rustix::thread::current_timer_slack()
        })
        .join()
        .unwrap();
        assert_eq!(slack.ok(), Some(1));
    }

    #[test]
    fn pinning_to_a_missing_core_is_refused() {
        let all = all_cpus();
        let tid = rustix::thread::gettid().as_raw_nonzero().get();
        assert!(pin(tid, CpuPin::Core(u16::MAX), &all).is_err());
        assert_eq!(pin(tid, CpuPin::None, &all).as_deref(), Ok("any core"));
    }
}
