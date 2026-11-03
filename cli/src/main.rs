//! `lezreg`: the LEZ Program Registry command line (DESIGN §7.2).
//!
//! Reads (`show`, `list`, `search`) talk to one LEZ sequencer with plain JSON-RPC and need no
//! wallet. Writes (`register`, `update`, `transfer`, `freeze`, `deploy`) sign with the LEZ
//! wallet, built in as `lezreg wallet`, whose keys live in `LEE_WALLET_HOME_DIR` (default
//! `~/.lee/wallet`). Every write is simulated first, sent with gas = 1.5 × simulated cycles,
//! and read back from the chain. No server of ours is ever on the path.

mod args;
mod config;
mod doctor;
mod out;
mod project;
mod publish;
mod render;
mod stats;
mod storage_cmd;
mod verify_cmd;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use lee_core::account::AccountId;
use lezreg_sdk::{
    Error as SdkError, HttpRpc, LinkCache, Registry, default_cache_path,
    tx::{SendOpts, Writer, lid, tid},
};
use lezreg_types::pda::addr;
use serde_json::json;
use wallet::{
    PASSWORD_ENV_VAR, WalletCore,
    config::{SequencerConnectionData, WalletConfigOverrides},
    utils::{fetch_config_path, fetch_persistent_storage_path, read_password},
};

use crate::{
    args::{EntryArgs, PatchArgs, Target, parse_account},
    config::Resolved as Net,
    out::Out,
};

