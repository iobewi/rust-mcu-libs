#![no_std]

//! Local log capture: a bounded ring of formatted lines and the global
//! `log` logger that fills it. Knows nothing about any network, transport or
//! streaming policy -- a firmware without a network uses this crate alone.
//!
//! There is exactly one logger and one ring per image: both are
//! process-lifetime statics owned here; consumers (such as `iobewi-log-stream`)
//! read the same ring through [`pop_line`] / [`discard`].

use core::{cell::RefCell, fmt::Write as _};
use critical_section::Mutex;
use heapless::{Deque, String as FixedString, Vec};
pub use log::LevelFilter;
use log::{Level, Metadata, Record};
use static_cell::StaticCell;

/// Maximum captured length of one log line (longer lines are dropped).
pub const LINE_MAX: usize = 160;
/// Number of lines the ring holds; when full, new lines are dropped.
pub const RING_CAPACITY: usize = 24;

/// One captured, formatted log line.
pub type Line = FixedString<LINE_MAX>;

/// Maximum target length, in UTF-8 bytes. Oversized targets are dropped.
pub const TARGET_MAX: usize = 64;
/// Maximum overrides in a policy.
pub const TARGET_RULES_MAX: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetRule {
    pub target: FixedString<TARGET_MAX>,
    pub level: LevelFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    EmptyTarget,
    TargetTooLong,
    TooManyRules,
    DuplicateTarget,
}

/// Bounded prefix rules. Longest matching prefix wins; otherwise default_level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogPolicy {
    pub default_level: LevelFilter,
    targets: Vec<TargetRule, TARGET_RULES_MAX>,
}

impl LogPolicy {
    pub const fn new(default_level: LevelFilter) -> Self {
        Self {
            default_level,
            targets: Vec::new(),
        }
    }

    pub fn add_target(&mut self, target: &str, level: LevelFilter) -> Result<(), PolicyError> {
        if target.is_empty() {
            return Err(PolicyError::EmptyTarget);
        }
        let target = FixedString::try_from(target).map_err(|_| PolicyError::TargetTooLong)?;
        if self.targets.iter().any(|rule| rule.target == target) {
            return Err(PolicyError::DuplicateTarget);
        }
        self.targets
            .push(TargetRule { target, level })
            .map_err(|_| PolicyError::TooManyRules)
    }

    pub fn targets(&self) -> &[TargetRule] {
        &self.targets
    }

    pub fn level_for(&self, target: &str) -> LevelFilter {
        self.targets
            .iter()
            .filter(|rule| target.starts_with(rule.target.as_str()))
            .max_by_key(|rule| rule.target.len())
            .map_or(self.default_level, |rule| rule.level)
    }

    /// Conservative facade ceiling, including every override.
    pub fn max_level(&self) -> LevelFilter {
        self.targets
            .iter()
            .fold(self.default_level, |max, rule| max.max(rule.level))
    }

    pub fn enabled(&self, level: Level, target: &str) -> bool {
        level <= self.level_for(target)
    }
}

