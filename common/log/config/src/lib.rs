#![no_std]
//! Version-one restricted YAML mapping for the dedicated `log` ConfigSpace.
use core::fmt::Write;
use heapless::String;
use iobewi_config_space::{ConfigBackend, ConfigSpace, SpaceError};
use iobewi_log::{LevelFilter, LogPolicy, PolicyError, apply_policy};

/// Claim this budget in the product's deterministic boot claim sequence.
pub const POLICY_BYTES_MAX: usize = 1024;
pub const SPACE_NAME: &str = "log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    TooLarge,
    Syntax,
    Level,
    Policy(PolicyError),
}

#[derive(Debug)]
pub enum ConfigError<E> {
    Space(SpaceError<E>),
    Schema(SchemaError),
}

fn parse_level(value: &str) -> Result<LevelFilter, SchemaError> {
    match value {
        "off" => Ok(LevelFilter::Off),
        "error" => Ok(LevelFilter::Error),
        "warn" => Ok(LevelFilter::Warn),
        "info" => Ok(LevelFilter::Info),
        "debug" => Ok(LevelFilter::Debug),
        "trace" => Ok(LevelFilter::Trace),
        _ => Err(SchemaError::Level),
    }
}
fn level_name(level: LevelFilter) -> &'static str {
    match level {
        LevelFilter::Off => "off",
        LevelFilter::Error => "error",
        LevelFilter::Warn => "warn",
        LevelFilter::Info => "info",
        LevelFilter::Debug => "debug",
        LevelFilter::Trace => "trace",
    }
}

/// Plain YAML target keys: Rust namespaces are accepted, YAML special forms are not.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-./".contains(&b))
        && key
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !key.ends_with(':')
}

/// Parse the documented restricted YAML, rejecting unknown/duplicate keys and
/// unsupported YAML constructs instead of partially applying a configuration.
pub fn decode(data: &[u8]) -> Result<LogPolicy, SchemaError> {
    if data.len() > POLICY_BYTES_MAX {
        return Err(SchemaError::TooLarge);
    }
    let text = core::str::from_utf8(data).map_err(|_| SchemaError::Syntax)?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    if lines.next() != Some("log:") {
        return Err(SchemaError::Syntax);
    }
    let default = lines
        .next()
        .and_then(|line| line.strip_prefix("  default_level: "))
        .ok_or(SchemaError::Syntax)?;
    let mut policy = LogPolicy::new(parse_level(default)?);
    match lines.next() {
        None => return Ok(policy),
        Some("  targets:") => {}
        _ => return Err(SchemaError::Syntax),
    }
    for line in lines {
        let (target, level) = line
            .strip_prefix("    ")
            .and_then(|line| line.rsplit_once(": "))
            .ok_or(SchemaError::Syntax)?;
        if !valid_key(target) {
            return Err(SchemaError::Syntax);
        }
        policy
            .add_target(target, parse_level(level)?)
            .map_err(SchemaError::Policy)?;
    }
    Ok(policy)
}

pub fn encode(policy: &LogPolicy) -> Result<String<POLICY_BYTES_MAX>, SchemaError> {
    let mut yaml = String::new();
    write!(
        yaml,
        "log:\n  default_level: {}\n  targets:\n",
        level_name(policy.default_level)
    )
    .map_err(|_| SchemaError::TooLarge)?;
    for rule in policy.targets() {
        if !valid_key(&rule.target) {
            return Err(SchemaError::Syntax);
        }
        writeln!(yaml, "    {}: {}", rule.target, level_name(rule.level))
            .map_err(|_| SchemaError::TooLarge)?;
    }
    Ok(yaml)
}

/// Owns the unique ConfigSpace handle. Serialize all updates through `&mut self`.
/// External writers must explicitly call reload; no polling/task is created.
pub struct LogConfig<B: ConfigBackend> {
    space: ConfigSpace<B>,
}
impl<B: ConfigBackend> LogConfig<B> {
    pub fn new(space: ConfigSpace<B>) -> Self {
        Self { space }
    }

    /// Missing value leaves the early-boot fallback intact. Invalid data/backend
    /// failures return errors and leave the current policy intact as well.
    pub async fn reload(&mut self) -> Result<Option<u64>, ConfigError<B::Error>> {
        let snapshot = self.space.load().await.map_err(ConfigError::Space)?;
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let policy = decode(&snapshot.data).map_err(ConfigError::Schema)?;
        apply_policy(policy);
        Ok(Some(snapshot.generation))
    }

