//! Host-test helpers: an in-memory duplex connection and a trivial executor.
extern crate std;

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::convert::Infallible;
use core::future::Future;
use core::task::{Context, Poll, Waker};
use embedded_io_async::{ErrorType, Read, Write};

pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[derive(Default)]
pub struct State {
    /// Bytes the peer "sent" (what `read` returns; empty = end of stream).
    pub input: VecDeque<u8>,
    /// Bytes this side wrote.
    pub output: Vec<u8>,
    pub closed: bool,
}

/// Two-directional in-memory stream whose state stays inspectable.
#[derive(Clone, Default)]
pub struct Duplex {
    pub state: Rc<RefCell<State>>,
}

impl ErrorType for Duplex {
    type Error = Infallible;
}

impl Read for Duplex {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Infallible> {
        let mut st = self.state.borrow_mut();
        let n = out.len().min(st.input.len());
        for slot in out.iter_mut().take(n) {
            *slot = st.input.pop_front().unwrap();
        }
        Ok(n)
    }
}

impl Write for Duplex {
    async fn write(&mut self, data: &[u8]) -> Result<usize, Infallible> {
        self.state.borrow_mut().output.extend_from_slice(data);
        Ok(data.len())
    }

    async fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}
