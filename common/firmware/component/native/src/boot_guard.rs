//! Crash-loop guard: a defective `Valid` Workload must never make the Agent unrecoverable.
//!
//! *Invariant: Agent bootability > Workload availability.*
//!
//! A fault confined to the Workload core does **not** reset the chip (measured: the core
//! stops, the Agent keeps serving; the runtime then quarantines the Workload). What can still
//! loop is a Workload that takes the **whole chip** down, e.g. a watchdog reset or a
//! corrupted Agent memory (the model is trusted native code, there is no memory isolation). For
//! that case this guard counts *unclean* auto-starts across resets:
//!
//! ```text
//! boot:   record invalid            -> reset it (power-on clears RTC memory anyway)
//!         clean-reboot flag set     -> deliberate reboot (operator/OTA): attempts = 0
//!         attempts >= MAX           -> Suppressed: the Agent boots, the Workload is NOT started
//!         otherwise                 -> attempts += 1, start the Workload
//! run:    Healthy for 30 s          -> attempts = 0   ("proven healthy")
//! reboot: the Agent sets the flag just before a deliberate reset
//! ```
//!
//! The state lives where it survives a reset but not a power cycle (RTC fast RAM on the ESP):
//! **nothing is added to OTM2**. A suppressed Workload stays `Valid` in OTM2; it can be
//! replaced by an ordinary upload + activate, and a power cycle or a deliberate reboot gives it
//! a fresh set of attempts.

pub const MAGIC: u32 = 0x4947_5331; // "IGS1"
/// Consecutive unclean auto-starts tolerated before the Workload is suppressed.
pub const MAX_UNCLEAN_STARTS: u32 = 3;
/// How long a Workload must stay Healthy to clear the counter.
pub const PROVEN_HEALTHY_MS: u64 = 30_000;

/// Four words so it can live in any reset-surviving memory; sealed with a checksum because a
/// reset can interrupt a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardRecord {
    pub magic: u32,
    pub attempts: u32,
    pub clean: u32,
    pub check: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDecision {
    /// Start the Workload; this is auto-start number `attempts`.
    Start { attempts: u32 },
    /// Do not auto-start: the previous `attempts` starts all ended in an unclean reset.
    Suppressed { attempts: u32 },
}

impl GuardRecord {
    pub const EMPTY: GuardRecord = GuardRecord {
        magic: 0,
        attempts: 0,
        clean: 0,
        check: 0,
    };

    fn checksum(magic: u32, attempts: u32, clean: u32) -> u32 {
        magic ^ attempts.rotate_left(7) ^ clean.rotate_left(13) ^ 0xA5A5_5A5A
    }

    pub fn seal(&mut self) {
        self.magic = MAGIC;
        self.check = Self::checksum(self.magic, self.attempts, self.clean);
    }

    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC && self.check == Self::checksum(self.magic, self.attempts, self.clean)
    }

    pub fn from_words(w: [u32; 4]) -> Self {
        Self {
            magic: w[0],
            attempts: w[1],
            clean: w[2],
            check: w[3],
        }
    }

    pub fn to_words(self) -> [u32; 4] {
        [self.magic, self.attempts, self.clean, self.check]
    }
}

/// Decide at boot whether the Workload may be auto-started, updating `rec` (re-sealed).
pub fn on_boot(rec: &mut GuardRecord) -> BootDecision {
    if !rec.is_valid() {
        *rec = GuardRecord::EMPTY;
    }
    if rec.clean != 0 {
        rec.clean = 0;
        rec.attempts = 0;
    }
    let decision = if rec.attempts >= MAX_UNCLEAN_STARTS {
        BootDecision::Suppressed {
            attempts: rec.attempts,
        }
    } else {
        rec.attempts += 1;
        BootDecision::Start {
            attempts: rec.attempts,
        }
    };
    rec.seal();
    decision
}

/// The Agent is about to reset on purpose (operator reboot, Agent OTA): the next boot is not
/// evidence of a crash.
pub fn mark_clean_reboot(rec: &mut GuardRecord) {
    if !rec.is_valid() {
        *rec = GuardRecord::EMPTY;
    }
    rec.clean = 1;
    rec.seal();
}

