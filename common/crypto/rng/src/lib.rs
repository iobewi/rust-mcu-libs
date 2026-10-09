#![no_std]

/// Platform capability supplying random bytes.
///
/// The platform implementation owns the actual entropy source and any
/// hardware-specific seeding requirements. Callers only request bytes.
pub trait EntropySource {
    fn fill_random(&self, output: &mut [u8]);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;

    impl EntropySource for Fixed {
        fn fill_random(&self, output: &mut [u8]) {
            for (i, byte) in output.iter_mut().enumerate() {
                *byte = i as u8;
            }
        }
    }

    #[test]
    fn fills_the_requested_buffer() {
        let source = Fixed;
        let mut out = [0xff; 4];
        source.fill_random(&mut out);
        assert_eq!(out, [0, 1, 2, 3]);
    }
}
