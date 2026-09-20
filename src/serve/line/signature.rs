use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Computes the base64-encoded HMAC-SHA256 signature for a request body using the channel secret.
pub fn compute_signature(channel_secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(channel_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    BASE64.encode(result)
}

/// Verifies that the provided signature matches the HMAC-SHA256 of the body using constant-time comparison.
pub fn verify_signature(channel_secret: &str, body: &[u8], signature_header: &str) -> bool {
    if channel_secret.is_empty() || signature_header.is_empty() {
        return false;
    }
    let expected = compute_signature(channel_secret, body);
    constant_time_eq(expected.as_bytes(), signature_header.trim().as_bytes())
}

/// Constant-time comparison of two byte slices to mitigate timing side-channel attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_verification_valid() {
        let secret = "test_channel_secret_12345";
        let body = b"{\"destination\":\"U123456\",\"events\":[]}";
        let sig = compute_signature(secret, body);
        assert!(!sig.is_empty());
        assert!(verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_signature_verification_tampered_body() {
        let secret = "test_channel_secret_12345";
        let body = b"{\"destination\":\"U123456\",\"events\":[]}";
        let sig = compute_signature(secret, body);

        let tampered_body = b"{\"destination\":\"U999999\",\"events\":[]}";
        assert!(!verify_signature(secret, tampered_body, &sig));
    }

    #[test]
    fn test_signature_verification_wrong_secret() {
        let secret1 = "secret_one";
        let secret2 = "secret_two";
        let body = b"hello world";
        let sig = compute_signature(secret1, body);

        assert!(!verify_signature(secret2, body, &sig));
    }

    #[test]
    fn test_signature_verification_empty() {
        assert!(!verify_signature("", b"hello", "sig"));
        assert!(!verify_signature("secret", b"hello", ""));
    }
}
