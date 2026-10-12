//! Opt-in, versioned persistent record codecs. Not separate update engines.

pub mod otm1;

pub mod otm2;
pub mod otm2_store;

#[cfg(test)]
mod tests;
