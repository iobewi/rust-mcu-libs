/// A standard state, shared by every module and product.
///
/// States are **flags, not a state machine**: any number of them can be raised at once, by one
/// source or by several. Nothing here orders them, ranks their severity or forbids a combination;
/// that is the business of the producer (what it raises) and of the consumer (what it shows).
///
/// The variants fall in two kinds:
///
/// - **Observations.** A producer raises a state because it *observed* it. This holds for the
///   negative states too: [`State::Disconnected`] means "this source established that the link is
///   down", not "this source says nothing about the link". The absence of a flag is the absence of
///   information, never its opposite: a registry that holds no `Connected` does not tell that
///   anything is disconnected.
/// - **Transitions** (`-ing`). Something is under way and will end with another state.
///
/// The discriminants are explicit and never change. New variants are only ever **appended**, and
/// at most 64 states exist ([`Flags`](crate::Flags) is a `u64`). The enum is `non_exhaustive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u8)]
pub enum State {
    // Lifecycle
    /// The system is starting up: the whole product, before any module is ready. Compare
    /// [`State::Initializing`], which is one module getting itself ready.
    Booting = 0,
    /// One module is getting itself ready (resources, configuration read, links being prepared).
    /// Compare [`State::Booting`], which is the product as a whole.
    Initializing = 1,
    /// The source has finished its initialisation and can do its job. It says nothing about
    /// whether work is under way (see [`State::Running`], [`State::Busy`]) or whether others can
    /// use the resource right now (see [`State::Available`]).
    Ready = 2,
    /// The source is in operation: its main activity is under way.
    Running = 3,
    /// The source is winding down its activity and will end up [`State::Stopped`].
    Stopping = 4,
    /// The source has stopped its activity and was not asked to go on (it is not paused, not
    /// restarting).
    Stopped = 5,
    /// The source is restarting: it stops and starts again, as a reboot does. Compare
    /// [`State::Resetting`], which is about returning to defaults.
    Restarting = 6,
    /// The product or the source is powering down for good.
    ShuttingDown = 7,

    // Operations
    /// The source observed that it has nothing to do right now. Not the same as the absence of
    /// [`State::Busy`], which only means that nobody reported work.
    Idle = 8,
    /// The source is working on a request and may be slow to take another.
    Busy = 9,
    /// Data is being read in (from storage, from a link) by the source.
    Loading = 10,
    /// Data is being written out (to storage, to a link) by the source.
    Saving = 11,
    /// Software, firmware or a data set is being replaced by a newer one. Plain reading and
    /// writing of data are [`State::Loading`] and [`State::Saving`].
    Updating = 12,
    /// The source is bringing its data in line with a peer (time, files, remote state).
    Synchronizing = 13,

    // Connectivity
    /// A link is being established for the first time.
    Connecting = 14,
    /// The source observed that its link is up and usable.
    Connected = 15,
    /// A link is being closed on purpose.
    Disconnecting = 16,
    /// The source observed that its link is down: lost, closed or never established. Compare the
    /// absence of [`State::Connected`], which only means that no source reported the link up.
    Disconnected = 17,
    /// A link that was lost is being established again.
    Reconnecting = 18,

    // Availability
    /// The resource the source provides can be used by others **right now**. A source can be
    /// [`State::Ready`] and not available (it is busy, or shared and taken).
    Available = 19,
    /// The resource the source provides cannot be used by others right now, for any reason.
    Unavailable = 20,
    /// The activity was suspended on purpose and can be resumed.
    Paused = 21,
    /// A paused activity is starting again.
    Resuming = 22,

    // Configuration
    /// The source observed that it lacks the configuration it needs to work.
    Unconfigured = 23,
    /// A configuration is being applied or entered.
    Configuring = 24,
    /// The source observed that it has a usable configuration.
    Configured = 25,

    // Health
    /// A check or a self-test succeeded and nothing is wrong. This is an observation: the absence
    /// of [`State::Error`] only means that nobody reported a fault, which a source that was never
    /// checked does too.
    Healthy = 26,
    /// The function is reduced (a fallback is used, a part is lost) but it still works.
    Degraded = 27,
    /// An anomaly was noted. The function is intact.
    Warning = 28,
    /// A fault occurred. The function is impaired or lost, and it may recover on its own.
    Error = 29,
    /// The source cannot operate without intervention.
    Critical = 30,

    // Maintenance
    /// The source is in a maintenance mode (service, calibration, administration).
    Maintenance = 31,
    /// The source is getting back to a working condition after a fault.
    Recovering = 32,
    /// The source is returning to its defaults, discarding its settings or data. Compare
    /// [`State::Restarting`], which keeps them.
    Resetting = 33,
    /// A test is under way.
    Testing = 34,
}

impl State {
    /// The number of states. At most 64.
    pub const COUNT: usize = 35;

    /// Every state, in discriminant order: `ALL[s as usize] == s`.
    pub const ALL: [State; State::COUNT] = [
        State::Booting,
        State::Initializing,
        State::Ready,
        State::Running,
        State::Stopping,
        State::Stopped,
        State::Restarting,
        State::ShuttingDown,
        State::Idle,
        State::Busy,
        State::Loading,
        State::Saving,
        State::Updating,
        State::Synchronizing,
        State::Connecting,
        State::Connected,
        State::Disconnecting,
        State::Disconnected,
        State::Reconnecting,
        State::Available,
        State::Unavailable,
        State::Paused,
        State::Resuming,
        State::Unconfigured,
        State::Configuring,
        State::Configured,
        State::Healthy,
        State::Degraded,
        State::Warning,
        State::Error,
        State::Critical,
        State::Maintenance,
        State::Recovering,
        State::Resetting,
        State::Testing,
    ];

    /// The state whose discriminant is `index`, if there is one.
    pub const fn from_index(index: u8) -> Option<State> {
        if (index as usize) < State::COUNT {
            Some(State::ALL[index as usize])
        } else {
            None
        }
    }
}

const _: () = assert!(State::COUNT <= 64, "Flags holds at most 64 states");
