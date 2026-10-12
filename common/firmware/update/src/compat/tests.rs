#[cfg(test)]
mod tests {
    use crate::compat::otm2::{DecodeError, RECORD_LEN, Record, RuntimeApi, Side, SlotMeta, State, newer};
    use crate::compat::otm2_store::{Loaded, MetadataBackend, MetadataStore};

    #[test]
    fn codec_roundtrip_and_corruption() {
        let mut record = Record::empty(7);
        record.state = State::Valid;
        record.active = Some(Side::A);
        record.meta[0] = SlotMeta::new(
            "firmware-b",
            "1.2.3",
            [0xA5; 32],
            4096,
            RuntimeApi::new(1, 2),
        )
        .unwrap();
        let bytes = record.encode();
        assert_eq!(&bytes[..4], b"OTM2");
        assert_eq!(bytes.len(), RECORD_LEN);
        assert_eq!(Record::decode(&bytes), Ok(record));
        let mut damaged = bytes;
        damaged[33] ^= 0x01;
        assert_eq!(Record::decode(&damaged), Err(DecodeError::BadCrc));
        assert_eq!(Record::decode(&[0xFF; RECORD_LEN]), Err(DecodeError::Blank));
    }

    struct Memory {
        copies: [[u8; RECORD_LEN]; 2],
    }

    impl MetadataBackend for Memory {
        type Error = ();
        fn read(&mut self, copy: usize) -> Result<[u8; RECORD_LEN], ()> {
            Ok(self.copies[copy])
        }
        fn write(&mut self, copy: usize, bytes: &[u8; RECORD_LEN]) -> Result<(), ()> {
            self.copies[copy] = *bytes;
            Ok(())
        }
    }

    #[test]
    fn double_copy_keeps_last_valid_when_newest_is_torn() {
        let mut store = MetadataStore::new(Memory {
            copies: [[0xFF; RECORD_LEN]; 2],
        });
        assert_eq!(store.load(), Ok(Loaded::Blank));
        let old = Record::empty(u32::MAX);
        let new = Record::empty(0);
        store.commit(&old).unwrap();
        store.commit(&new).unwrap();
        assert!(matches!(store.load(), Ok(Loaded::Record {record, ..}) if record.sequence == 0));
        store.backend_mut().copies[1][40] ^= 0x20;
        assert!(
            matches!(store.load(), Ok(Loaded::Record {record, degraded: true, ..}) if record.sequence == u32::MAX)
        );
        assert!(newer(0, u32::MAX));
    }
}
