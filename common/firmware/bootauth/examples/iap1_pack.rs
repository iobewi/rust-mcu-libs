//! Host tool: RSA-PSS 3072 keys, IAM1 envelopes and IAP1 proof sectors for VerifiedBoot.
//!
//!   iap1_pack keygen PRIVATE_KEY.pem            writes the key (mode 0600), prints the pin
//!   iap1_pack pin PRIVATE_KEY.pem               prints the pin (SHA-256 of the key encoding)
//!   iap1_pack sign PRIVATE_KEY.pem IMAGE.bin PRODUCT MCU VARIANT SECURITY_VERSION OUT_PREFIX
//!                                                writes OUT_PREFIX.env (IAM1 envelope, base64url
//!                                                text for `X-Image-Manifest`) and
//!                                                OUT_PREFIX.iap1 (the 4096-byte proof sector)
//!   iap1_pack inspect SECTOR.iap1               decodes a proof sector (no key needed)
//!
//! MCU is `esp32c3` or `esp32s3`. The private key never leaves the machine that runs `sign`.

use std::{env, fs, process::exit};

use iobewi_firmware_auth::{
    Algorithm, ImageFormat, KeyId, Manifest, Mcu, Product, encode_b64url, manifest::MANIFEST_LEN,
};
use iobewi_firmware_bootauth::{host::HostKey, iap1};
use sha2::{Digest as _, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn die(message: &str) -> ! {
    eprintln!("{message}");
    exit(2)
}

fn load(path: &str) -> HostKey {
    let pem = fs::read_to_string(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    HostKey::from_pem(&pem).unwrap_or_else(|| die("not an RSA-3072 PKCS#8 PEM key"))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", path] => {
            let key = HostKey::generate();
            #[cfg(unix)]
            {
                use std::io::Write as _;
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .unwrap_or_else(|e| die(&format!("{path}: {e}")));
                file.write_all(key.to_pem().as_bytes()).unwrap();
            }
            #[cfg(not(unix))]
            fs::write(path, key.to_pem()).unwrap();
            println!("key_id={}", hex(&KeyId::of(&key.public_struct).0));
            println!("pin={}", hex(&key.fingerprint()));
        }
        ["pin", path] => {
            let key = load(path);
            println!("key_id={}", hex(&KeyId::of(&key.public_struct).0));
            println!("pin={}", hex(&key.fingerprint()));
        }
        ["sign", path, image, product, mcu, variant, security, out] => {
            let key = load(path);
            let image = fs::read(image).unwrap_or_else(|e| die(&format!("{image}: {e}")));
            let manifest = Manifest {
                algorithm: Algorithm::RsaPss3072Sha256,
                image_format: ImageFormat::EspApp,
                mcu: match *mcu {
                    "esp32c3" => Mcu::Esp32C3,
                    "esp32s3" => Mcu::Esp32S3,
                    _ => die("MCU must be esp32c3 or esp32s3"),
                },
                key_id: KeyId::of(&key.public_struct),
                product: Product::new(product).unwrap_or_else(|| die("bad product tag")),
                hw_variant: variant.parse().unwrap_or_else(|_| die("bad variant")),
                security_version: security
                    .parse()
                    .unwrap_or_else(|_| die("bad security version")),
                size: u32::try_from(image.len()).unwrap_or_else(|_| die("image too large")),
                digest: iobewi_firmware_image::Digest(Sha256::digest(&image).into()),
            };
            let envelope = key.envelope(&manifest);
            let mut text = vec![0u8; iobewi_firmware_auth::MAX_ENVELOPE_TEXT];
            let n = encode_b64url(&envelope, &mut text).unwrap();
            fs::write(format!("{out}.env"), &text[..n]).unwrap();
            let mut scratch = [0u8; iap1::MAX_PAYLOAD];
            let write = iap1::Write::new(
                Algorithm::RsaPss3072Sha256,
                &envelope,
                &key.public_struct,
                &mut scratch,
            )
            .unwrap();
            let mut sector = vec![0xFFu8; iap1::SECTOR];
            sector[..iap1::OFF_COMMIT].copy_from_slice(&write.head);
            sector[iap1::OFF_COMMIT..iap1::HEADER_LEN].copy_from_slice(&iap1::COMMIT.to_le_bytes());
            sector[iap1::HEADER_LEN..iap1::HEADER_LEN + write.payload.len()]
                .copy_from_slice(write.payload);
            fs::write(format!("{out}.iap1"), &sector).unwrap();
            println!("size={} digest={}", image.len(), hex(&manifest.digest.0));
        }
        ["inspect", path] => {
            let sector = fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
            if sector.len() != iap1::SECTOR {
                die("a proof sector is exactly 4096 bytes");
            }
            let head: [u8; iap1::HEADER_LEN] = sector[..iap1::HEADER_LEN].try_into().unwrap();
            println!("header: {:?}", iap1::classify(&head));
            if let iap1::Header::Committed { algorithm, .. } = iap1::classify(&head) {
                let n = iap1::envelope_len(algorithm);
                let base = iap1::HEADER_LEN;
                println!(
                    "{:?}",
                    iobewi_firmware_auth::Manifest::decode(&sector[base..base + MANIFEST_LEN])
                );
                println!(
                    "key pin: {}",
                    hex(&Sha256::digest(
                        &sector[base + n..base + iap1::payload_len(algorithm)]
                    ))
                );
            }
        }
        _ => die(
            "usage: iap1_pack keygen KEY | pin KEY | sign KEY IMAGE PRODUCT MCU VARIANT SECVER OUT | inspect SECTOR",
        ),
    }
}
