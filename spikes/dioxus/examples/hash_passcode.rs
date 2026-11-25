//! Print an argon2id PHC hash for ADMIN_PASSCODE_HASH.
//! Usage: cargo run --example hash_passcode --features server -- <passcode>

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString},
};

fn main() {
    let pass = std::env::args().nth(1).expect("usage: hash_passcode <passcode>");
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).unwrap();
    let salt = SaltString::encode_b64(&bytes).unwrap();
    let hash = Argon2::default().hash_password(pass.as_bytes(), &salt).unwrap();
    println!("{hash}");
}
