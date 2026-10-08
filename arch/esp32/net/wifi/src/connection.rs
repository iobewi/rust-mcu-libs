//! Evidence from the last completed association, never a substitute for live link state.
extern crate alloc;
use alloc::string::String;

#[derive(Default)]
pub(crate) struct Connection(Option<(String, String)>);

impl Connection {
    pub(crate) fn can_reuse(
        &self,
        associated: bool,
        link_up: bool,
        configured: bool,
        ssid: &str,
        password: &str,
    ) -> bool {
        associated
            && link_up
            && configured
            && self
                .0
                .as_ref()
                .is_some_and(|(s, p)| s == ssid && p == password)
    }

    pub(crate) fn invalidate(&mut self) {
        self.0 = None;
    }

    pub(crate) fn established(&mut self, ssid: &str, password: String) {
        self.0 = Some((String::from(ssid), password));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_credentials_reuse_only_a_live_configured_association() {
        let mut connection = Connection::default();
        assert!(!connection.can_reuse(true, true, true, "home", "pw"));
        connection.established("home", String::from("pw"));
        assert!(connection.can_reuse(true, true, true, "home", "pw"));
        // DHCP can briefly retain its lease after the radio disconnects.
        assert!(!connection.can_reuse(false, true, true, "home", "pw"));
        assert!(!connection.can_reuse(true, false, true, "home", "pw"));
        assert!(!connection.can_reuse(true, true, false, "home", "pw"));
        assert!(!connection.can_reuse(true, true, true, "other", "pw"));
        assert!(!connection.can_reuse(true, true, true, "home", "changed"));
    }

    #[test]
    fn failed_or_cancelled_reconfiguration_cannot_reuse_previous_credentials() {
        let mut connection = Connection::default();
        connection.established("home", String::from("pw"));
        connection.invalidate();
        assert!(!connection.can_reuse(true, true, true, "home", "pw"));
        connection.established("other", String::new());
        assert!(connection.can_reuse(true, true, true, "other", ""));
        assert!(!connection.can_reuse(true, true, true, "home", "pw"));
    }
}
