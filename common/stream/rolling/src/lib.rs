#![no_std]
#![doc = include_str!("../README.md")]

/// Availability of a requested byte range in the retained window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadStatus {
    Ready,
    Pending,
    Expired,
}

/// Consumer origin and highest successfully consumed absolute byte position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub base: u64,
    pub read_end: u64,
}

/// A single-producer, single-session rolling byte window.
/// Synchronization, scheduling and application policy belong to the caller.
pub struct RollingStream<const N: usize> {
    data: [u8; N],
    written: u64,
    retain: u64,
    max_lead: u64,
    session: Option<Session>,
}

impl<const N: usize> RollingStream<N> {
    /// `retain` is the history kept at a new session's origin.
    pub const fn new(retain: u64, max_lead: u64) -> Self {
        assert!(N > 0);
        assert!(retain <= max_lead && max_lead <= N as u64);
        Self {
            data: [0; N],
            written: 0,
            retain,
            max_lead,
            session: None,
        }
    }

    pub fn written(&self) -> u64 {
        self.written
    }
    pub fn session(&self) -> Option<Session> {
        self.session
    }

    /// Rebase offset zero near the live edge, including after a disconnected interval.
    pub fn begin_session(&mut self) -> Session {
        let base = self.written - self.written.min(self.retain);
        let session = Session {
            base,
            read_end: base,
        };
        self.session = Some(session);
        session
    }

    pub fn end_session(&mut self) -> Option<Session> {
        self.session.take()
    }

    /// Maximum safe next producer chunk. No session means the live edge keeps moving.
    pub fn writable(&self) -> usize {
        let allowance = self.session.map_or(N as u64, |s| {
            self.max_lead
                .saturating_sub(self.written.saturating_sub(s.read_end))
        });
        allowance.min(N as u64).min(u64::MAX - self.written) as usize
    }

    /// Returns the accepted prefix length; callers must retain any remaining input.
    pub fn push(&mut self, input: &[u8]) -> usize {
        let n = input.len().min(self.writable());
        let pos = (self.written % N as u64) as usize;
        let first = n.min(N - pos);
        self.data[pos..pos + first].copy_from_slice(&input[..first]);
        self.data[..n - first].copy_from_slice(&input[first..n]);
        self.written += n as u64;
        n
    }

    /// Relative session read. Unavailable reads fill output with zeroes and do not
    /// advance backpressure. Far-ahead probe policy is deliberately left to callers.
    pub fn read(&mut self, offset: u64, out: &mut [u8]) -> ReadStatus {
        out.fill(0);
        let Some(mut session) = self.session else {
            return ReadStatus::Pending;
        };
        let Some(start) = session.base.checked_add(offset) else {
            return ReadStatus::Pending;
        };
        let Some(end) = start.checked_add(out.len() as u64) else {
            return ReadStatus::Pending;
        };
        if end > self.written {
            return ReadStatus::Pending;
        }
        if start < self.written.saturating_sub(N as u64) {
            return ReadStatus::Expired;
        }
        let pos = (start % N as u64) as usize;
        let first = out.len().min(N - pos);
        out[..first].copy_from_slice(&self.data[pos..pos + first]);
        let remaining = out.len() - first;
        out[first..].copy_from_slice(&self.data[..remaining]);
        session.read_end = session.read_end.max(end);
        self.session = Some(session);
        ReadStatus::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_wrap_and_rebase() {
        let mut s = RollingStream::<12>::new(8, 10);
        assert_eq!(s.push(b"abcdefghijkl"), 12);
        assert_eq!(s.begin_session().base, 4);
        assert_eq!(s.push(b"mnop"), 2);
        assert_eq!(s.writable(), 0);
        let mut out = [0; 4];
        assert_eq!(s.read(0, &mut out), ReadStatus::Ready);
        assert_eq!(&out, b"efgh");
        assert_eq!(s.push(b"opqr"), 4);
        assert_eq!(s.read(8, &mut out), ReadStatus::Ready);
        assert_eq!(&out, b"mnop");
        let consumed = s.session().unwrap().read_end;
        assert_eq!(s.read(4, &mut out), ReadStatus::Ready);
        assert_eq!(s.session().unwrap().read_end, consumed);
        s.end_session();
        assert_eq!(s.push(b"stuvwxyz0123"), 12);
        assert_eq!(s.begin_session().base, 22);
        assert_eq!(s.read(0, &mut out), ReadStatus::Ready);
        assert_eq!(&out, b"wxyz");
    }

    #[test]
    fn unavailable_reads_do_not_consume() {
        let mut s = RollingStream::<8>::new(4, 6);
        let mut out = [1; 4];
        assert_eq!(s.read(0, &mut out), ReadStatus::Pending);
        assert_eq!(out, [0; 4]);
        s.begin_session();
        s.push(b"abcdef");
        assert_eq!(s.read(4, &mut out), ReadStatus::Pending);
        assert_eq!(s.read(u64::MAX, &mut out), ReadStatus::Pending);
        assert_eq!(s.session().unwrap().read_end, 0);
        assert_eq!(s.read(0, &mut out), ReadStatus::Ready);
        s.push(b"ghij");
        assert_eq!(s.read(0, &mut out), ReadStatus::Expired);
        assert_eq!(out, [0; 4]);
    }

    #[test]
    fn continuous_position_exceeds_old_file_extent() {
        let mut s = RollingStream::<96_000>::new(64_000, 80_000);
        let chunk = [7; 2048];
        for _ in 0..2048 {
            assert_eq!(s.push(&chunk), chunk.len());
        }
        assert!(s.written() > 2 * 1024 * 1024);
        s.begin_session();
        let mut out = [0; 512];
        assert_eq!(s.read(0, &mut out), ReadStatus::Ready);
        assert_eq!(out, [7; 512]);
    }

    #[test]
    #[should_panic]
    fn invalid_capacity_is_rejected() {
        let _ = RollingStream::<8>::new(7, 6);
    }
}
