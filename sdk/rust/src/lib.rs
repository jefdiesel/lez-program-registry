//! LEZ Program Registry SDK (DESIGN §7.1).
//!
//! **Reads** ([`Registry`]) talk only to a LEZ sequencer the caller chooses, with two JSON-RPC
//! methods (`getAccount`, `getAccountView`), and need no LEZ crates:
//! - [`Registry::resolve`]: the deployer entry, the live program header and fresh/stale, in
//!   **one** `getAccount`;
//! - [`Registry::third_party`], [`Registry::third_party_entry`]: community entries;
//! - [`Registry::list`]: the global list, newest first, page by page; `Link`s are write-once
//!   and cached (on disk with [`LinkCache::at`]);
//! - [`Registry::by_name`], [`Registry::search_name`], [`Registry::by_author`] (proven),
//!   [`Registry::search_label`] (unproven), [`Registry::search_tag`].
//!
//! **Writes** (feature `tx`, [`tx::Writer`]) build every registry instruction, simulate it
//! against the chain with the on-chain registry bytecode, send it with a gas limit of 1.5× the
//! simulated cycles, and read the change back.
//!
//! ```no_run
//! use lezreg_sdk::{HttpRpc, Registry};
//! use lezreg_types::AccountId;
//! let reg = AccountId::from_base58("…registry id…").unwrap();
//! let r = Registry::new(HttpRpc::new("http://127.0.0.1:3062"), reg);
//! let program = AccountId::from_base58("…program id…").unwrap();
//! let resolved = r.resolve(&program)?;
//! if let Some(e) = &resolved.deployer {
//!     println!("{} {} ({:?})", e.name, e.version, resolved.freshness());
//! }
//! # Ok::<(), lezreg_sdk::Error>(())
//! ```

pub mod cid;
pub mod error;
pub mod idl;
pub mod jcs;
pub mod json;
pub mod read;
pub mod rpc;
pub mod storage;
#[cfg(feature = "tx")]
pub mod tx;

use std::path::PathBuf;

pub use error::{Error, Result};
pub use idl::{FetchConfig, IdlStatus, Peer, Unavailable};
pub use lezreg_types as types;
pub use read::{
    Found, Freshness, Header, Kind, LOADER, LinkCache, Listed, Located, Page, Registry, Resolved,
};
pub use rpc::{AccountStates, HttpRpc, MockRpc, Rpc};
pub use storage::StorageClient;

/// A network: a sequencer and the registry's program id on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    pub rpc: Option<String>,
    pub registry: Option<lezreg_types::AccountId>,
}

/// The built-in networks. `local` is a standalone sequencer on this machine (its registry id
/// is whatever `chain/deploy.sh` deployed). `testnet` is a placeholder until LEZ testnet 0.4
/// launches: no RPC and no registry id yet, so commands say so instead of guessing.
#[must_use]
pub fn builtin_network(name: &str) -> Option<Network> {
    match name {
        "local" => Some(Network {
            name: "local".into(),
            rpc: Some("http://127.0.0.1:3062".into()),
            registry: None,
        }),
        "testnet" => Some(Network {
            name: "testnet".into(),
            rpc: None,
            registry: None,
        }),
        _ => None,
    }
}

/// Where a `LinkCache` for `registry` on `rpc` lives: `$XDG_CACHE_HOME/lez-registry/` (or
/// `~/.cache/lez-registry/`), one file per registry and sequencer.
#[must_use]
pub fn default_cache_path(registry: &lezreg_types::AccountId, rpc: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let host = hex::encode(&lezreg_types::pda::sha256(rpc.as_bytes())[..6]);
    Some(
        base.join("lez-registry")
            .join(format!("links-{}-{host}.bin", registry.to_base58())),
    )
}
