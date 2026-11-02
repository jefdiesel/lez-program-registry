//! Events (DESIGN §2.5), for an optional self-hosted indexer. No client depends on them.
//! Selector: `sha256("lez_program_registry::<Name>")[..8]`; data is borsh.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::AccountId;

pub const REGISTERED: &str = "lez_program_registry::Registered";
pub const UPDATED: &str = "lez_program_registry::Updated";
pub const AUTHOR_TRANSFERRED: &str = "lez_program_registry::AuthorTransferred";
pub const FROZEN: &str = "lez_program_registry::Frozen";

/// Defines an event struct and its `FIELDS` (name, Rust type) in declaration order, so the IDL's
/// event layouts are derived from the same tokens as the borsh layout and cannot drift from it.
macro_rules! event {
    ($(#[$m:meta])* $name:ident { $($f:ident : $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
        pub struct $name { $(pub $f: $t),* }
        impl $name {
            /// `(field, Rust type)` in borsh order.
            pub const FIELDS: &'static [(&'static str, &'static str)] = &[$((stringify!($f), stringify!($t))),*];
        }
    };
}

event!(Registered {
    entry: AccountId,
    program: AccountId,
    kind: u8,
    author: AccountId,
    name: String,
    seq: u64,
    image_id: [u8; 32],
    idl_sha256: [u8; 32],
    born_with_deploy: bool,
});

event!(Updated {
    entry: AccountId,
    revision: u32,
    version: String,
    image_id: [u8; 32],
    idl_sha256: [u8; 32],
    build_hash: [u8; 32],
});

event!(AuthorTransferred { entry: AccountId, from: AccountId, to: AccountId });

event!(Frozen { entry: AccountId });

/// Every event: `(short name, full selector name, FIELDS)`.
pub const ALL: [(&str, &str, &[(&str, &str)]); 4] = [
    ("Registered", REGISTERED, Registered::FIELDS),
    ("Updated", UPDATED, Updated::FIELDS),
    ("AuthorTransferred", AUTHOR_TRANSFERRED, AuthorTransferred::FIELDS),
    ("Frozen", FROZEN, Frozen::FIELDS),
];

/// `sha256(name)[..8]`.
#[must_use]
pub fn selector(name: &str) -> [u8; 8] {
    crate::pda::sha256(name.as_bytes())[..8].try_into().expect("8 bytes")
}
