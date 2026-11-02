//! Addresses, by SPEL's `compute_pda` rule (`spel-framework-core/src/pda.rs`) under LEZ's
//! `AccountId::for_public_pda` (`lee_core/src/program/mod.rs`).
//!
//! - Each seed component is 32 bytes: strings UTF-8 zero-padded (at most 32 bytes), account ids
//!   and `[u8; 32]` raw. Integer `arg` seeds are not used (F3).
//! - One component is the seed itself; several are combined as `sha256(c1 ‖ c2 ‖ …)`.
//! - The address is `sha256("/LEE/v0.2/AccountId/PDA/" ‖ pad8 ‖ program ‖ seed)`.
//!
//! Host-side implementation (sha2). The guest derives the same addresses through
//! `spel04-shim`; tests pin the two together.

use sha2::{Digest as _, Sha256};

use crate::{AccountId, BuildConfig, seeds};

#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// A string seed component; `None` if longer than 32 bytes.
#[must_use]
pub fn try_seed_str(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() > 32 {
        return None;
    }
    let mut out = [0; 32];
    out[..b.len()].copy_from_slice(b);
    Some(out)
}

/// A string seed component. Panics if longer than 32 bytes.
#[must_use]
pub fn seed_str(s: &str) -> [u8; 32] {
    try_seed_str(s).unwrap_or_else(|| panic!("seed string '{s}' exceeds 32 bytes"))
}

/// SPEL `compute_pda` combination.
#[must_use]
pub fn combine(parts: &[[u8; 32]]) -> [u8; 32] {
    assert!(!parts.is_empty(), "a PDA needs at least one seed component");
    if parts.len() == 1 {
        return parts[0];
    }
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// `lee_core::account::AccountId::for_public_pda`.
#[must_use]
pub fn for_public_pda(program: &AccountId, seed: &[u8; 32]) -> AccountId {
    const PREFIX: &[u8; 32] = b"/LEE/v0.2/AccountId/PDA/\x00\x00\x00\x00\x00\x00\x00\x00";
    let mut h = Sha256::new();
    h.update(PREFIX);
    h.update(program.0);
    h.update(seed);
    AccountId(h.finalize().into())
}

/// `revision.to_le_bytes()` zero-padded to 32 bytes: the `revision_seed` argument every
/// instruction that writes a `Release` carries (same bytes as SPEL `ToSeed(u32)`).
#[must_use]
pub fn revision_seed(revision: u32) -> [u8; 32] {
    let mut s = [0; 32];
    s[..4].copy_from_slice(&revision.to_le_bytes());
    s
}

/// `sha256(borsh(BuildConfig))`, stored in every `Release`.
#[must_use]
pub fn build_hash(build: &BuildConfig) -> [u8; 32] {
    sha256(&borsh::to_vec(build).expect("borsh"))
}

/// The PDA seeds (combined) of every registry-owned address.
pub mod seed {
    use super::{combine, seed_str};
    use crate::{AccountId, seeds};

    #[must_use]
    pub fn third_party_entry(program: &AccountId, publisher: &AccountId) -> [u8; 32] {
        combine(&[seed_str(seeds::THIRD_PARTY), program.0, publisher.0])
    }
    #[must_use]
    pub fn release(entry: &AccountId, revision: u32) -> [u8; 32] {
        combine(&[seed_str(seeds::RELEASE), entry.0, super::revision_seed(revision)])
    }
    #[must_use]
    pub fn link(entry: &AccountId) -> [u8; 32] {
        combine(&[seed_str(seeds::LINK), entry.0])
    }
    #[must_use]
    pub fn global_head() -> [u8; 32] {
        combine(&[seed_str(seeds::HEAD)])
    }
    #[must_use]
    pub fn name_head(name: &str) -> [u8; 32] {
        combine(&[seed_str(seeds::NAME), seed_str(name)])
    }
    #[must_use]
    pub fn author_head(author: &AccountId) -> [u8; 32] {
        combine(&[seed_str(seeds::AUTHOR), author.0])
    }
    #[must_use]
    pub fn program_head(program: &AccountId) -> [u8; 32] {
        combine(&[seed_str(seeds::PROGRAM_HEAD), program.0])
    }
    #[must_use]
    pub fn author_node(new_author: &AccountId, entry: &AccountId) -> [u8; 32] {
        combine(&[seed_str(seeds::AUTHOR_NODE), new_author.0, entry.0])
    }
}

/// Registry-owned addresses under the registry program `reg`.
pub mod addr {
    use super::{for_public_pda, seed};
    use crate::AccountId;

    #[must_use]
    pub fn third_party_entry(reg: &AccountId, program: &AccountId, publisher: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::third_party_entry(program, publisher))
    }
    #[must_use]
    pub fn release(reg: &AccountId, entry: &AccountId, revision: u32) -> AccountId {
        for_public_pda(reg, &seed::release(entry, revision))
    }
    #[must_use]
    pub fn link(reg: &AccountId, entry: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::link(entry))
    }
    #[must_use]
    pub fn global_head(reg: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::global_head())
    }
    #[must_use]
    pub fn name_head(reg: &AccountId, name: &str) -> AccountId {
        for_public_pda(reg, &seed::name_head(name))
    }
    #[must_use]
    pub fn author_head(reg: &AccountId, author: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::author_head(author))
    }
    #[must_use]
    pub fn program_head(reg: &AccountId, program: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::program_head(program))
    }
    #[must_use]
    pub fn author_node(reg: &AccountId, new_author: &AccountId, entry: &AccountId) -> AccountId {
        for_public_pda(reg, &seed::author_node(new_author, entry))
    }
}

/// The seed domains, checked to fit a 32-byte component.
#[must_use]
pub fn all_domains() -> [&'static str; 8] {
    [
        seeds::THIRD_PARTY,
        seeds::RELEASE,
        seeds::LINK,
        seeds::HEAD,
        seeds::NAME,
        seeds::AUTHOR,
        seeds::PROGRAM_HEAD,
        seeds::AUTHOR_NODE,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_fit() {
        for d in all_domains() {
            assert!(d.len() <= 32, "{d}");
        }
    }

    #[test]
    fn spike_vectors() {
        // Spike (a), cross-checked against the released `spel` CLI's resolver (program a5…a5):
        // head = 7oS5b5cW… (650b321f…47cd), name_head("alice") under "toyreg/v1/name" = DYxSzNmx… (ba7bf18c…5a03).
        let reg = AccountId([0xa5; 32]);
        let head = for_public_pda(&reg, &combine(&[seed_str("toyreg/v1/head")]));
        assert_eq!(head.to_base58(), "7oS5b5cWo6iM46LfbvBdrAVgkM74XL6svxmEuHotKyPr");
        let name = for_public_pda(&reg, &combine(&[seed_str("toyreg/v1/name"), seed_str("alice")]));
        assert_eq!(name.to_base58(), "DYxSzNmxQy8KPZJahuVvnnSPQhijPTKyUCzJU4T6Zpok");
    }

    #[test]
    fn revision_seed_is_le_padded() {
        let s = revision_seed(0x0102_0304);
        assert_eq!(&s[..4], &[4, 3, 2, 1]);
        assert!(s[4..].iter().all(|b| *b == 0));
    }

    #[test]
    fn single_component_is_the_seed() {
        assert_eq!(seed::global_head(), seed_str(seeds::HEAD));
    }
}
