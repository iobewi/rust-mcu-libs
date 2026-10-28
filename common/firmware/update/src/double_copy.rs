//! Format-independent selection rules of a two-copy record store.
//!
//! Two physical copies are kept; a commit overwrites the **older** one (or the
//! invalid one), so the last good record survives an interrupted write. A
//! reader picks the newest valid copy by serial-number arithmetic on the
//! sequence. This module owns only those rules: it knows neither the record
//! layout nor the medium, so the OTM2 store and the OTA metadata frame store
//! share the decision without sharing a format.

/// Number of physical copies.
pub const COPIES: usize = 2;

/// Serial-number comparison: is `a` newer than `b`, wrap-safe.
pub const fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// What one physical copy holds, as classified by the record codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found<T> {
    /// Fully erased: never written, or erased and not yet programmed.
    Blank,
    /// Not blank and not a valid record: torn, corrupt or of an unknown format.
    Invalid,
    /// A valid record with its sequence number.
    Valid { value: T, sequence: u32 },
}

/// What the two copies say together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection<T> {
    /// Both copies erased: nothing was ever stored.
    Blank,
    /// The newest valid record, and which copy holds it. `degraded` is true
    /// when the other copy does not hold a valid record (blank or invalid).
    Record {
        value: T,
        copy: usize,
        degraded: bool,
    },
    /// No valid copy although at least one is not blank: never replaced by an
    /// invented state.
    Corrupted,
}

/// Picks the newest valid copy.
pub fn select<T>(copies: [Found<T>; COPIES]) -> Selection<T> {
    let [a, b] = copies;
    match (a, b) {
        (
            Found::Valid {
                value: va,
                sequence: sa,
            },
            Found::Valid {
                value: vb,
                sequence: sb,
            },
        ) => {
            if newer(sb, sa) {
                Selection::Record {
                    value: vb,
                    copy: 1,
                    degraded: false,
                }
            } else {
                Selection::Record {
                    value: va,
                    copy: 0,
                    degraded: false,
                }
            }
        }
        (Found::Valid { value, .. }, _) => Selection::Record {
            value,
            copy: 0,
            degraded: true,
        },
        (_, Found::Valid { value, .. }) => Selection::Record {
            value,
            copy: 1,
            degraded: true,
        },
        (Found::Blank, Found::Blank) => Selection::Blank,
        _ => Selection::Corrupted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(value: u8, sequence: u32) -> Found<u8> {
        Found::Valid { value, sequence }
    }

    #[test]
    fn serial_arithmetic_is_wrap_safe() {
        assert!(newer(2, 1));
        assert!(!newer(1, 2));
        assert!(!newer(5, 5));
        assert!(newer(0, u32::MAX));
        assert!(!newer(u32::MAX, 0));
    }

    #[test]
    fn two_blank_copies_are_blank() {
        assert_eq!(select::<u8>([Found::Blank; 2]), Selection::Blank);
    }

    #[test]
    fn the_newest_valid_copy_wins_across_the_wrap() {
        let pick = |a, b| match select([a, b]) {
            Selection::Record { value, copy, .. } => (value, copy),
            other => panic!("{other:?}"),
        };
        assert_eq!(pick(v(1, 7), v(2, 8)), (2, 1));
        assert_eq!(pick(v(1, 8), v(2, 7)), (1, 0));
        assert_eq!(pick(v(1, u32::MAX), v(2, 0)), (2, 1));
        assert_eq!(pick(v(1, 0), v(2, u32::MAX)), (1, 0));
    }

    #[test]
    fn one_valid_copy_is_degraded_whatever_the_other_is() {
        for other in [Found::Blank, Found::Invalid] {
            assert_eq!(
                select([v(9, 3), other]),
                Selection::Record {
                    value: 9,
                    copy: 0,
                    degraded: true
                }
            );
            assert_eq!(
                select([other, v(9, 3)]),
                Selection::Record {
                    value: 9,
                    copy: 1,
                    degraded: true
                }
            );
        }
    }

    #[test]
    fn no_valid_copy_but_not_blank_is_corrupted() {
        assert_eq!(
            select::<u8>([Found::Invalid, Found::Blank]),
            Selection::Corrupted
        );
        assert_eq!(
            select::<u8>([Found::Blank, Found::Invalid]),
            Selection::Corrupted
        );
        assert_eq!(
            select::<u8>([Found::Invalid, Found::Invalid]),
            Selection::Corrupted
        );
    }
}
