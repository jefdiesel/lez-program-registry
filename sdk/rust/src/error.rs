//! Typed SDK errors. Every failure a caller may want to branch on has its own variant; the
//! `Display` text is what the CLI prints.

use lezreg_types::AccountId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The sequencer could not be reached, or answered with something that is not JSON-RPC.
    Transport { url: String, message: String },
    /// The sequencer answered with a JSON-RPC error.
    Rpc { method: String, message: String },
    /// An account's registry state does not decode as the record expected there.
    BadRecord { at: AccountId, what: &'static str },
    /// A list walk found a missing `Link` (a private or unverifiable entry, DESIGN §2.4).
    MissingLink(AccountId),
    /// A list walk came back to an entry it had already visited.
    Cycle(AccountId),
    /// No registry entry where one was asked for.
    NoEntry(AccountId),
    /// No program header at the account.
    NotDeployed(AccountId),
    /// A field fails the registry's own limits; sending would be a charged revert.
    Invalid(String),
    /// Configuration is missing or wrong (network, registry id, wallet).
    Config(String),
    /// The wallet holds no signing key for an account that must sign.
    NoKey(AccountId),
    /// The signer is not the entry's current author.
    NotAuthor { entry: AccountId, author: AccountId, signer: AccountId },
    /// The entry is frozen and can no longer change.
    Frozen(AccountId),
    /// Simulation against current chain state refused the transaction (nothing was sent).
    Refused(String),
    /// The payer cannot cover the fee reserve.
    Funds(String),
    /// The sequencer did not accept the transaction.
    Rejected(String),
    /// The sequencer accepted the transaction, but it could not be confirmed in time. `hash` is
    /// the transaction; `detail` says whether its nonce was used (it may have applied, or another
    /// transaction from the same signer took the slot: check before retrying) or not (nothing
    /// applied: safe to retry).
    NotIncluded { hash: String, detail: String },
    /// Included but reverted: its fee was charged and nothing else applied.
    Reverted { hash: String, block: u64, fee: u128 },
    /// Applied, but reading the chain back did not show the expected change.
    ReadBack(String),
    /// A wallet operation failed.
    Wallet(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport { url, message } => write!(f, "cannot reach the sequencer at {url}: {message}"),
            Self::Rpc { method, message } => write!(f, "sequencer error on {method}: {message}"),
            Self::BadRecord { at, what } => write!(f, "{at} does not hold a registry {what}"),
            Self::MissingLink(e) => write!(f, "entry {e} has no list link (private or unverifiable)"),
            Self::Cycle(e) => write!(f, "list walk looped at {e}"),
            Self::NoEntry(e) => write!(f, "no registry entry at {e}"),
            Self::NotDeployed(p) => write!(f, "no program is deployed at {p}"),
            Self::Invalid(m) => write!(f, "invalid input: {m}"),
            Self::Config(m) => write!(f, "{m}"),
            Self::NoKey(a) => write!(f, "this wallet holds no signing key for {a}"),
            Self::NotAuthor { entry, author, signer } => {
                write!(f, "only the entry's author may change it: entry {entry} is held by {author}, not {signer}")
            }
            Self::Frozen(e) => write!(f, "entry {e} is frozen"),
            Self::Refused(m) => write!(f, "simulation refused the transaction (nothing was sent): {m}"),
            Self::Funds(m) => write!(f, "{m}"),
            Self::Rejected(m) => write!(f, "the sequencer did not accept the transaction: {m}"),
            Self::NotIncluded { hash, detail } => write!(f, "transaction {hash} was not confirmed: {detail}"),
            Self::Reverted { hash, block, fee } => {
                write!(f, "{hash} was included in block {block} but REVERTED: fee {fee} charged, nothing else applied")
            }
            Self::ReadBack(m) => write!(f, "applied, but the read-back check failed: {m}"),
            Self::Wallet(m) => write!(f, "wallet: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The transaction hash, for errors about a transaction that was sent.
    #[must_use]
    pub fn tx_hash(&self) -> Option<&str> {
        match self {
            Self::NotIncluded { hash, .. } | Self::Reverted { hash, .. } => Some(hash),
            _ => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
