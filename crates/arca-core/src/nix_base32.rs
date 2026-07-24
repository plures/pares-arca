//! Nix-specific base32 encoding.
//!
//! Nix uses a non-standard base32 encoding with a custom alphabet and
//! reversed byte-to-character mapping. This is used in store path hashes
//! and narinfo fingerprints.
//!
//! Alphabet: `0123456789abcdfghijklmnpqrsvwxyz` (32 chars, no e/o/t/u)

const NIX_BASE32_CHARS: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Encode bytes to Nix base32 string.
///
/// Nix base32 encodes bytes in a specific reversed order. For a 32-byte
/// SHA-256 hash, this produces a 52-character string.
pub fn encode(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }

    // Nix base32 output length: ceil(input.len() * 8 / 5)
    let len = (input.len() * 8).div_ceil(5);
    let mut output = String::with_capacity(len);

    for n in (0..len).rev() {
        let b = n * 5;
        let byte_idx = b / 8;
        let bit_idx = b % 8;

        let mut c = (input[byte_idx] >> bit_idx) & 0x1f;
        if bit_idx > 3 && byte_idx + 1 < input.len() {
            c |= input[byte_idx + 1] << (8 - bit_idx);
            c &= 0x1f;
        }

        output.push(NIX_BASE32_CHARS[c as usize] as char);
    }

    output
}

/// Decode a Nix base32 string to bytes.
///
/// Returns None if the string contains invalid characters.
pub fn decode(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return Some(vec![]);
    }

    let len = input.len() * 5 / 8;
    let mut output = vec![0u8; len];

    for (n, ch) in input.chars().rev().enumerate() {
        let digit = NIX_BASE32_CHARS.iter().position(|&c| c == ch as u8)? as u8;
        let b = n * 5;
        let byte_idx = b / 8;
        let bit_idx = b % 8;

        if byte_idx < len {
            output[byte_idx] |= digit << bit_idx;
        }
        if bit_idx > 3 && byte_idx + 1 < len {
            output[byte_idx + 1] |= digit >> (8 - bit_idx);
        }
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_known_hash() {
        // SHA-256 of empty NAR: known value from Nix
        // nix-hash --type sha256 --base32 (of specific test data)
        let bytes = [0u8; 32];
        let encoded = encode(&bytes);
        assert_eq!(encoded.len(), 52);
        // All-zero hash should encode to all-zero chars
        assert_eq!(encoded, "0".repeat(52));
    }

    #[test]
    fn test_roundtrip() {
        let original: Vec<u8> = (0..32).collect();
        let encoded = encode(&original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_decode_invalid_char() {
        // 'e' is not in the Nix base32 alphabet
        assert!(decode("e").is_none());
        // 'o' is not in the Nix base32 alphabet
        assert!(decode("o").is_none());
    }

    #[test]
    fn test_empty() {
        assert_eq!(encode(&[]), "");
        assert_eq!(decode("").unwrap(), vec![0u8; 0]);
    }

    #[test]
    fn test_known_nix_store_hash_length() {
        // A SHA-256 hash (32 bytes) in Nix base32 is always 52 chars
        let hash = [0xab; 32];
        assert_eq!(encode(&hash).len(), 52);
    }

    #[test]
    fn test_known_nix_hash_conversion() {
        // Test vector from `nix hash to-base32 sha256-8EUDsWeTeZwJNrtjEsUNLMt9I9mjabPRBZG83u7xtPw=`
        // Expected nix-base32: 1z5ly7pdxg4i0p8v6sd3v4ipvjrc1p2i4qxv6q4rqyckcyqh6igh
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;
        let bytes = B64
            .decode("8EUDsWeTeZwJNrtjEsUNLMt9I9mjabPRBZG83u7xtPw=")
            .unwrap();
        assert_eq!(bytes.len(), 32);
        let encoded = encode(&bytes);
        assert_eq!(
            encoded,
            "1z5ly7pdxg4i0p8v6sd3v4ipvjrc1p2i4qxv6q4rqyckcyqh6igh"
        );
    }
}
