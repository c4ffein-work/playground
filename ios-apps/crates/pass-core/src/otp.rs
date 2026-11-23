//! Time-based one-time passwords (RFC 6238) from the `otpauth://totp/...` lines
//! that `pass-otp` keeps in an entry.

use ring::hmac;

use crate::{OtpCode, PassError};

fn invalid(why: &str) -> PassError {
    PassError::InvalidOtp {
        message: format!("not a usable otpauth:// line: {why}"),
    }
}

/// RFC 4648 base32, padding and spaces optional, either case.
fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| !matches!(c, b'=' | b' ' | b'-')) {
        let value = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The code of an `otpauth://totp/` URI at `unix_time`.
pub(crate) fn totp(uri: &str, unix_time: u64) -> Result<OtpCode, PassError> {
    let rest = uri
        .trim()
        .strip_prefix("otpauth://")
        .ok_or_else(|| invalid("it does not start with otpauth://"))?;
    let (kind, rest) = rest.split_once('/').ok_or_else(|| invalid("no type"))?;
    if !kind.eq_ignore_ascii_case("totp") {
        return Err(invalid("only time-based codes (totp) are supported"));
    }
    let query = rest.split_once('?').map(|(_, query)| query).unwrap_or("");
    let (mut secret, mut digits, mut period, mut algorithm) = (None, 6u32, 30u64, hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY);
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        let value = percent_decode(value);
        match key.to_ascii_lowercase().as_str() {
            "secret" => secret = Some(base32_decode(&value).ok_or_else(|| invalid("the secret is not base32"))?),
            "digits" => digits = value.parse().map_err(|_| invalid("digits is not a number"))?,
            "period" => period = value.parse().map_err(|_| invalid("period is not a number"))?,
            "algorithm" => {
                algorithm = match value.to_ascii_uppercase().as_str() {
                    "SHA1" => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
                    "SHA256" => hmac::HMAC_SHA256,
                    "SHA512" => hmac::HMAC_SHA512,
                    _ => return Err(invalid("unknown algorithm")),
                }
            }
            _ => {}
        }
    }
    let secret = secret.filter(|s| !s.is_empty()).ok_or_else(|| invalid("no secret"))?;
    if !(6..=8).contains(&digits) || period == 0 {
        return Err(invalid("digits is 6 to 8, and the period is not zero"));
    }

    let counter = unix_time / period;
    let mac = hmac::sign(&hmac::Key::new(algorithm, &secret), &counter.to_be_bytes());
    let mac = mac.as_ref();
    let offset = usize::from(mac[mac.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([mac[offset], mac[offset + 1], mac[offset + 2], mac[offset + 3]]) & 0x7fff_ffff;
    Ok(OtpCode {
        code: format!("{:0width$}", truncated % 10u32.pow(digits), width = digits as usize),
        period: period as u32,
        remaining: (period - unix_time % period) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test vectors of RFC 6238, appendix B. The secrets are the ASCII strings
    /// "12345678901234567890" (and its 32 and 64 byte extensions), in base32.
    #[test]
    fn rfc_6238_vectors() {
        let sha1 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let sha256 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA====";
        let sha512 =
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNA=";
        for (time, algorithm, secret, code) in [
            (59, "SHA1", sha1, "94287082"),
            (59, "SHA256", sha256, "46119246"),
            (59, "SHA512", sha512, "90693936"),
            (1111111109, "SHA1", sha1, "07081804"),
            (1234567890, "SHA256", sha256, "91819424"),
            (20000000000, "SHA512", sha512, "47863826"),
        ] {
            let uri = format!("otpauth://totp/x?secret={secret}&digits=8&algorithm={algorithm}");
            assert_eq!(totp(&uri, time).unwrap().code, code, "{algorithm} at {time}");
        }
    }

    #[test]
    fn defaults_and_refusals() {
        let code = totp(
            "otpauth://totp/Example:alice?secret=jbsw y3dp-ehpk3pxp&issuer=Example",
            61,
        )
        .unwrap();
        assert_eq!(code.code.len(), 6);
        assert_eq!((code.period, code.remaining), (30, 29));
        assert_eq!(
            code.code,
            totp("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP", 89).unwrap().code
        );
        assert_ne!(
            code.code,
            totp("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP", 90).unwrap().code
        );
        for bad in [
            "https://example.org",
            "otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1",
            "otpauth://totp/x",
            "otpauth://totp/x?secret=",
            "otpauth://totp/x?secret=not!base32",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=12",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5",
        ] {
            assert!(matches!(totp(bad, 0), Err(PassError::InvalidOtp { .. })), "{bad}");
        }
    }
}
