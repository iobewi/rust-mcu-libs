//! Bounded request-body streaming. Services supply a buffer and a chunk
//! destination; the HTTP layer owns the read loop and exact byte count.

use picoserve::io::Read;

#[allow(async_fn_in_trait)]
pub trait ChunkSink {
    type Error;

    async fn write_chunk(&self, chunk: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug)]
pub enum StreamError<ReadError, WriteError> {
    Read(ReadError),
    UnexpectedEof,
    Write(WriteError),
    EmptyBuffer,
}

/// Pass exactly `length` bytes to `sink`, without buffering the whole body.
/// A reader failure remains distinct from an incomplete body or sink failure
/// so the route can preserve its own HTTP error policy.
pub async fn stream_exact<R: Read, S: ChunkSink>(
    reader: &mut R,
    length: usize,
    buffer: &mut [u8],
    sink: &S,
) -> Result<(), StreamError<R::Error, S::Error>> {
    if buffer.is_empty() {
        return Err(StreamError::EmptyBuffer);
    }

    let mut remaining = length;
    while remaining > 0 {
        let to_read = remaining.min(buffer.len());
        let n = reader.read(&mut buffer[..to_read]).await
            .map_err(StreamError::Read)?;
        if n == 0 {
            return Err(StreamError::UnexpectedEof);
        }
        sink.write_chunk(&buffer[..n]).await.map_err(StreamError::Write)?;
        remaining -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    struct Recorder {
        bytes: RefCell<[u8; 6]>,
        written: Cell<usize>,
        fail_after: usize,
    }

    impl ChunkSink for Recorder {
        type Error = ();

        async fn write_chunk(&self, chunk: &[u8]) -> Result<(), Self::Error> {
            let start = self.written.get();
            if start >= self.fail_after {
                return Err(());
            }
            self.bytes.borrow_mut()[start..start + chunk.len()].copy_from_slice(chunk);
            self.written.set(start + chunk.len());
            Ok(())
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = core::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("synchronous test future unexpectedly pending"),
        }
    }

    #[test]
    fn reads_exact_length_without_consuming_next_body_and_reports_failures() {
        let sink = Recorder { bytes: RefCell::new([0; 6]), written: Cell::new(0), fail_after: usize::MAX };
        let mut body: &[u8] = b"abcdefextra";
        let mut buffer = [0u8; 2];
        assert!(ready(stream_exact(&mut body, 6, &mut buffer, &sink)).is_ok());
        assert_eq!(*sink.bytes.borrow(), *b"abcdef");
        assert_eq!(body, b"extra");

        let mut short: &[u8] = b"ab";
        let short_sink = Recorder { bytes: RefCell::new([0; 6]), written: Cell::new(0), fail_after: usize::MAX };
        assert!(matches!(ready(stream_exact(&mut short, 3, &mut buffer, &short_sink)), Err(StreamError::UnexpectedEof)));

        let rejecting = Recorder { bytes: RefCell::new([0; 6]), written: Cell::new(0), fail_after: 0 };
        let mut body: &[u8] = b"abcdef";
        assert!(matches!(ready(stream_exact(&mut body, 6, &mut buffer, &rejecting)), Err(StreamError::Write(()))));
        assert_eq!(body, b"cdef");
    }
}
