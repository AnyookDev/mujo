use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

#[derive(Clone, Debug)]
pub struct Totp {
    pub(super) secret: Vec<u8>,
    pub(super) period: u64,
    pub(super) digits: usize,
}

impl Totp {
    pub fn from_secret(secret_b32: &str) -> Option<Self> {
        let secret = base32::decode(
            base32::Alphabet::RFC4648 { padding: false },
            &secret_b32.to_uppercase().replace(" ", ""),
        )?;
        // base32::decode("") succeeds with an empty vec, but an empty
        // secret is not a valid TOTP key.
        if secret.is_empty() {
            return None;
        }
        Some(Self {
            secret,
            period: 30,
            digits: 6,
        })
    }

    pub fn generate(&self) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let counter = timestamp / self.period;
        self.generate_for_counter(counter)
    }

    pub(super) fn generate_for_counter(&self, counter: u64) -> String {
        let mut mac =
            HmacSha1::new_from_slice(&self.secret).expect("HMAC can take key of any size");
        mac.update(&counter.to_be_bytes());
        let result = mac.finalize();
        let hash = result.into_bytes();
        let offset = (hash[hash.len() - 1] & 0x0f) as usize;
        let code = ((hash[offset] & 0x7f) as u32) << 24
            | (hash[offset + 1] as u32) << 16
            | (hash[offset + 2] as u32) << 8
            | hash[offset + 3] as u32;
        let code = code % 10_u32.pow(self.digits as u32);
        format!("{:0width$}", code, width = self.digits)
    }

    pub fn time_remaining(&self) -> u64 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.period - (timestamp % self.period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc_secret() -> &'static str {
        "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
    }

    #[test]
    fn test_totp_from_secret_valid() {
        let totp = Totp::from_secret(rfc_secret());
        assert!(totp.is_some(), "valid base32 secret must parse");
    }

    #[test]
    fn test_totp_from_secret_invalid() {
        assert!(Totp::from_secret("!!!").is_none());
        assert!(Totp::from_secret("").is_none());
    }

    #[test]
    fn test_totp_whitespace_only_secret_rejected() {
        assert!(Totp::from_secret("   ").is_none());
    }

    #[test]
    fn test_totp_valid_secret_accepted() {
        assert!(Totp::from_secret("JBSWY3DPEHPK3PXP").is_some());
    }

    #[test]
    fn test_totp_from_secret_normalizes() {
        let canonical = Totp::from_secret(rfc_secret()).unwrap();
        let messy = Totp::from_secret("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap();
        assert_eq!(
            canonical.generate_for_counter(1),
            messy.generate_for_counter(1)
        );
    }

    #[test]
    fn test_totp_rfc4226_vectors() {
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let expected = [
            (0, "755224"),
            (1, "287082"),
            (2, "359152"),
            (3, "969429"),
            (4, "338314"),
            (5, "254676"),
        ];
        for (counter, code) in expected {
            assert_eq!(
                totp.generate_for_counter(counter),
                code,
                "HOTP value at counter {} mismatched",
                counter
            );
        }
    }

    #[test]
    fn test_totp_generate_shape() {
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let code = totp.generate();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "code: {}", code);
    }

    #[test]
    fn test_totp_time_remaining_in_period() {
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let remaining = totp.time_remaining();
        assert!((1..=30).contains(&remaining), "remaining: {}", remaining);
    }
}
