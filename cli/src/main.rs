//! `lezreg`: the LEZ Program Registry command line (DESIGN §7.2).
//!
//! Reads (`show`, `list`, `search`) talk to one LEZ sequencer with plain JSON-RPC and need no
//! wallet. Writes (`register`, `update`, `transfer`, `freeze`, `deploy`) sign with the LEZ
//! wallet, built in as `lezreg wallet`, whose keys live in `LEE_WALLET_HOME_DIR` (default
//! `~/.lee/wallet`). Every write is simulated first, sent with gas = 1.5 × simulated cycles,
//! and read back from the chain. No server of ours is ever on the path.

mod args;
mod config;
mod out;
mod render;

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
    /// Account that pays the fee (the wallet must hold its key). Default: the first signer.
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
    /// List every entry, newest first, a page at a time.
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
    /// transaction (DeployAndRegister), unless --no-register.
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
    /// Hand an entry to a new author. The current and the new author both sign.
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
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// How many of the newest entries a substring search scans.
        #[arg(long, default_value_t = 1000)]
        scan: usize,
    },
    /// By proven author account: the entries it holds now.
    Author {
        account: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// By free-text author label (UNPROVEN: anyone can write any label).
    Label {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 1000)]
        scan: usize,
    },
    /// By tag.
    Tag {
        tag: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 1000)]
        scan: usize,
    },
}

#[derive(Subcommand)]
enum NetworkCmd {
    /// Print the network in use (and where its settings come from).
    Show,
    /// Save a network's RPC and/or registry id in the lezreg config file.
    Set {
        name: String,
        #[arg(long)]
        rpc: Option<String>,
        #[arg(long)]
        registry: Option<String>,
    },
}

fn reader(net: &Net) -> Result<Registry<HttpRpc>> {
    let rpc = net.rpc()?;
    let reg = net.registry()?;
    let cache = default_cache_path(&reg, &rpc).map(LinkCache::at).unwrap_or_default();
    Ok(Registry::with_cache(HttpRpc::new(&rpc), reg, cache))
}

/// Writes go through a sequencer on this machine unless `--yes`: on any other one they spend
/// real funds.
fn check_write_target(rpc: &str, opts: &WriteOpts) -> Result<()> {
    let local = ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"].iter().any(|p| rpc.starts_with(p));
    if !local && !opts.yes {
        bail!("{rpc} is not on this machine: writing there spends real funds. Pass --yes to go ahead");
    }
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
        sequencer: Some(SequencerConnectionData { sequencer_addr: rpc.parse().map_err(|_| anyhow!("{rpc} is not a URL"))?, basic_auth: None }),
        seq_tx_poll_max_blocks: Some(60),
        ..Default::default()
    });
    write_default_wallet_files()?;
    let storage_path = fetch_persistent_storage_path()?;
    if storage_path.exists() {
        let password = read_password(&storage_path)?;
        return WalletCore::new_update_chain(fetch_config_path()?, storage_path, overrides, &password)
            .context("cannot open the wallet (a LEZ 0.3 wallet does not load on 0.4)");
    }
    bail!("there is no wallet at {} yet: make one with `lezreg wallet account new public`", storage_path.display())
}

/// Writes the wallet's default config if missing (the wallet would, but announces it on stdout).
fn write_default_wallet_files() -> Result<()> {
    let config_path = fetch_config_path()?;
    if let Some(home) = config_path.parent() {
        std::fs::create_dir_all(home).with_context(|| format!("failed to create the wallet home {}", home.display()))?;
    }
    if !config_path.exists() {
        std::fs::write(&config_path, serde_json::to_vec_pretty(&serde_json::to_value(wallet::config::WalletConfig::default())?)?)?;
    }
    Ok(())
}

async fn writer(net: &Net, opts: &WriteOpts) -> Result<Writer> {
    writer_for(net, opts, true).await
}

/// `need_registry` false: a plain loader deploy, which needs no registry id.
async fn writer_for(net: &Net, opts: &WriteOpts, need_registry: bool) -> Result<Writer> {
    let rpc = net.rpc()?;
    check_write_target(&rpc, opts)?;
    let reg = if need_registry { net.registry()? } else { net.registry().unwrap_or_default() };
    let w = out::quietly(|| async { open_wallet(&rpc).await }).await?;
    Ok(Writer::new(w, lid(&reg)))
}

