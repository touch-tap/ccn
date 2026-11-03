//! Signs a release's SHA256SUMS, so `ccn upgrade` can tell the release came from TouchTap and not
//! just from wherever the files were downloaded. Run on the release Mac by scripts/release.sh; the
//! private key never leaves that Mac's keychain (see RELEASING.md, "The update signing key").
//!
//!   cargo run --release --example release-sign -- keygen
//!       prints a new private key (hex) and its public key (hex) on two lines
//!   CCN_RELEASE_SIGNING_KEY=<hex> cargo run --release --example release-sign -- sign SHA256SUMS
//!       writes SHA256SUMS.sig: the ed25519 signature of the file, as 128 hex characters
//!   CCN_RELEASE_SIGNING_KEY=<hex> cargo run --release --example release-sign -- public
//!       prints the public key, to compare with RELEASE_KEYS in src/update.rs

use ed25519_dalek::{Signer, SigningKey};
use std::io::Read;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> [u8; 32] {
    let text = text.trim();
    assert!(text.len() == 64, "the key must be 64 hex characters");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("the key is not hex");
    }
    out
}

fn key_from_env() -> SigningKey {
    let text = std::env::var("CCN_RELEASE_SIGNING_KEY")
        .expect("set CCN_RELEASE_SIGNING_KEY to the private key (hex)");
    SigningKey::from_bytes(&unhex32(&text))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            // 32 bytes from the OS's random source are a whole ed25519 private key
            let mut seed = [0u8; 32];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut seed))
                .expect("reading /dev/urandom");
            let key = SigningKey::from_bytes(&seed);
            println!("{}", hex(&seed));
            println!("{}", hex(key.verifying_key().as_bytes()));
        }
        Some("sign") => {
            let path = args.get(1).expect("sign <file>");
            let data = std::fs::read(path).expect("reading the file to sign");
            let key = key_from_env();
            let signature = key.sign(&data);
            // Check before writing: a key pasted wrong must not produce a signature nobody accepts
            key.verifying_key()
                .verify_strict(&data, &signature)
                .expect("the signature doesn't verify");
            std::fs::write(format!("{path}.sig"), format!("{}\n", hex(&signature.to_bytes())))
                .expect("writing the signature");
        }
        Some("public") => println!("{}", hex(key_from_env().verifying_key().as_bytes())),
        _ => {
            eprintln!("usage: release-sign keygen | sign <file> | public");
            std::process::exit(2);
        }
    }
}