/// Compatibility policy: legacy raw prefix matching.
/// A legacy application prefix may be empty or exceed TARGET_MAX, so it is
/// retained separately in static logger storage rather than silently shortened.
pub fn default_policy(application_target: &str) -> Result<LogPolicy, PolicyError> {
    let mut policy = LogPolicy::new(LevelFilter::Warn);
    policy.add_target("iobewi_log", LevelFilter::Info)?;
    if application_target != "iobewi_log" {
        policy.add_target(application_target, LevelFilter::Info)?;
    }
    Ok(policy)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedRecord {
    pub level: Level,
    pub target: FixedString<TARGET_MAX>,
    pub message: Line,
}

static RING: Mutex<RefCell<Deque<CapturedRecord, RING_CAPACITY>>> =
    Mutex::new(RefCell::new(Deque::new()));
static POLICY: Mutex<RefCell<Option<LogPolicy>>> = Mutex::new(RefCell::new(None));
static LOGGER: StaticCell<Logger> = StaticCell::new();

struct Logger {
    print: fn(&Record<'_>),
    application_target: &'static str,
}

/// Atomically replace the effective policy. Existing records are retained.
/// Policy and facade ceiling are updated in the same critical section so
/// concurrent writers cannot leave the ceiling inconsistent with the policy.
/// Calls overlapping a change may observe the old or new facade ceiling.
pub fn apply_policy(policy: LogPolicy) {
    critical_section::with(|cs| {
        let max = policy.max_level();
        *POLICY.borrow(cs).borrow_mut() = Some(policy);
        log::set_max_level(max);
    });
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        critical_section::with(|cs| {
            POLICY.borrow(cs).borrow().as_ref().map_or_else(
                || {
                    let level = if metadata.target().starts_with(self.application_target)
                        || metadata.target().starts_with("iobewi_log")
                    {
                        LevelFilter::Info
                    } else {
                        LevelFilter::Warn
                    };
                    metadata.level() <= level
                },
                |policy| policy.enabled(metadata.level(), metadata.target()),
            )
        })
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        (self.print)(record);
        let mut line: Line = FixedString::new();
        if write!(line, "{}", record.args()).is_err() {
            return;
        }
        let Ok(target) = FixedString::try_from(record.target()) else {
            return;
        };
        let captured = CapturedRecord {
            level: record.level(),
            target,
            message: line,
        };
        critical_section::with(|cs| {
            let mut ring = RING.borrow(cs).borrow_mut();
            if !ring.is_full() {
                let _ = ring.push_back(captured);
            }
        });
    }

    fn flush(&self) {}
}

/// Install once, during single-threaded startup, before other code logs.
/// The platform owns local console output and chooses the app log target.
pub fn install(print: fn(&Record<'_>), application_target: &'static str) {
    let logger = LOGGER.init(Logger {
        print,
        application_target,
    });
    // SAFETY: the logger is process-lifetime storage and installation takes
    // place only once, before the executor and interrupt-driven loggers start.
    unsafe {
        let _ = log::set_logger_racy(logger);
        critical_section::with(|cs| {
            let max = POLICY
                .borrow(cs)
                .borrow()
                .as_ref()
                .map_or(LevelFilter::Info, LogPolicy::max_level);
            log::set_max_level(max);
        });
    }
}

/// Removes and returns the oldest captured line.
pub fn pop_line() -> Option<Line> {
    pop_record().map(|record| record.message)
}

/// Removes the oldest record, preserving its original level and target.
pub fn pop_record() -> Option<CapturedRecord> {
    critical_section::with(|cs| RING.borrow(cs).borrow_mut().pop_front())
}

/// Drops every captured line.
pub fn discard() {
    critical_section::with(|cs| RING.borrow(cs).borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use log::Log;

    fn noop(_: &Record<'_>) {}

    fn emit(logger: &Logger, level: Level, target: &str, text: &str) {
        logger.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{text}"))
                .build(),
        );
    }

    // One sequential test: the ring is a process-wide static shared by all tests.
    #[test]
    fn ring_capture_levels_overflow_and_discard() {
        let logger = Logger {
            print: noop,
            application_target: "app",
        };
        discard();

        // Application and the iobewi_log family log at Info, others only at Warn.
        emit(&logger, Level::Info, "app::sub", "app info");
        emit(&logger, Level::Info, "iobewi_log_stream", "stream info");
        emit(&logger, Level::Info, "smoltcp", "dropped info");
        emit(&logger, Level::Warn, "smoltcp", "kept warn");
        assert_eq!(pop_line().unwrap().as_str(), "app info");
        assert_eq!(pop_line().unwrap().as_str(), "stream info");
        assert_eq!(pop_line().unwrap().as_str(), "kept warn");
        assert!(pop_line().is_none());

        // A line longer than LINE_MAX is dropped, not truncated.
        let long = "x".repeat(LINE_MAX + 1);
        emit(&logger, Level::Warn, "app", &long);
        assert!(pop_line().is_none());

        // When the ring is full, new lines are dropped (oldest are kept).
        for i in 0..RING_CAPACITY + 5 {
            emit(&logger, Level::Warn, "app", &std::format!("line {i}"));
        }
        assert_eq!(pop_line().unwrap().as_str(), "line 0");
        let mut n = 1;
        while pop_line().is_some() {
            n += 1;
        }
        assert_eq!(n, RING_CAPACITY);

        // Runtime updates do not reinstall the global logger.
        install(noop, "app");
        assert_eq!(log::max_level(), LevelFilter::Info);
        log::debug!(target: "app", "legacy debug rejected");
        log::trace!(target: "app", "legacy trace rejected");
        assert!(pop_record().is_none());
        for level in [
            LevelFilter::Off,
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            apply_policy(LogPolicy::new(level));
            assert_eq!(log::max_level(), level);
        }
        let mut override_policy = LogPolicy::new(LevelFilter::Off);
        override_policy
            .add_target("app", LevelFilter::Debug)
            .unwrap();
        apply_policy(override_policy);
        assert_eq!(log::max_level(), LevelFilter::Debug);
        apply_policy(LogPolicy::new(LevelFilter::Off));
        struct NeverFormat;
        impl core::fmt::Display for NeverFormat {
            fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                panic!("Off must reject before formatting");
            }
        }
        fn must_not_print(_: &Record<'_>) {
            panic!("Off must reject before console");
        }
        let silent = Logger {
            print: must_not_print,
            application_target: "app",
        };
        silent.log(
            &Record::builder()
                .level(Level::Error)
                .target("app")
                .args(format_args!("{}", NeverFormat))
                .build(),
        );
        assert!(!log::log_enabled!(target: "app", Level::Error));
        assert!(pop_record().is_none());

        let mut policy = LogPolicy::new(LevelFilter::Error);
        policy.add_target("app", LevelFilter::Debug).unwrap();
        policy.add_target("app::usb", LevelFilter::Trace).unwrap();
        policy
            .add_target("app::usb::quiet", LevelFilter::Off)
            .unwrap();
        assert_eq!(policy.level_for("other"), LevelFilter::Error);
        assert_eq!(policy.level_for("app::usb::task"), LevelFilter::Trace);
        assert_eq!(policy.level_for("app::usb::quiet"), LevelFilter::Off);
        assert_eq!(
            policy.add_target("app", LevelFilter::Info),
            Err(PolicyError::DuplicateTarget)
        );
        assert_eq!(
            policy.add_target("", LevelFilter::Info),
            Err(PolicyError::EmptyTarget)
        );
        assert_eq!(
            policy.add_target(&"x".repeat(TARGET_MAX + 1), LevelFilter::Info),
            Err(PolicyError::TargetTooLong)
        );
        apply_policy(policy);
        assert_eq!(log::max_level(), LevelFilter::Trace);
        log::debug!(target: "app", "runtime debug");
        log::trace!(target: "app::usb", "runtime trace");
        log::trace!(target: "app", "rejected");
        log::warn!(target: "other", "rejected");
        let record = pop_record().unwrap();
        assert_eq!(record.level, Level::Debug);
        assert_eq!(record.target.as_str(), "app");
        assert_eq!(record.message.as_str(), "runtime debug");
        assert_eq!(pop_record().unwrap().level, Level::Trace);
        assert!(pop_record().is_none());
        emit(
            &logger,
            Level::Error,
            &"x".repeat(TARGET_MAX + 1),
            "oversized target",
        );
        assert!(pop_record().is_none());
        emit(
            &logger,
            Level::Error,
            &"x".repeat(TARGET_MAX),
            &"x".repeat(LINE_MAX),
        );
        assert_eq!(pop_record().unwrap().message.len(), LINE_MAX);
        let mut bounded = LogPolicy::new(LevelFilter::Off);
        for i in 0..TARGET_RULES_MAX {
            bounded
                .add_target(&std::format!("t{i}"), LevelFilter::Info)
                .unwrap();
        }
        assert_eq!(
            bounded.add_target("overflow", LevelFilter::Info),
            Err(PolicyError::TooManyRules)
        );
        apply_policy(default_policy("app").unwrap());
        assert_eq!(log::max_level(), LevelFilter::Info);
        std::println!(
            "record={} ring={} policy={} legacy_ring={}",
            core::mem::size_of::<CapturedRecord>(),
            core::mem::size_of::<Deque<CapturedRecord, RING_CAPACITY>>(),
            core::mem::size_of::<LogPolicy>(),
            core::mem::size_of::<Deque<Line, RING_CAPACITY>>()
        );

        emit(&logger, Level::Warn, "app", "again");
        discard();
        assert!(pop_line().is_none());
    }
}