/// Runs the bundled LEZ wallet CLI (LNS pattern).
async fn run_wallet(args: Vec<String>, rpc: Option<String>) -> Result<()> {
    use clap::CommandFactory as _;
    use wallet::{
        ExecutionFailureKind,
        cli::{Args, execute_continuous_run, execute_subcommand},
    };
    let Args { continuous_run, dry_run, fee_headroom, fee_payer, refund_to, command } =
        Args::parse_from(std::iter::once("lezreg wallet".to_owned()).chain(args.iter().cloned()));
    let mut overrides = WalletConfigOverrides { seq_tx_poll_max_blocks: Some(60), ..Default::default() };
    if let Some(rpc) = rpc {
        overrides.sequencer = Some(SequencerConnectionData { sequencer_addr: rpc.parse().map_err(|_| anyhow!("{rpc} is not a URL"))?, basic_auth: None });
    }
    // Native transfers meter no cycles and the whole limit is reserved: keep it tiny.
    if args.iter().any(|a| a == "auth-transfer") {
        overrides.gas_limit = Some(1_000);
    }
    if fee_headroom.is_some() {
        overrides.private_fee_headroom_blocks = fee_headroom;
    }
    let overrides = Some(overrides);
    write_default_wallet_files()?;
    let config_path = fetch_config_path()?;
    let storage_path = fetch_persistent_storage_path()?;
    if let Some(command) = command {
        let mut wallet = if storage_path.exists() {
            let password = read_password(&storage_path)?;
            WalletCore::new_update_chain(config_path, storage_path, overrides, &password)?
        } else {
            let password = std::env::var(PASSWORD_ENV_VAR).unwrap_or_default();
            let (mut wallet, mnemonic) = WalletCore::new_init_storage(config_path, storage_path, overrides, &password)?;
            wallet.start_at_chain_tip().await.ok();
            eprintln!("Created a new wallet. Write down its recovery phrase and keep it safe; it is the only way to get the wallet back:\n");
            eprintln!("  {mnemonic}\n");
            wallet.store_persistent_data()?;
            wallet
        };
        wallet.set_dry_run(dry_run);
        wallet.set_private_fee_payer(fee_payer);
        wallet.set_private_refund_target(refund_to);
        match execute_subcommand(&mut wallet, command).await {
            Err(error) if matches!(error.root_cause().downcast_ref(), Some(ExecutionFailureKind::DryRun)) => {}
            result => {
                result?;
            }
        }
    } else if continuous_run {
        let password = read_password(&storage_path)?;
        let mut wallet = WalletCore::new_update_chain(config_path, storage_path, overrides, &password)?;
        execute_continuous_run(&mut wallet).await?;
    } else {
        println!("{}", Args::command().name("lezreg wallet").render_long_help());
    }
    Ok(())
}