#[derive(Parser)]
#[command(
    name = "lezreg",
    version,
    about = "LEZ Program Registry: register, find and check programs on the Logos Execution Zone",
    long_about = "LEZ Program Registry: register, find and check programs on the Logos Execution Zone (LEZ 0.4).\n\n\
        Reads (show, list, search) need no setup and talk only to the sequencer you choose. \
        Writes sign with the LEZ wallet, built in as `lezreg wallet`, whose keys live in \
        LEE_WALLET_HOME_DIR (default ~/.lee/wallet).\n\n\
        A deployer entry for program X lives at X's own account and can only be written by X's \
        header key, so `show X` tells a deployer entry from community (third-party) entries by \
        its address alone, and compares its claimed image id with the live program (fresh/stale).\n\n\
        Networks: `local` (a standalone sequencer on 127.0.0.1:3062; set its registry id with \
        `lezreg network set local --registry <id>`, which chain/deploy.sh does) and `testnet` \
        (LEZ testnet 0.4: not launched yet)."
)]
struct Cli {
    /// Network: `local` or `testnet` (or any name set with `lezreg network set`).
    #[arg(long, env = "LEZREG_NETWORK", default_value = "local", global = true)]
    network: String,
    /// Sequencer RPC URL (overrides the network's).
    #[arg(long, env = "LEZREG_RPC", global = true)]
    rpc: Option<String>,
    /// Registry program id (overrides the network's).
    #[arg(long, env = "LEZREG_REGISTRY", global = true)]
    registry: Option<String>,
    /// Print one JSON object on stdout instead of text.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

/// Options every write takes.
#[derive(clap::Args, Clone)]
struct WriteOpts {
    /// Account that pays the fee (the wallet must hold its key). Default: the first signer
    /// (for `deploy`: --author, since a new header account holds nothing).
    #[arg(long, env = "LEZREG_PAYER")]
    payer: Option<String>,
    /// Simulate only: report what would happen and the gas, send nothing.
    #[arg(long)]
    dry_run: bool,
    /// Send even when simulation refuses (to show the chain refusing it). The payer is
    /// charged the whole --gas-limit for the revert.
    #[arg(long)]
    force_send: bool,
    /// Gas limit for --force-send.
    #[arg(long, default_value_t = 400_000, requires = "force_send")]
    gas_limit: u64,
    /// Allow writing through a sequencer that is not on this machine (spends real funds).
    #[arg(long)]
    yes: bool,
}

/// Search options.
#[derive(clap::Args, Clone, Copy)]
struct SearchOpts {
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// How many entries the search may examine (newest first) before it stops; a search that
    /// stops early says so.
    #[arg(long, default_value_t = 1000)]
    scan: usize,
}

#[derive(Subcommand)]
enum Command {
    /// Show a program's registry entries: the deployer entry (fresh/stale against the live
    /// program) and community entries. TARGET is a program id or a name.
    Show {
        target: String,
        /// Show the third-party entry this publisher made for TARGET instead.
        #[arg(long)]
        publisher: Option<String>,
        /// Also show the write-once release snapshot of this revision.
        #[arg(long)]
        revision: Option<u32>,
    },
    /// List every entry, newest first, a page at a time, with each program's status
    /// (fresh, stale, or NO-PROGRAM when nothing is deployed at the claimed program).
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Start after this entry address (the `next` of the previous page).
        #[arg(long)]
        after: Option<String>,
    },
    /// Search entries.
    Search {
        #[command(subcommand)]
        by: SearchBy,
    },
    /// Register a program: a deployer entry (the program's header key signs), or with
    /// --third-party a community entry for someone else's program.
    Register {
        /// The program's id (its header account).
        #[arg(long)]
        program: String,
        /// The proven author: co-signs now and is the only account that may update later.
        /// Default: the program's own key (deployer) or the publisher (third party).
        #[arg(long)]
        author: Option<String>,
        /// Make a third-party (community) entry instead of a deployer entry.
        #[arg(long)]
        third_party: bool,
        /// With --third-party: the publishing account (signs; is the author).
        #[arg(long, requires = "third_party")]
        publisher: Option<String>,
        #[command(flatten)]
        entry: EntryArgs,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// Deploy a program binary (segments, then the loader header) and register it in the same
    /// transaction (DeployAndRegister), unless --no-register. Everything that could refuse is
    /// checked before the first (paid) segment write; --dry-run stops there and prints the
    /// estimate. The payer defaults to --author (a new header account holds nothing). The code
    /// is upgradeable by the header key unless --immutable.
    Deploy {
        /// The risc0 program binary (`*.bin`).
        elf: std::path::PathBuf,
        /// Header account to deploy at (the wallet must hold its key). Default: a new account
        /// in the wallet.
        #[arg(long)]
        program: Option<String>,
        /// Deploy immutable: the code can never be upgraded.
        #[arg(long)]
        immutable: bool,
        /// Plain loader deploy, no registry entry.
        #[arg(long)]
        no_register: bool,
        /// The proven author (co-signs). Default: the header key.
        #[arg(long)]
        author: Option<String>,
        #[command(flatten)]
        entry: EntryArgs,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// Update an entry (only its author can). Every update writes a release snapshot.
    Update {
        #[command(flatten)]
        target: Target,
        /// The signing author. Default: the entry's current author.
        #[arg(long)]
        author: Option<String>,
        #[command(flatten)]
        patch: PatchArgs,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// Hand an entry to a new author. The current and the new author both sign. This moves the
    /// entry only: the program's header (upgrade) key stays where it is, so the original
    /// deployer can still repoint the code (the entry then shows stale).
    Transfer {
        #[command(flatten)]
        target: Target,
        /// The new author.
        #[arg(long)]
        to: String,
        #[arg(long)]
        author: Option<String>,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// Freeze an entry for good: no more updates or transfers.
    Freeze {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        author: Option<String>,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// IDLs on Logos Storage: fetch and check an entry's IDL, or upload one.
    Idl {
        #[command(subcommand)]
        cmd: IdlCmd,
    },
    /// Keep an entry's IDL on your Storage node and announce it, so it stays resolvable
    /// when the author's node is offline.
    Pin {
        /// Program id or name.
        target: String,
        #[command(flatten)]
        storage: storage_cmd::StorageOpts,
    },
    /// Rebuild an entry's recorded source in Docker (network off for the compile) and compare
    /// image ids: rebuilt vs claimed vs the live program. Verdict Match, MatchStale, Mismatch or
    /// NotReproducible. Needs Docker with buildx.
    Verify {
        /// The program id.
        program: String,
        /// Verify this recorded release instead of the current revision.
        #[arg(long)]
        revision: Option<u32>,
        /// Work directory (kept afterwards). Default: a temporary one.
        #[arg(long)]
        work: Option<std::path::PathBuf>,
        /// If the network-off build fails, retry with the network on (reported as weaker).
        #[arg(long)]
        allow_network: bool,
        /// On Match, record a local "verified" mark (never published).
        #[arg(long)]
        mark: bool,
    },
    /// Local verification marks: list them, or forget one (by program id).
    Verified {
        #[arg(long)]
        forget: Option<String>,
    },
    /// The canonical reproducible build of a local source tree, printing its image id: what
    /// `verify` will rebuild. Writes the guest ELF and the deployable `.bin`. Without
    /// --binary, the build settings come from lezreg.toml. Needs Docker with buildx.
    Build {
        /// Source root (the docker root). Default: lezreg.toml's, else the current directory.
        dir: Option<std::path::PathBuf>,
        /// lezreg.toml to read when --binary is not given.
        #[arg(long, default_value = project::FILE)]
        config: std::path::PathBuf,
        #[command(flatten)]
        build: args::BuildArgs,
        #[arg(long)]
        work: Option<std::path::PathBuf>,
    },
    /// Write lezreg.toml for this project: name, version, IDL, repository and the canonical
    /// build tuple, detected from Cargo.toml, Cargo.lock and git (--detect). Refuses a tree
    /// with uncommitted changes.
    Init {
        #[command(flatten)]
        o: project::InitOpts,
    },
    /// The one command: deploy and register (if nothing is deployed yet; needs --elf),
    /// register (if deployed), or update (if registered and lezreg.toml says something new),
    /// from lezreg.toml. The IDL goes to your Storage node (LEZREG_STORAGE_API). Rerun after a
    /// version bump and it sends an Update; rerun with nothing changed and it sends nothing.
    Publish {
        /// The project's lezreg.toml.
        #[arg(long, default_value = project::FILE)]
        config: std::path::PathBuf,
        /// The program binary (`.bin`) or guest ELF: deployed if the program is not, else
        /// checked against the live program's image.
        #[arg(long)]
        elf: Option<std::path::PathBuf>,
        /// The program id (default: lezreg.toml's [deploy] program; none: deploy to a new
        /// header account, recorded in lezreg.toml).
        #[arg(long)]
        program: Option<String>,
        /// The proven author (default: lezreg.toml's [deploy] author, else the header key).
        #[arg(long)]
        author: Option<String>,
        /// Go ahead although tracked files have uncommitted changes.
        #[arg(long)]
        allow_dirty: bool,
        /// Publish what the GitHub Action prepared: a directory or URL holding
        /// lezreg-release.json, program.bin and idl.json (a GitHub release tag URL works).
        /// Every hash is checked again here; the keys stay on this machine.
        #[arg(long, conflicts_with_all = ["elf", "allow_dirty"])]
        from_release: Option<String>,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// Register an already deployed program whose header key is in the wallet, with the
    /// metadata in the project's lezreg.toml (an update if it is registered already).
    Adopt {
        /// The program id.
        program: String,
        /// The project directory (holding lezreg.toml).
        #[arg(long, default_value = ".")]
        project: std::path::PathBuf,
        /// Entry name, instead of lezreg.toml's.
        #[arg(long)]
        name: Option<String>,
        /// The proven author (default: lezreg.toml's [deploy] author, else the header key).
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        allow_dirty: bool,
        #[command(flatten)]
        opts: WriteOpts,
    },
    /// The adoption evidence table, from chain: entries whose program is deployed (others are
    /// listed as not counted), distinct programs, proven authors and images; stale entries
    /// are flagged.
    Stats {
        /// Per month (UTC) of registration: entries, programs, authors, new authors.
        #[arg(long)]
        by_month: bool,
        /// Per proven author: entries and names.
        #[arg(long)]
        by_author: bool,
        /// Leave out entries whose author or registrant is listed in this file (one account
        /// per line, `#` comments): your own team's keys.
        #[arg(long)]
        exclude_keys: Option<std::path::PathBuf>,
    },
    /// Check this machine: sequencer, registry id, Storage node, r0vm, wallet, Docker, clock.
    Doctor {
        #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
        storage_api: String,
    },
    /// Release payloads for the GitHub Action (DESIGN §7.6).
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
    },
    /// Start a Logos Storage node in the background (for authors who don't run one).
    Storage {
        #[command(subcommand)]
        cmd: StorageCmd,
    },
    /// Pin every IDL in the registry (a self-hostable pinner). With --every, repeat forever.
    PinAll {
        /// Walk again every N seconds (at least 60).
        #[arg(long)]
        every: Option<u64>,
        #[command(flatten)]
        storage: storage_cmd::StorageOpts,
    },
    /// Show or set network settings (sequencer RPC and registry id).
    Network {
        #[command(subcommand)]
        cmd: NetworkCmd,
    },
    /// The LEZ wallet's own commands: accounts, balances, transfers. `lezreg wallet help`.
    #[command(disable_help_flag = true)]
    Wallet {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SearchBy {
    /// By name: exact matches first, then names containing the query.
    Name {
        query: String,
        #[command(flatten)]
        o: SearchOpts,
    },
    /// By proven author account: the entries it holds now.
    Author {
        account: String,
        #[command(flatten)]
        o: SearchOpts,
    },
    /// By free-text author label (UNPROVEN: anyone can write any label).
    Label {
        query: String,
        #[command(flatten)]
        o: SearchOpts,
    },
    /// By tag.
    Tag {
        tag: String,
        #[command(flatten)]
        o: SearchOpts,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Write lezreg-release.json, program.bin and idl.json (canonical) for a built binary:
    /// the unsigned payload `publish --from-release` checks and signs. With --live, the
    /// program's live image must equal the binary's.
    Prepare {
        /// The binary `lezreg build` wrote (`.bin` or guest ELF).
        #[arg(long)]
        elf: std::path::PathBuf,
        /// Output directory.
        #[arg(long = "out", default_value = "lezreg-release")]
        out_dir: std::path::PathBuf,
        #[arg(long, default_value = project::FILE)]
        config: std::path::PathBuf,
        /// The program id (default: lezreg.toml's [deploy] program).
        #[arg(long)]
        program: Option<String>,
        /// Compare with the live program on the sequencer (--rpc).
        #[arg(long)]
        live: bool,
        #[arg(long)]
        allow_dirty: bool,
    },
}

#[derive(Subcommand)]
enum StorageCmd {
    /// Start the node, wait until it advertises an address, print the API to use.
    Up {
        #[command(flatten)]
        o: storage_cmd::UpOpts,
    },
}

#[derive(Subcommand)]
enum IdlCmd {
    /// Fetch an entry's IDL from Logos Storage, check its hash against the chain, print it.
    Get {
        /// Program id or name.
        target: String,
        /// Save the exact (canonical) bytes here instead of printing.
        #[arg(long, short)]
        out: Option<std::path::PathBuf>,
        #[command(flatten)]
        storage: storage_cmd::StorageOpts,
    },
    /// Canonicalize an IDL (RFC 8785), upload it to your Storage node, print its reference.
    Push {
        file: std::path::PathBuf,
        #[command(flatten)]
        storage: storage_cmd::StorageOpts,
    },
}

#[derive(Subcommand)]
enum NetworkCmd {
    /// Print the network in use (and where its settings come from).
    Show,
    /// Save a network's RPC and/or registry id in the lezreg config file. A registry id is
    /// checked first (a program with the self-entry `lez_program_registry`).
    Set {
        name: String,
        #[arg(long)]
        rpc: Option<String>,
        #[arg(long)]
        registry: Option<String>,
        /// Save the registry id even if it cannot be checked or does not look like a registry.
        #[arg(long)]
        force: bool,
    },
}

fn reader(net: &Net) -> Result<Registry<HttpRpc>> {
    let rpc = net.rpc()?;
    let reg = net.registry()?;
    let cache = default_cache_path(&reg, &rpc)
        .map(LinkCache::at)
        .unwrap_or_default();
    Ok(Registry::with_cache(HttpRpc::new(&rpc), reg, cache))
}

/// Whether `rpc` is a sequencer on this machine: an http(s) URL whose host is a loopback
/// address or `localhost`, with no user info (`http://127.0.0.1:1@evil.example/` is
/// evil.example).
fn is_local(rpc: &str) -> bool {
    let Ok(u) = url::Url::parse(rpc) else {
        return false;
    };
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return false;
    }
    match u.host() {
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Writes go through a sequencer on this machine unless `--yes`: on any other one they spend
/// real funds. A dry run spends nothing.
fn check_write_target(rpc: &str, yes: bool, dry_run: bool) -> Result<()> {
    if !is_local(rpc) && !yes && !dry_run {
        bail!(
            "{rpc} is not on this machine: writing there spends real funds. Pass --yes to go ahead"
        );
    }
    Ok(())
}

/// Holds an exclusive lock on the wallet for the rest of the process, so two lezreg writes
/// from one wallet never race for the same nonce (the sequencer silently drops the loser).
fn lock_wallet() -> Result<()> {
    use std::os::fd::AsRawFd as _;
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    if LOCK.get().is_some() {
        return Ok(());
    }
    let path = fetch_persistent_storage_path()?.with_file_name("lezreg.lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!(
            "waiting for another lezreg write that uses this wallet ({})",
            path.display()
        );
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("cannot lock {}", path.display());
        }
    }
    let _ = LOCK.set(f);
    Ok(())
}

fn send_opts(opts: &WriteOpts) -> Result<SendOpts> {
    Ok(SendOpts {
        payer: opts.payer.as_deref().map(parse_account).transpose()?,
        dry_run: opts.dry_run,
        force_gas: opts.force_send.then_some(opts.gas_limit),
    })
}

/// The LEZ wallet at the wallet home, pointed at `rpc`. Made (and saved) if missing.
async fn open_wallet(rpc: &str) -> Result<WalletCore<sequencer_service_rpc::SequencerClient>> {
    let overrides = Some(WalletConfigOverrides {
        sequencer: Some(SequencerConnectionData {
            sequencer_addr: rpc.parse().map_err(|_| anyhow!("{rpc} is not a URL"))?,
            basic_auth: None,
        }),
        seq_tx_poll_max_blocks: Some(60),
        ..Default::default()
    });
    write_default_wallet_files(rpc)?;
    lock_wallet()?;
    let storage_path = fetch_persistent_storage_path()?;
    if storage_path.exists() {
        let password = read_password(&storage_path)?;
        return WalletCore::new_update_chain(
            fetch_config_path()?,
            storage_path,
            overrides,
            &password,
        )
        .context("cannot open the wallet (a LEZ 0.3 wallet does not load on 0.4)");
    }
    bail!(
        "there is no wallet at {} yet: make one with `lezreg wallet account new public`",
        storage_path.display()
    )
}

/// Writes the wallet's default config if missing (the wallet would, but announces it on
/// stdout), pointed at `rpc`: never at the LEZ wallet's built-in default sequencer, which the
/// user did not choose. lezreg overrides the sequencer on every run anyway.
fn write_default_wallet_files(rpc: &str) -> Result<()> {
    let config_path = fetch_config_path()?;
    if let Some(home) = config_path.parent() {
        std::fs::create_dir_all(home)
            .with_context(|| format!("failed to create the wallet home {}", home.display()))?;
    }
    if !config_path.exists() {
        let sequencer = SequencerConnectionData {
            sequencer_addr: rpc.parse().map_err(|_| anyhow!("{rpc} is not a URL"))?,
            basic_auth: None,
        };
        let config = wallet::config::WalletConfig {
            sequencer,
            ..Default::default()
        };
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&serde_json::to_value(config)?)?,
        )?;
    }
    Ok(())
}

/// Wallet subcommands that never send a transaction.
const WALLET_READ_ONLY: &[&str] = &[
    "account",
    "chain-info",
    "check-health",
    "config",
    "set-password",
    "restore-keys",
    "help",
];

async fn writer(net: &Net, opts: &WriteOpts) -> Result<Writer> {
    writer_for(net, opts, true).await
}

/// `need_registry` false: a plain loader deploy, which needs no registry id.
async fn writer_for(net: &Net, opts: &WriteOpts, need_registry: bool) -> Result<Writer> {
    let rpc = net.rpc()?;
    check_write_target(&rpc, opts.yes, opts.dry_run)?;
    let reg = if need_registry {
        net.registry()?
    } else {
        net.registry().unwrap_or_default()
    };
    let w = out::quietly(|| async { open_wallet(&rpc).await }).await?;
    Ok(Writer::new(w, lid(&reg)))
}

/// Runs the bundled LEZ wallet CLI (LNS pattern), always against the network's sequencer:
/// a network with no RPC is refused rather than falling back to whatever the wallet's own
/// config names. `--yes` (anywhere in the arguments) allows sending through a sequencer that
/// is not on this machine. `change-network` is refused: the sequencer is lezreg's
/// `--network`/`--rpc`.
async fn run_wallet(mut args: Vec<String>, net: Result<Net>) -> Result<()> {
    use clap::CommandFactory as _;
    use wallet::{
        ExecutionFailureKind,
        cli::{Args, execute_continuous_run, execute_subcommand},
    };
    let yes = args.iter().any(|a| a == "--yes");
    args.retain(|a| a != "--yes");
    // The subcommand (a flag's value can land here too; that only makes the guard stricter).
    let sub = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_default();
    let help = sub.is_empty() && !args.iter().any(|a| a == "-c" || a == "--continuous-run")
        || sub == "help";
    let mut rpc = String::new();
    if !help {
        if sub == "change-network" {
            bail!(
                "lezreg's wallet always uses lezreg's sequencer: choose it with --network or --rpc (and `lezreg network set`), not change-network"
            );
        }
        rpc = net?.rpc()?;
        if !WALLET_READ_ONLY.contains(&sub.as_str()) {
            check_write_target(&rpc, yes, args.iter().any(|a| a == "--dry-run"))?;
            lock_wallet()?;
        }
    }
    let Args {
        continuous_run,
        dry_run,
        fee_headroom,
        fee_payer,
        refund_to,
        command,
    } = Args::parse_from(std::iter::once("lezreg wallet".to_owned()).chain(args.iter().cloned()));
    if help || (command.is_none() && !continuous_run) {
        println!(
            "{}",
            Args::command().name("lezreg wallet").render_long_help()
        );
        return Ok(());
    }
    let mut overrides = WalletConfigOverrides {
        seq_tx_poll_max_blocks: Some(60),
        ..Default::default()
    };
    overrides.sequencer = Some(SequencerConnectionData {
        sequencer_addr: rpc.parse().map_err(|_| anyhow!("{rpc} is not a URL"))?,
        basic_auth: None,
    });
    // Native transfers meter no cycles and the whole limit is reserved: keep it tiny.
    if args.iter().any(|a| a == "auth-transfer") {
        overrides.gas_limit = Some(1_000);
    }
    if fee_headroom.is_some() {
        overrides.private_fee_headroom_blocks = fee_headroom;
    }
    let overrides = Some(overrides);
    write_default_wallet_files(&rpc)?;
    let config_path = fetch_config_path()?;
    let storage_path = fetch_persistent_storage_path()?;
    if let Some(command) = command {
        let mut wallet = if storage_path.exists() {
            let password = read_password(&storage_path)?;
            WalletCore::new_update_chain(config_path, storage_path, overrides, &password)?
        } else {
            let password = std::env::var(PASSWORD_ENV_VAR).unwrap_or_default();
            let (mut wallet, mnemonic) =
                WalletCore::new_init_storage(config_path, storage_path, overrides, &password)?;
            wallet.start_at_chain_tip().await.ok();
            eprintln!(
                "Created a new wallet. Write down its recovery phrase and keep it safe; it is the only way to get the wallet back:\n"
            );
            eprintln!("  {mnemonic}\n");
            if password.is_empty() {
                eprintln!(
                    "The wallet is NOT encrypted (no password set). Its keys, including any program's upgrade (header) key, are readable by anyone who can read {}. Encrypt it with `lezreg wallet set-password`.\n",
                    fetch_persistent_storage_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
            }
            wallet.store_persistent_data()?;
            wallet
        };
        wallet.set_dry_run(dry_run);
        wallet.set_private_fee_payer(fee_payer);
        wallet.set_private_refund_target(refund_to);
        match execute_subcommand(&mut wallet, command).await {
            Err(error)
                if matches!(
                    error.root_cause().downcast_ref(),
                    Some(ExecutionFailureKind::DryRun)
                ) => {}
            result => {
                result?;
            }
        }
    } else {
        let password = read_password(&storage_path)?;
        let mut wallet =
            WalletCore::new_update_chain(config_path, storage_path, overrides, &password)?;
        execute_continuous_run(&mut wallet).await?;
    }
    Ok(())
}

/// How many entries of a name list a write resolves a name over.
const NAME_WALK: usize = 10_000;

/// The entry address a write targets, resolving a name through the whole name list: names are
/// not unique (anyone can register any name), so among the deployer entries with that name it
/// picks the one whose author's key this wallet holds.
fn target_address(net: &Net, target: &Target, w: &Writer) -> Result<AccountId> {
    if let Some(e) = &target.entry {
        return parse_account(e);
    }
    let name = target.target.as_deref().unwrap_or_default();
    let program = match parse_account(name) {
        Ok(p) => p,
        Err(_) => {
            let r = reader(net)?;
            let name = name.to_ascii_lowercase();
            let found = r.by_name(&name, NAME_WALK)?;
            let deployers: Vec<_> = found
                .iter()
                .filter(|i| i.kind == Some(lezreg_sdk::Kind::Deployer))
                .collect();
            let mine: Vec<_> = deployers
                .iter()
                .copied()
                .filter(|i| i.entry.as_ref().is_some_and(|e| w.holds(lid(&e.author))))
                .collect();
            match (mine.as_slice(), deployers.as_slice()) {
                ([one], _) | ([], [one]) => lid(&one.entry.as_ref().expect("checked").program),
                ([], []) if found.len() == 1 => lid(&found[0].link.program),
                ([], []) => bail!(
                    "'{name}' names no deployer entry ({} entries in all); give the program id instead",
                    found.len()
                ),
                ([], many) => bail!(
                    "'{name}' names {} deployer entries and this wallet is the author of none; give the program id instead",
                    many.len()
                ),
                (many, _) => bail!(
                    "'{name}' names {} deployer entries this wallet is the author of; give the program id instead",
                    many.len()
                ),
            }
        }
    };
    Ok(match &target.publisher {
        Some(p) => lid(&addr::third_party_entry(
            &net.registry()?,
            &tid(&program),
            &tid(&parse_account(p)?),
        )),
        None => program,
    })
}

/// Checks that `registry` on `rpc` is a LEZ Program Registry: a deployed program whose
/// self-entry `(R, R)` is named `lez_program_registry`.
fn check_registry(rpc: &str, registry: &lezreg_types::AccountId) -> Result<()> {
    let r = Registry::new(HttpRpc::new(rpc), *registry);
    let res = r.resolve(registry)?;
    if res.header.is_none() {
        bail!(
            "{registry} is not a LEZ Program Registry on {rpc}: no program is deployed there (--force saves it anyway)"
        );
    }
    match res.deployer {
        Some(e) if e.name == "lez_program_registry" => Ok(()),
        _ => bail!(
            "{registry} is not a LEZ Program Registry on {rpc}: it has no self-entry named lez_program_registry (--force saves it anyway)"
        ),
    }
}

async fn run(cli: Cli, out: &mut Out) -> Result<()> {
    let net = config::resolve(&cli.network, cli.rpc.as_deref(), cli.registry.as_deref())?;
    match cli.command {
        Command::Wallet { .. } => unreachable!("handled in main"),
        Command::Network { cmd } => match cmd {
            NetworkCmd::Show => {
                out.text(&format!(
                    "network  {}\nrpc      {}\nregistry {}\nconfig   {}",
                    net.name,
                    net.rpc.as_deref().unwrap_or("(none)"),
                    net.registry
                        .map_or_else(|| "(none)".to_owned(), |r| r.to_base58()),
                    config::path()
                        .map_or_else(|| "(no config dir)".into(), |p| p.display().to_string())
                ));
                out.json(json!({"network": net.name, "rpc": net.rpc, "registry": net.registry.map(|r| r.to_base58())}));
            }
            NetworkCmd::Set {
                name,
                rpc,
                registry,
                force,
            } => {
                if let Some(reg) = &registry
                    && !force
                {
                    let reg = config::parse_registry(reg)?;
                    let target = config::resolve(&name, rpc.as_deref(), None)
                        .ok()
                        .and_then(|n| n.rpc)
                        .or_else(|| rpc.clone());
                    let target = target.ok_or_else(|| anyhow!("network '{name}' has no RPC to check the registry on: pass --rpc (or --force)"))?;
                    check_registry(&target, &reg)?;
                }
                let p = config::set(&name, rpc.as_deref(), registry.as_deref())?;
                out.text(&format!("saved network '{name}' in {}", p.display()));
                out.json(json!({"saved": name, "file": p.display().to_string()}));
            }
        },
        Command::Idl { cmd } => match cmd {
            IdlCmd::Get {
                target,
                out: save,
                storage,
            } => storage_cmd::get(out, &reader(&net)?, &target, &storage, save.as_deref())?,
            IdlCmd::Push { file, storage } => {
                storage_cmd::push(out, &file, &storage)?;
            }
        },
        Command::Verify {
            program,
            revision,
            work,
            allow_network,
            mark,
        } => {
            verify_cmd::verify(
                out,
                &reader(&net)?,
                &net.name,
                &program,
                revision,
                work.as_deref(),
                allow_network,
                mark,
            )?;
        }
        Command::Verified { forget } => verify_cmd::marks(out, forget.as_deref())?,
        Command::Build {
            dir,
            config,
            build,
            work,
        } => {
            let (dir, b) = match &build.binary {
                Some(binary) => (
                    dir.unwrap_or_else(|| ".".into()),
                    build.apply(args::default_build(binary)?)?,
                ),
                None => {
                    let p = project::Project::load(&config)
                        .map_err(|e| anyhow!("{e:#}\n(or pass --binary and the build flags)"))?;
                    let b = build.apply(publish::build_config(&p.m, &[0; 32])?)?;
                    let dir = match dir {
                        Some(d) => d,
                        None => p.docker_root_dir()?,
                    };
                    (dir, b)
                }
            };
            verify_cmd::build(out, &dir, &b, work.as_deref())?;
        }
        Command::Init { o } => {
            let (path, m, notes) = project::init(&o)?;
            for n in &notes {
                eprintln!("note: {n}");
            }
            out.text(&format!(
                "wrote {}: {} {} (guest {} in {}, risc0-build {})\nnext: commit it, then `lezreg publish --elf <program .bin>` (`lezreg build` makes one)",
                path.display(),
                m.program.name,
                m.program.version,
                m.build.binary,
                m.build.manifest_path,
                m.build.risc0_build
            ));
            out.json(json!({"file": path.display().to_string(), "manifest": serde_json::to_value(&m)?, "notes": notes}));
        }
        Command::Publish {
            config,
            elf,
            program,
            author,
            allow_dirty,
            from_release,
            opts,
        } => {
            let program = program.as_deref().map(parse_account).transpose()?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let mut proj = if from_release.is_none() || config.exists() {
                Some(project::Project::load(&config)?)
            } else {
                None
            };
            let mut plan = match &from_release {
                Some(src) => {
                    let scratch =
                        std::env::temp_dir().join(format!("lezreg-release-{}", std::process::id()));
                    let (payload, bin, idl) = publish::fetch_release(src, &scratch)?;
                    eprintln!(
                        "release payload: {} {} commit {} image {} (sizes, sha256s and image id checked)",
                        payload.name, payload.version, payload.commit, payload.image_id
                    );
                    if let Some(p) = &proj {
                        if p.m.program.name != payload.name {
                            eprintln!(
                                "note: {} names {}, the release {}; publishing the release",
                                p.path.display(),
                                p.m.program.name,
                                payload.name
                            );
                        }
                        if publish::check_against_checkout(&payload, p)? == Some(true) {
                            eprintln!("Cargo.lock checked against this checkout");
                        }
                    }
                    let mut plan = payload.plan(bin, idl)?;
                    if plan.program.is_none() {
                        plan.program = proj
                            .as_ref()
                            .and_then(project::Project::program)
                            .map(parse_account)
                            .transpose()?;
                    }
                    plan
                }
                None => {
                    let bin = elf.as_deref().map(publish::read_bin).transpose()?;
                    publish::plan_from_project(
                        proj.as_ref().expect("loaded"),
                        bin,
                        program,
                        allow_dirty,
                    )?
                }
            };
            if program.is_some() {
                plan.program = program;
            }
            if author.is_some() {
                plan.author = author;
            }
            publish::publish(
                out,
                &net,
                &plan,
                &opts,
                proj.as_mut(),
                publish::Mode::Publish,
            )
            .await?;
        }
        Command::Adopt {
            program,
            project: dir,
            name,
            author,
            allow_dirty,
            opts,
        } => {
            let mut p = project::Project::load(&dir.join(project::FILE))?;
            if let Some(n) = name {
                p.m.program.name = n;
            }
            let program = parse_account(&program)?;
            let mut plan = publish::plan_from_project(&p, None, Some(program), allow_dirty)?;
            if let Some(a) = author {
                plan.author = Some(parse_account(&a)?);
            }
            publish::publish(out, &net, &plan, &opts, Some(&mut p), publish::Mode::Adopt).await?;
        }
        Command::Stats {
            by_month,
            by_author,
            exclude_keys,
        } => {
            let exclude = exclude_keys
                .as_deref()
                .map(stats::read_keys)
                .transpose()?
                .unwrap_or_default();
            let r = reader(&net)?;
            let (rows, unreadable) = stats::collect(&r)?;
            r.cache().save().ok();
            let s = stats::aggregate(&rows, unreadable, &exclude);
            stats::report(out, &s, by_month, by_author);
        }
        Command::Doctor { storage_api } => {
            let checks = doctor::run_checks(&net, &storage_api);
            if !doctor::report(out, &checks) {
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Command::Release {
            cmd:
                ReleaseCmd::Prepare {
                    elf,
                    out_dir,
                    config,
                    program,
                    live,
                    allow_dirty,
                },
        } => {
            let p = project::Project::load(&config)?;
            let program = program.as_deref().map(parse_account).transpose()?;
            publish::prepare(out, &net, &p, &elf, &out_dir, program, live, allow_dirty)?;
        }
        Command::Storage {
            cmd: StorageCmd::Up { o },
        } => storage_cmd::up(out, &o)?,
        Command::Pin { target, storage } => {
            storage_cmd::pin(out, &reader(&net)?, &target, &storage)?
        }
        Command::PinAll { every, storage } => {
            storage_cmd::pin_all(out, &reader(&net)?, &storage, every)?
        }
        Command::Show {
            target,
            publisher,
            revision,
        } => render::show(out, &reader(&net)?, &target, publisher.as_deref(), revision)?,
        Command::List { limit, after } => {
            let r = reader(&net)?;
            let after = after
                .as_deref()
                .map(parse_account)
                .transpose()?
                .map(|a| tid(&a));
            let page = r.list(after.as_ref(), limit)?;
            render::list(out, &r, &page)?;
            r.cache().save().ok();
        }
        Command::Search { by } => {
            let r = reader(&net)?;
            let mut exact = None;
            let (title, found, proven) = match by {
                SearchBy::Name { query, o } => {
                    exact = Some(query.to_ascii_lowercase());
                    (
                        format!("name '{query}'"),
                        r.search_name(&query, o.limit, o.scan)?,
                        true,
                    )
                }
                SearchBy::Author { account, o } => {
                    let a = tid(&parse_account(&account)?);
                    (
                        format!("author {a} (proven co-signer)"),
                        r.by_author(&a, o.limit, o.scan)?,
                        true,
                    )
                }
                SearchBy::Label { query, o } => (
                    format!("author label '{query}' (UNPROVEN)"),
                    r.search_label(&query, o.limit, o.scan)?,
                    false,
                ),
                SearchBy::Tag { tag, o } => (
                    format!("tag '{tag}'"),
                    r.search_tag(&tag, o.limit, o.scan)?,
                    true,
                ),
            };
            render::search(out, &r, &title, &found, proven, exact.as_deref())?;
            r.cache().save().ok();
        }
        Command::Register {
            program,
            author,
            third_party,
            publisher,
            entry,
            opts,
        } => {
            let program = parse_account(&program)?;
            let so = send_opts(&opts)?;
            let input = entry.input(None)?;
            note_build_defaults(&entry);
            let mut w = writer(&net, &opts).await?;
            let receipt = if third_party {
                let publisher = parse_account(
                    publisher
                        .as_deref()
                        .ok_or_else(|| anyhow!("--third-party needs --publisher"))?,
                )?;
                if author
                    .as_deref()
                    .is_some_and(|a| parse_account(a).ok() != Some(publisher))
                {
                    bail!("a third-party entry's author is always its publisher");
                }
                w.register_third_party(program, publisher, &input, &so)
                    .await?
            } else {
                let author = author
                    .as_deref()
                    .map(parse_account)
                    .transpose()?
                    .unwrap_or(program);
                w.register_deployer(program, author, &input, &so).await?
            };
            render::receipt(out, &receipt);
        }
        Command::Deploy {
            elf,
            program,
            immutable,
            no_register,
            author,
            entry,
            opts,
        } => {
            // Everything that can be checked offline first: no account is made for a bad
            // command line.
            let bytes =
                std::fs::read(&elf).with_context(|| format!("cannot read {}", elf.display()))?;
            let input = if no_register {
                None
            } else {
                Some(entry.input(Some(&bytes))?)
            };
            if !no_register {
                note_build_defaults(&entry);
            }
            let given = program.as_deref().map(parse_account).transpose()?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let so = send_opts(&opts)?;
            let mut w = writer_for(&net, &opts, !no_register).await?;
            // A new header account lives only in memory until the first segment write saves
            // the wallet: a refusal before that leaves nothing behind.
            let program = match given {
                Some(p) => p,
                None => w.new_account().await?,
            };
            let register = input.as_ref().map(|i| (author.unwrap_or(program), i));
            let result = w.deploy(&bytes, program, immutable, register, &so).await;
            // On failure, name the header account only if its key is in the wallet (given, or
            // saved by the first segment write), so a rerun can finish the deploy there.
            if given.is_some() || w.wrote() {
                out.context("program", json!(program.to_string()));
            }
            let receipt = result?;
            out.text(&format!("program {program}"));
            render::receipt(out, &receipt);
            out.json_merge(json!({"program": program.to_string()}));
            if given.is_none() && receipt.hash.is_some() {
                eprintln!(
                    "note: {program}'s header key is in the wallet at {}. It is the program's upgrade key{}: back the wallet up (and encrypt it with `lezreg wallet set-password`)",
                    fetch_persistent_storage_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    if immutable {
                        " (the code is immutable, but the key still signs as the program)"
                    } else {
                        ""
                    }
                );
            }
        }
        Command::Update {
            target,
            author,
            patch,
            opts,
        } => {
            let mut w = writer(&net, &opts).await?;
            let at = target_address(&net, &target, &w)?;
            let stored = reader(&net)?
                .entry(&tid(&at))?
                .ok_or(SdkError::NoEntry(tid(&at)))?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let p = patch.input(&stored)?;
            let receipt = w.update(at, author, &p, &send_opts(&opts)?).await?;
            render::receipt(out, &receipt);
        }
        Command::Transfer {
            target,
            to,
            author,
            opts,
        } => {
            let mut w = writer(&net, &opts).await?;
            let at = target_address(&net, &target, &w)?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let receipt = w
                .transfer(at, author, parse_account(&to)?, &send_opts(&opts)?)
                .await?;
            render::receipt(out, &receipt);
        }
        Command::Freeze {
            target,
            author,
            opts,
        } => {
            let mut w = writer(&net, &opts).await?;
            let at = target_address(&net, &target, &w)?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let receipt = w.freeze(at, author, &send_opts(&opts)?).await?;
            render::receipt(out, &receipt);
        }
    }
    Ok(())
}

/// Set by a command that printed its result but must still exit non-zero (`doctor`).
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says when an entry gets lezreg's default build settings rather than the author's.
fn note_build_defaults(entry: &args::EntryArgs) {
    if !entry.build.any() {
        eprintln!(
            "note: no build flags given, so the entry records lezreg's default build settings (risc0-guest-builder r0.1.91.1, binary = the name) and no Cargo.lock hash; `show` marks such build info as unverified"
        );
    }
}

/// Error kind names for `--json` (stable).
fn error_kind(e: &anyhow::Error) -> &'static str {
    match e.downcast_ref::<SdkError>() {
        Some(SdkError::Transport { .. }) => "transport",
        Some(SdkError::Rpc { .. }) => "rpc",
        Some(SdkError::BadRecord { .. }) => "bad_record",
        Some(SdkError::MissingLink(_)) => "missing_link",
        Some(SdkError::Cycle(_)) => "cycle",
        Some(SdkError::NoEntry(_)) => "no_entry",
        Some(SdkError::NotDeployed(_)) => "not_deployed",
        Some(SdkError::Invalid(_)) => "invalid",
        Some(SdkError::Config(_)) => "config",
        Some(SdkError::NoKey(_)) => "no_key",
        Some(SdkError::NotAuthor { .. }) => "not_author",
        Some(SdkError::Frozen(_)) => "frozen",
        Some(SdkError::Refused(_)) => "refused",
        Some(SdkError::Funds(_)) => "funds",
        Some(SdkError::Rejected(_)) => "rejected",
        Some(SdkError::NotIncluded { .. }) => "not_included",
        Some(SdkError::Reverted { .. }) => "reverted",
        Some(SdkError::ReadBack(_)) => "read_back",
        Some(SdkError::Wallet(_)) => "wallet",
        None => "error",
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // `lezreg list | head` ends quietly instead of panicking on the closed pipe.
    // SAFETY: setting a signal disposition before any other thread writes to stdout.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Command::Wallet { args } = &cli.command {
        let net = config::resolve(&cli.network, cli.rpc.as_deref(), cli.registry.as_deref());
        if let Err(e) = run_wallet(args.clone(), net).await {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let mut out = Out::new(cli.json);
    let result = run(cli, &mut out).await;
    if let Err(e) = &result {
        out.error(error_kind(e), &format!("{e:#}"));
        if let Some(h) = e.downcast_ref::<SdkError>().and_then(SdkError::tx_hash) {
            out.error_field("tx", json!(h));
        }
    }
    out.finish();
    if result.is_err() || FAILED.load(std::sync::atomic::Ordering::Relaxed) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every subcommand's arguments are consistent (no clashing ids such as `--binary` in
    /// both `build` and its flattened build flags).
    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
        let p = Cli::try_parse_from(["lezreg", "publish", "--from-release", "x", "--elf", "y"]);
        assert!(p.is_err(), "--from-release conflicts with --elf");
        assert!(Cli::try_parse_from(["lezreg", "build", "--binary", "g"]).is_ok());
        assert!(Cli::try_parse_from(["lezreg", "init", "--detect", "--version", "1.0.0"]).is_ok());
    }

    /// Review fix (conformance 6, robustness 9): the write guard parses the URL.
    #[test]
    fn local_means_a_loopback_host() {
        for ok in [
            "http://127.0.0.1:3062",
            "http://localhost/",
            "https://127.0.0.1:3062/",
            "http://[::1]:3062",
            "http://LOCALHOST:1",
            "http://127.0.0.2:5/",
        ] {
            assert!(is_local(ok), "{ok}");
        }
        for no in [
            "http://127.0.0.1:3062@evil.example/",
            "http://127.0.0.1:1@example.invalid:3062",
            "http://user@127.0.0.1:3062",
            "http://192.168.1.50:3062",
            "https://testnet.lez.logos.co/",
            "http://127.0.0.1.evil.example/",
            "file:///tmp/x",
            "not a url",
        ] {
            assert!(!is_local(no), "{no}");
        }
        assert!(check_write_target("http://192.168.1.50:3062", false, false).is_err());
        assert!(check_write_target("http://192.168.1.50:3062", true, false).is_ok());
        assert!(
            check_write_target("http://192.168.1.50:3062", false, true).is_ok(),
            "a dry run spends nothing"
        );
    }

    /// Review fix (conformance 1, outsider 2): `lezreg wallet` never falls back to the wallet
    /// config's sequencer (the LEZ default is the public testnet).
    #[tokio::test]
    async fn wallet_needs_a_network_with_an_rpc() {
        let net = Net {
            name: "testnet".into(),
            rpc: None,
            registry: None,
        };
        let e = run_wallet(vec!["check-health".into()], Ok(net))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("testnet 0.4 has not launched"),
            "{e}"
        );
        let e = run_wallet(
            vec!["check-health".into()],
            Err(anyhow!("unknown network 'bogus'")),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("unknown network"), "{e}");
        let net = Net {
            name: "local".into(),
            rpc: Some("http://127.0.0.1:3062".into()),
            registry: None,
        };
        let e = run_wallet(vec!["change-network".into(), "testnet".into()], Ok(net))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("change-network"), "{e}");
        let net = Net {
            name: "x".into(),
            rpc: Some("http://192.168.1.50:3062".into()),
            registry: None,
        };
        let e = run_wallet(vec!["auth-transfer".into(), "send".into()], Ok(net))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("--yes"), "{e}");
    }
}
