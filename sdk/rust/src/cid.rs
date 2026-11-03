//! CIDs as text. The chain stores the binary CIDv1 (`ContentRef.cid`); people and Logos
//! Storage use multibase text (`zDv…`, base58btc). Accepted on input: `z…` (base58btc),
//! `b…` (base32 lower, RFC 4648, no padding), `f…` or `0x…` (hex). Printed as `z…`.

/// Multibase text to binary CID bytes.
pub fn parse(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    let bytes = if let Some(rest) = s.strip_prefix("0x") {
        hex::decode(rest).map_err(|e| format!("bad hex CID: {e}"))?
    } else if let Some(rest) = s.strip_prefix('z') {
        bs58::decode(rest)
            .into_vec()
            .map_err(|e| format!("bad base58btc CID: {e}"))?
    } else if let Some(rest) = s.strip_prefix('b') {
        if rest.bytes().any(|c| c.is_ascii_uppercase()) {
            return Err("bad base32 CID: multibase 'b' is lower case".to_owned());
        }
        base32_decode(rest).ok_or_else(|| "bad base32 CID".to_owned())?
    } else if let Some(rest) = s.strip_prefix('f') {
        hex::decode(rest).map_err(|e| format!("bad hex CID: {e}"))?
    } else {
        return Err(format!(
            "'{s}' is not a multibase CID (expected z…, b…, f… or 0x…)"
        ));
    };
    if bytes.is_empty() || bytes.len() > lezreg_types::limits::CID {
        return Err(format!(
            "a CID must be 1..={} bytes, this one is {}",
            lezreg_types::limits::CID,
            bytes.len()
        ));
    }
    if bytes[0] != 0x01 {
        return Err("only CIDv1 is accepted (first byte must be 0x01)".to_owned());
    }
    parts(&bytes)?;
    Ok(bytes)
}

/// Codec (`0x55` raw), and the multihash's code (`0x12` sha2-256) and digest.
pub struct Parts<'a> {
    pub codec: u64,
    pub hash_code: u64,
    pub digest: &'a [u8],
}

fn varint(b: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut v = 0u64;
    for shift in (0..63).step_by(7) {
        let byte = *b.get(*at).ok_or("CID ends inside a varint")?;
        *at += 1;
        v |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err("CID varint too long".to_owned())
}

/// Splits a binary CIDv1 (`0x01`, codec, multihash code, digest length, digest) and checks the
/// digest length matches what follows.
pub fn parts(cid: &[u8]) -> Result<Parts<'_>, String> {
    let mut at = 0;
    if varint(cid, &mut at)? != 1 {
        return Err("only CIDv1 is accepted".to_owned());
    }
    let codec = varint(cid, &mut at)?;
    let hash_code = varint(cid, &mut at)?;
    let len = usize::try_from(varint(cid, &mut at)?).map_err(|_| "bad digest length")?;
    let digest = &cid[at..];
    if digest.len() != len {
        return Err(format!(
            "not a CID: its multihash says a {len}-byte digest but {} bytes follow",
            digest.len()
        ));
    }
    if hash_code == 0x12 && len != 32 {
        return Err("a sha2-256 multihash must have a 32-byte digest".to_owned());
    }
    Ok(Parts {
        codec,
        hash_code,
        digest,
    })
}

/// For a raw-codec sha2-256 CID (`0x01 0x55 0x12 0x20 <sha256>`, which anyone can compute from
/// the bytes): the content's sha256 it names.
#[must_use]
pub fn raw_sha256(cid: &[u8]) -> Option<[u8; 32]> {
    let p = parts(cid).ok()?;
    (p.codec == 0x55 && p.hash_code == 0x12)
        .then(|| p.digest.try_into().ok())
        .flatten()
}

/// The raw-codec sha2-256 CID of `bytes`: the interim IDL CID until Storage upload (M3).
#[must_use]
pub fn raw_cid(bytes: &[u8]) -> Vec<u8> {
    let mut c = vec![0x01, 0x55, 0x12, 0x20];
    c.extend_from_slice(&lezreg_types::pda::sha256(bytes));
    c
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
        // "b…" base32 of an identity-hash CIDv1 with an empty digest: 0x01 0x55 0x00 0x00.
        assert_eq!(parse("bafkqaaa").unwrap(), vec![0x01, 0x55, 0x00, 0x00]);
        let raw = raw_cid(b"sample idl");
        assert_eq!(
            raw_sha256(&parse(&format!("0x{}", hex::encode(&raw))).unwrap()),
            Some(lezreg_types::pda::sha256(b"sample idl"))
        );
    }

    /// Review fix (outsider 12): a string that merely starts with "b" or "0x01" is not a CID.
    #[test]
    fn checks_the_multihash() {
        assert!(parse("bafyNOTACID").is_err(), "upper case in base32");
        assert!(
            parse("bafynotacid").is_err(),
            "digest length does not match"
        );
        assert!(parse("0x01551220").is_err(), "sha2-256 with no digest");
        assert!(
            parse(&format!("0x01551210{}", "00".repeat(16))).is_err(),
            "sha2-256 with a 16-byte digest"
        );
        assert!(parse(&format!("0x01551220{}", "00".repeat(32))).is_ok());
    }

    #[test]
    fn refuses_bad_input() {
        assert!(parse("Qm123").is_err());
        assert!(parse("z").is_err());
        assert!(
            parse(&format!("0x{}", "00".repeat(5))).is_err(),
            "CIDv0/garbage first byte"
        );
        assert!(
            parse(&format!("0x01{}", "00".repeat(64))).is_err(),
            "too long"
        );
    }
}