    /// Validate, persist the complete YAML, then apply. Failed commits never
    /// change runtime filtering; a power cut after commit is recovered by reload.
    pub async fn replace(&mut self, policy: LogPolicy) -> Result<u64, ConfigError<B::Error>> {
        let yaml = encode(&policy).map_err(ConfigError::Schema)?;
        let generation = self
            .space
            .commit(yaml.as_bytes())
            .await
            .map_err(ConfigError::Space)?;
        apply_policy(policy);
        Ok(generation)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use iobewi_config_space::{Budget, ConfigManager, Snapshot};
    use std::{
        cell::RefCell,
        future::Future,
        rc::Rc,
        vec::Vec,
    };

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        let mut future = core::pin::pin!(future);
        loop {
            if let core::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
        }
    }
    #[derive(Default)]
    struct State {
        data: Option<Snapshot>,
        fail: bool,
        generation: u64,
    }
    #[derive(Clone, Default)]
    struct Memory(Rc<RefCell<State>>);
    impl ConfigBackend for Memory {
        type Error = ();
        fn capacity_units(&self) -> usize {
            POLICY_BYTES_MAX
        }
        fn reservation_units(&self, _: &str, budget: Budget) -> Option<usize> {
            Some(budget.max_bytes())
        }
        async fn load(&self, _: &str) -> Result<Option<Snapshot>, ()> {
            if self.0.borrow().fail {
                return Err(());
            }
            Ok(self.0.borrow().data.clone())
        }
        async fn commit(&self, _: &str, bytes: &[u8]) -> Result<u64, ()> {
            let mut state = self.0.borrow_mut();
            if state.fail {
                return Err(());
            }
            state.generation += 1;
            state.data = Some(Snapshot {
                generation: state.generation,
                data: Vec::from(bytes),
            });
            Ok(state.generation)
        }
        async fn clear(&self, _: &str) -> Result<u64, ()> {
            unimplemented!()
        }
    }

    #[test]
    fn yaml_contract_and_bounds() {
        let yaml = b"log:\n  default_level: off\n  targets:\n    app: debug\n    app::usb: trace\n";
        let policy = decode(yaml).unwrap();
        assert_eq!(policy.level_for("app::usb::task"), LevelFilter::Trace);
        assert_eq!(decode(encode(&policy).unwrap().as_bytes()).unwrap(), policy);
        for level in [
            LevelFilter::Off,
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            let policy = LogPolicy::new(level);
            assert_eq!(decode(encode(&policy).unwrap().as_bytes()).unwrap(), policy);
        }
        for invalid in [
            "",
            "log:\n  default_level: bogus",
            "log:\n  default_level: info\n  unknown: true",
            "log:\n  default_level: info\n  targets:\n    app: debug\n    app: trace",
            "log:\n  default_level: info\n  targets:\n    'app': trace",
            "log:\n  default_level: info\n  targets:\n    - app: trace",
        ] {
            assert!(decode(invalid.as_bytes()).is_err(), "{invalid}");
        }
        assert_eq!(
            decode(&[b'x'; POLICY_BYTES_MAX + 1]),
            Err(SchemaError::TooLarge)
        );
        assert!(decode(&[0xff]).is_err());
        let mut maximal = LogPolicy::new(LevelFilter::Trace);
        for i in 0..iobewi_log::TARGET_RULES_MAX {
            let key = std::format!("t{i}{}", "x".repeat(iobewi_log::TARGET_MAX - 2));
            maximal.add_target(&key, LevelFilter::Trace).unwrap();
        }
        assert_eq!(
            decode(encode(&maximal).unwrap().as_bytes()).unwrap(),
            maximal
        );
    }

    #[test]
    fn persist_reload_missing_invalid_and_failed_commit() {
        iobewi_log::install(|_| {}, "app");
        let backend = Memory::default();
        let mut manager = ConfigManager::new(backend.clone());
        let space = manager
            .claim(SPACE_NAME, Budget::new(POLICY_BYTES_MAX))
            .unwrap();
        let mut config = LogConfig::new(space);
        assert_eq!(block_on(config.reload()).unwrap(), None);
        assert!(log::log_enabled!(target: "app", log::Level::Info));
        assert_eq!(
            block_on(config.replace(LogPolicy::new(LevelFilter::Off))).unwrap(),
            1
        );
        assert!(!log::log_enabled!(log::Level::Error));
        apply_policy(LogPolicy::new(LevelFilter::Trace));
        assert_eq!(block_on(config.reload()).unwrap(), Some(1));
        assert!(!log::log_enabled!(log::Level::Error));
        backend.0.borrow_mut().fail = true;
        assert!(block_on(config.replace(LogPolicy::new(LevelFilter::Debug))).is_err());
        assert!(!log::log_enabled!(log::Level::Error));
        assert!(block_on(config.reload()).is_err());
        backend.0.borrow_mut().fail = false;
        backend.0.borrow_mut().data.as_mut().unwrap().data = Vec::from(b"invalid");
        assert!(block_on(config.reload()).is_err());
        assert!(!log::log_enabled!(log::Level::Error));
    }
}