/// The entry address a write targets, resolving a name through the name list.
fn target_address(net: &Net, target: &Target) -> Result<AccountId> {
    if let Some(e) = &target.entry {
        return parse_account(e);
    }
    let name = target.target.as_deref().unwrap_or_default();
    let program = match parse_account(name) {
        Ok(p) => p,
        Err(_) => {
            let r = reader(net)?;
            let found = r.by_name(name, 10)?;
            let deployers: Vec<_> = found.iter().filter(|i| i.kind == Some(lezreg_sdk::Kind::Deployer)).collect();
            match (deployers.as_slice(), target.publisher.is_some()) {
                ([one], _) => lid(&one.entry.as_ref().expect("checked").program),
                ([], _) if found.len() == 1 => lid(&found[0].link.program),
                _ => bail!("'{name}' names {} entries; give the program id instead", found.len()),
            }
        }
    };
    Ok(match &target.publisher {
        Some(p) => lid(&addr::third_party_entry(&net.registry()?, &tid(&program), &tid(&parse_account(p)?))),
        None => program,
    })
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
                    net.registry.map_or_else(|| "(none)".to_owned(), |r| r.to_base58()),
                    config::path().map_or_else(|| "(no config dir)".into(), |p| p.display().to_string())
                ));
                out.json(json!({"network": net.name, "rpc": net.rpc, "registry": net.registry.map(|r| r.to_base58())}));
            }
            NetworkCmd::Set { name, rpc, registry } => {
                let p = config::set(&name, rpc.as_deref(), registry.as_deref())?;
                out.text(&format!("saved network '{name}' in {}", p.display()));
                out.json(json!({"saved": name, "file": p.display().to_string()}));
            }
        },
        Command::Show { target, publisher, revision } => render::show(out, &reader(&net)?, &target, publisher.as_deref(), revision)?,
        Command::List { limit, after } => {
            let r = reader(&net)?;
            let after = after.as_deref().map(parse_account).transpose()?.map(|a| tid(&a));
            let page = r.list(after.as_ref(), limit)?;
            render::list(out, &r, &page);
            r.cache().save().ok();
        }
        Command::Search { by } => {
            let r = reader(&net)?;
            let (title, items, proven) = match by {
                SearchBy::Name { query, limit, scan } => (format!("name '{query}'"), r.search_name(&query, limit, scan)?, true),
                SearchBy::Author { account, limit } => {
                    let a = tid(&parse_account(&account)?);
                    (format!("author {a} (proven co-signer)"), r.by_author(&a, limit)?, true)
                }
                SearchBy::Label { query, limit, scan } => (format!("author label '{query}' (UNPROVEN)"), r.search_label(&query, limit, scan)?, false),
                SearchBy::Tag { tag, limit, scan } => (format!("tag '{tag}'"), r.search_tag(&tag, limit, scan)?, true),
            };
            render::search(out, &r, &title, &items, proven);
            r.cache().save().ok();
        }
        Command::Register { program, author, third_party, publisher, entry, opts } => {
            let mut w = writer(&net, &opts).await?;
            let program = parse_account(&program)?;
            let so = send_opts(&opts)?;
            let input = entry.input(None)?;
            let receipt = if third_party {
                let publisher = parse_account(publisher.as_deref().ok_or_else(|| anyhow!("--third-party needs --publisher"))?)?;
                if author.as_deref().is_some_and(|a| parse_account(a).ok() != Some(publisher)) {
                    bail!("a third-party entry's author is always its publisher");
                }
                w.register_third_party(program, publisher, &input, &so).await?
            } else {
                let author = author.as_deref().map(parse_account).transpose()?.unwrap_or(program);
                w.register_deployer(program, author, &input, &so).await?
            };
            render::receipt(out, &receipt);
        }
        Command::Deploy { elf, program, immutable, no_register, author, entry, opts } => {
            let mut w = writer_for(&net, &opts, !no_register).await?;
            let bytes = std::fs::read(&elf).with_context(|| format!("cannot read {}", elf.display()))?;
            let program = match program {
                Some(p) => parse_account(&p)?,
                None => {
                    let p = w.new_account().await?;
                    out::quietly(|| async { w.save_wallet() }).await?;
                    eprintln!("new header account (program id) {p}, key saved in the wallet");
                    p
                }
            };
            let so = send_opts(&opts)?;
            let receipt = if no_register {
                w.deploy(&bytes, program, immutable, None, &so).await?
            } else {
                let author = author.as_deref().map(parse_account).transpose()?.unwrap_or(program);
                let input = entry.input(Some(&bytes))?;
                w.deploy(&bytes, program, immutable, Some((author, &input)), &so).await?
            };
            out.text(&format!("program {program}"));
            render::receipt(out, &receipt);
            out.json_merge(json!({"program": program.to_string()}));
        }
        Command::Update { target, author, patch, opts } => {
            let at = target_address(&net, &target)?;
            let stored = reader(&net)?.entry(&tid(&at))?.ok_or(SdkError::NoEntry(tid(&at)))?;
            let mut w = writer(&net, &opts).await?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let p = patch.input(&stored)?;
            let receipt = w.update(at, author, &p, &send_opts(&opts)?).await?;
            render::receipt(out, &receipt);
        }
        Command::Transfer { target, to, author, opts } => {
            let at = target_address(&net, &target)?;
            let mut w = writer(&net, &opts).await?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let receipt = w.transfer(at, author, parse_account(&to)?, &send_opts(&opts)?).await?;
            render::receipt(out, &receipt);
        }
        Command::Freeze { target, author, opts } => {
            let at = target_address(&net, &target)?;
            let mut w = writer(&net, &opts).await?;
            let author = author.as_deref().map(parse_account).transpose()?;
            let receipt = w.freeze(at, author, &send_opts(&opts)?).await?;
            render::receipt(out, &receipt);
        }
    }
    Ok(())
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
        Some(SdkError::NotIncluded(_)) => "not_included",
        Some(SdkError::Reverted { .. }) => "reverted",
        Some(SdkError::ReadBack(_)) => "read_back",
        Some(SdkError::Wallet(_)) => "wallet",
        None => "error",
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = Cli::parse();
    if let Command::Wallet { args } = &cli.command {
        let rpc = config::resolve(&cli.network, cli.rpc.as_deref(), cli.registry.as_deref()).ok().and_then(|n| n.rpc);
        if let Err(e) = run_wallet(args.clone(), rpc).await {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let mut out = Out::new(cli.json);
    let result = run(cli, &mut out).await;
    if let Err(e) = &result {
        out.error(error_kind(e), &format!("{e:#}"));
    }
    out.finish();
    if result.is_err() {
        std::process::exit(1);
    }
}

