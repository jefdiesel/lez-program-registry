//! CIDs as text. The chain stores the binary CIDv1 (`ContentRef.cid`); people and Logos
//! Storage use multibase text (`zDv…`, base58btc). Accepted on input: `z…` (base58btc),
//! `b…` (base32 lower, RFC 4648, no padding), `f…` or `0x…` (hex). Printed as `z…`.

/// Multibase text to binary CID bytes.
pub fn parse(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    let bytes = if let Some(rest) = s.strip_prefix("0x") {
        hex::decode(rest).map_err(|e| format!("bad hex CID: {e}"))?
    } else if let Some(rest) = s.strip_prefix('z') {
        bs58::decode(rest).into_vec().map_err(|e| format!("bad base58btc CID: {e}"))?
    } else if let Some(rest) = s.strip_prefix('b') {
        base32_decode(rest).ok_or_else(|| "bad base32 CID".to_owned())?
    } else if let Some(rest) = s.strip_prefix('f') {
        hex::decode(rest).map_err(|e| format!("bad hex CID: {e}"))?
    } else {
        return Err(format!("'{s}' is not a multibase CID (expected z…, b…, f… or 0x…)"));
    };
    if bytes.is_empty() || bytes.len() > lezreg_types::limits::CID {
        return Err(format!("a CID must be 1..={} bytes, this one is {}", lezreg_types::limits::CID, bytes.len()));
    }
    if bytes[0] != 0x01 {
        return Err("only CIDv1 is accepted (first byte must be 0x01)".to_owned());
    }
    Ok(bytes)
}

/// Binary CID bytes to multibase base58btc text.
#[must_use]
pub fn to_string(cid: &[u8]) -> String {
    format!("z{}", bs58::encode(cid).into_string())
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c.to_ascii_lowercase())? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_forms() {
        let mut cid = vec![0x01, 0xCD, 0x01, 0x12, 0x20];
        cid.extend_from_slice(&[7; 32]);
        let z = to_string(&cid);
        assert!(z.starts_with('z'));
        assert_eq!(parse(&z).unwrap(), cid);
        assert_eq!(parse(&format!("0x{}", hex::encode(&cid))).unwrap(), cid);
        assert_eq!(parse(&format!("f{}", hex::encode(&cid))).unwrap(), cid);
        // "bafy…"-style base32 of a short CIDv1: 0x01 0x55 0x00 = "afkqa".
        assert_eq!(parse("bafkqa").unwrap(), vec![0x01, 0x55, 0x00]);
    }

    #[test]
    fn refuses_bad_input() {
        assert!(parse("Qm123").is_err());
        assert!(parse("z").is_err());
        assert!(parse(&format!("0x{}", "00".repeat(5))).is_err(), "CIDv0/garbage first byte");
        assert!(parse(&format!("0x01{}", "00".repeat(64))).is_err(), "too long");
    }
}
