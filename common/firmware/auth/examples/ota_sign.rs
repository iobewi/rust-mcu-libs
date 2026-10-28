//! Host tool: generate a signing key and sign firmware images (IAM1 envelopes).
//!
//! The private key is a 32-byte scalar written as hex to a file you choose,
//! outside the repository. Nothing here is linked into firmware.
//!
//! ```text
//! ota_sign keygen PRIVATE_KEY_FILE            prints the public key (hex) and its key id
//! ota_sign pubkey PRIVATE_KEY_FILE            same, for an existing key
//! ota_sign sign PRIVATE_KEY_FILE IMAGE.bin PRODUCT MCU VARIANT SECURITY_VERSION
//!                                              prints the envelope as unpadded base64url
//! ```
//! `MCU` is one of `esp32c3`, `esp32s3`, `rp2350`, `imxrt1062`.

use iobewi_firmware_auth::manifest::{
    Algorithm, Envelope, ImageFormat, KeyId, Manifest, Mcu, Product,
};
use iobewi_firmware_auth::verify::host::{p256::ecdsa::SigningKey, public_key, sign};
use iobewi_firmware_auth::{MAX_ENVELOPE, MAX_ENVELOPE_TEXT, encode_b64url};
use iobewi_firmware_image::Digest;
use sha2::{Digest as _, Sha256};
use std::io::Read;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load(path: &str) -> SigningKey {
    let text = std::fs::read_to_string(path).expect("read private key file");
    let bytes: Vec<u8> = (0..text.trim().len() / 2)
        .map(|i| u8::from_str_radix(&text.trim()[i * 2..i * 2 + 2], 16).expect("hex private key"))
        .collect();
    SigningKey::from_slice(&bytes).expect("valid P-256 scalar")
}

fn show(key: &SigningKey) {
    let public = public_key(key);
    println!("public_key_hex={}", hex(&public));
    println!("key_id={}", hex(&KeyId::of(&public).0));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", path] => {
            let mut seed = [0u8; 32];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut seed))
                .expect("read /dev/urandom");
            let key = SigningKey::from_slice(&seed).expect("random scalar in range");
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .expect("create private key file (must not exist)");
                file.write_all(hex(&seed).as_bytes()).expect("write key");
            }
            show(&key);
        }
        ["pubkey", path] => show(&load(path)),
        ["sign", path, image, product, mcu, variant, version] => {
            let key = load(path);
            let image = std::fs::read(image).expect("read image");
            let mcu = match *mcu {
                "esp32c3" => Mcu::Esp32C3,
                "esp32s3" => Mcu::Esp32S3,
                "rp2350" => Mcu::Rp2350,
                "imxrt1062" => Mcu::Imxrt1062,
                other => panic!("unknown MCU {other}"),
            };
            let manifest = Manifest {
                algorithm: Algorithm::EcdsaP256Sha256,
                image_format: ImageFormat::EspApp,
                mcu,
                key_id: KeyId::of(&public_key(&key)),
                product: Product::new(product).expect("product tag: 1-16 printable ASCII"),
                hw_variant: variant.parse().expect("variant u16"),
                security_version: version.parse().expect("security version u32"),
                size: u32::try_from(image.len()).expect("image < 4 GiB"),
                digest: Digest(Sha256::digest(&image).into()),
            };
            let mut envelope = [0u8; MAX_ENVELOPE];
            let n = Envelope::encode(&manifest, &sign(&key, &manifest), &mut envelope).unwrap();
            let mut text = [0u8; MAX_ENVELOPE_TEXT];
            let t = encode_b64url(&envelope[..n], &mut text).unwrap();
            println!("{}", std::str::from_utf8(&text[..t]).unwrap());
        }
        _ => {
            eprintln!("usage: see the header of examples/ota_sign.rs");
            std::process::exit(2);
        }
    }
}
