//! HTTP Content-Range syntax. Session resume decisions belong to the
//! service receiving the content.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total: u64,
}

impl ContentRange {
    /// Whether the provided range has no bytes (start exceeds end).
    pub fn is_empty(self) -> bool {
        self.start > self.end
    }

    pub fn len(self) -> Option<u64> {
        self.end.checked_sub(self.start)?.checked_add(1)
    }
}

/// Parse `Content-Range: bytes <start>-<end>/<total>`.
pub fn parse_content_range(value: &str) -> Option<ContentRange> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim().parse().ok()?;
    let total = total.trim().parse().ok()?;
    (start <= end && end < total).then_some(ContentRange { start, end, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_bounds_and_overflow() {
        assert_eq!(
            parse_content_range("bytes 1024-2047/4096").unwrap().len(),
            Some(1024)
        );
        assert_eq!(parse_content_range("bytes 2048-1024/4096"), None);
        assert_eq!(parse_content_range("bytes 0-4096/4096"), None);
        assert_eq!(
            ContentRange {
                start: 0,
                end: u64::MAX,
                total: u64::MAX
            }
            .len(),
            None
        );
    }
}