/// The Workload has been Healthy long enough: forget past failures.
pub fn mark_proven_healthy(rec: &mut GuardRecord) {
    if !rec.is_valid() {
        *rec = GuardRecord::EMPTY;
    }
    rec.attempts = 0;
    rec.seal();
}

/// Turns periodic health observations into a single "proven healthy" event.
#[derive(Debug, Default)]
pub struct ProvenHealth {
    since: Option<u64>,
    fired: bool,
}

impl ProvenHealth {
    pub const fn new() -> Self {
        Self {
            since: None,
            fired: false,
        }
    }

    /// Feed one observation; returns `true` exactly once, when the Workload has been
    /// continuously Healthy for [`PROVEN_HEALTHY_MS`].
    pub fn observe(&mut self, healthy: bool, now_ms: u64) -> bool {
        if !healthy {
            self.since = None;
            self.fired = false;
            return false;
        }
        let since = *self.since.get_or_insert(now_ms);
        if !self.fired && now_ms.saturating_sub(since) >= PROVEN_HEALTHY_MS {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> GuardRecord {
        let mut r = GuardRecord::EMPTY;
        r.seal();
        r
    }

    #[test]
    fn an_unwritten_or_torn_record_is_reset_and_starts() {
        let mut garbage = GuardRecord::from_words([0xDEAD_BEEF, 7, 9, 1]);
        assert_eq!(on_boot(&mut garbage), BootDecision::Start { attempts: 1 });
        assert!(garbage.is_valid());
        let mut torn = fresh();
        torn.attempts = 2; // written but not re-sealed: reset interrupted the update
        assert_eq!(on_boot(&mut torn), BootDecision::Start { attempts: 1 });
    }

    #[test]
    fn repeated_unclean_boots_end_in_suppression_not_in_a_loop() {
        let mut r = fresh();
        for n in 1..=MAX_UNCLEAN_STARTS {
            assert_eq!(on_boot(&mut r), BootDecision::Start { attempts: n });
        }
        // The Workload took the chip down every time: the 4th boot leaves it alone.
        assert_eq!(
            on_boot(&mut r),
            BootDecision::Suppressed {
                attempts: MAX_UNCLEAN_STARTS
            }
        );
        assert_eq!(
            on_boot(&mut r),
            BootDecision::Suppressed {
                attempts: MAX_UNCLEAN_STARTS
            },
            "and stays suppressed"
        );
    }

    #[test]
    fn a_deliberate_reboot_is_not_a_crash_and_gives_fresh_attempts() {
        let mut r = fresh();
        for _ in 0..MAX_UNCLEAN_STARTS {
            on_boot(&mut r);
        }
        assert!(matches!(on_boot(&mut r), BootDecision::Suppressed { .. }));
        mark_clean_reboot(&mut r);
        assert_eq!(on_boot(&mut r), BootDecision::Start { attempts: 1 });
        // The flag is consumed: an unclean boot afterwards counts again.
        assert_eq!(on_boot(&mut r), BootDecision::Start { attempts: 2 });
    }

    #[test]
    fn a_healthy_run_clears_the_counter() {
        let mut r = fresh();
        on_boot(&mut r);
        on_boot(&mut r);
        mark_proven_healthy(&mut r);
        assert_eq!(on_boot(&mut r), BootDecision::Start { attempts: 1 });
    }

    #[test]
    fn proven_health_fires_once_after_a_continuous_healthy_period() {
        let mut p = ProvenHealth::new();
        assert!(!p.observe(true, 1_000));
        assert!(!p.observe(true, 20_000));
        assert!(p.observe(true, 31_000));
        assert!(!p.observe(true, 60_000), "only once");
        // An unhealthy blip restarts the clock.
        let mut p = ProvenHealth::new();
        assert!(!p.observe(true, 0));
        assert!(!p.observe(false, 20_000));
        assert!(!p.observe(true, 25_000));
        assert!(!p.observe(true, 50_000));
        assert!(p.observe(true, 55_000));
    }

    #[test]
    fn the_record_round_trips_through_words() {
        let mut r = fresh();
        on_boot(&mut r);
        assert_eq!(GuardRecord::from_words(r.to_words()), r);
        assert!(GuardRecord::from_words(r.to_words()).is_valid());
    }
}
