#![cfg(feature = "tx")]
//! End to end against a real standalone LEZ sequencer. Skipped unless `LEZREG_E2E_RPC` is set.
//!
//! - `LEZREG_E2E_RPC`: a LOCAL sequencer (loopback only; this never spends testnet funds).
//! - `LEZREG_E2E_REGISTRY`: the deployed registry (`chain/deploy.sh` prints it).
//! - optional `LEZREG_E2E_PAYER` + `LEE_WALLET_HOME_DIR` (a COPY of a funded wallet): also run
//!   writes: register a deployer entry for a fresh account with no program (status
//!   "no-program"), update it, freeze it, and read each change back through the read SDK.
//!
//! Run: `LEZREG_E2E_RPC=http://127.0.0.1:3062 LEZREG_E2E_REGISTRY=<id> cargo test -p lezreg-sdk --test e2e -- --nocapture`

use lezreg_sdk::{Freshness, HttpRpc, Kind, Registry};
use lezreg_types::AccountId;

fn env() -> Option<(String, AccountId)> {
    let rpc = std::env::var("LEZREG_E2E_RPC").ok()?;
    assert!(
        rpc.starts_with("http://127.0.0.1:") || rpc.starts_with("http://localhost:"),
        "LEZREG_E2E_RPC must be a local sequencer"
    );
    let reg = std::env::var("LEZREG_E2E_REGISTRY").expect("set LEZREG_E2E_REGISTRY with LEZREG_E2E_RPC");
    Some((rpc, AccountId::from_base58(reg.trim_start_matches("Public/")).expect("registry id")))
}

#[test]
fn e2e_reads() {
    let Some((rpc, reg)) = env() else {
        eprintln!("skipped: LEZREG_E2E_RPC is not set");
        return;
    };
    let r = Registry::new(HttpRpc::new(&rpc), reg);
    // The registry is registered in itself, at (REG, REG), fresh against its own header.
    let me = r.resolve(&reg).unwrap();
    let e = me.deployer.as_ref().expect("self-entry at (REG, REG)");
    assert_eq!(e.name, "lez_program_registry");
    assert_eq!(me.freshness(), Some(Freshness::Fresh));
    assert!(!e.born_with_deploy(), "the self-entry comes from CreateHeader + RegisterDeployer");
    // The global list ends at the self-entry (seq 0).
    let total = r.count().unwrap();
    assert!(total >= 1);
    let mut after = None;
    let mut seen = 0u64;
    let mut last = None;
    loop {
        let page = r.list(after.as_ref(), 7).unwrap();
        seen += page.items.len() as u64;
        last = page.items.last().cloned().or(last);
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert_eq!(seen, total, "paging visits every entry once");
    let oldest = last.unwrap();
    assert_eq!((oldest.address, oldest.link.seq, oldest.kind), (reg, 0, Some(Kind::Deployer)));
    assert!(r.by_name("lez_program_registry", 10).unwrap().iter().any(|i| i.address == reg));
    assert!(r.by_author(&reg, 10, 1000).unwrap().items.iter().any(|i| i.address == reg));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_writes() {
    use lezreg_sdk::tx::{EntryInput, PatchInput, SendOpts, Writer, tid};
    use lezreg_types::{BuildConfig, ContentRef, SourceRef};
    let Some((rpc, reg)) = env() else {
        eprintln!("skipped: LEZREG_E2E_RPC is not set");
        return;
    };
    let Ok(payer) = std::env::var("LEZREG_E2E_PAYER") else {
        eprintln!("writes skipped: LEZREG_E2E_PAYER is not set");
        return;
    };
    let payer: lee_core::account::AccountId = payer.trim_start_matches("Public/").parse().expect("payer id");
    let overrides = wallet::config::WalletConfigOverrides {
        sequencer: Some(wallet::config::SequencerConnectionData { sequencer_addr: rpc.parse().unwrap(), basic_auth: None }),
        seq_tx_poll_max_blocks: Some(60),
        ..Default::default()
    };
    let storage = wallet::utils::fetch_persistent_storage_path().unwrap();
    let password = wallet::utils::read_password(&storage).unwrap();
    let w = wallet::WalletCore::new_update_chain(wallet::utils::fetch_config_path().unwrap(), storage, Some(overrides), &password).unwrap();
    let mut writer = Writer::new(w, lezreg_sdk::tx::lid(&reg));
    let x = writer.new_account().await.unwrap();
    writer.save_wallet().unwrap();
    let input = EntryInput {
        name: "e2e-sdk".into(),
        version: "0.0.1".into(),
        author_label: "e2e".into(),
        description: "registered before deploying: no program yet".into(),
        tags: vec!["e2e".into()],
        idl: ContentRef { cid: vec![1, 0x55, 0x12, 0x20, 1], sha256: [1; 32], size: 1, peer_hint: None },
        source: SourceRef { repo: "https://example.org/e2e".into(), commit: vec![2; 20], docker_root: ".".into(), archive: None },
        build: BuildConfig { schema: 1, builder: 1, binary: "e2e".into(), manifest_path: "Cargo.toml".into(), ..BuildConfig::default() },
        image_id: Some([3; 32]),
    };
    let opts = SendOpts { payer: Some(payer), ..SendOpts::default() };
    // Dry run first: simulates, sends nothing.
    let dry = writer.register_deployer(x, x, &input, &SendOpts { dry_run: true, ..opts.clone() }).await.unwrap();
    assert!(dry.hash.is_none() && dry.cycles.is_some());
    let reg_r = writer.register_deployer(x, x, &input, &opts).await.unwrap();
    assert!(reg_r.gas_limit >= reg_r.cycles.unwrap() * 3 / 2);
    let r = Registry::new(HttpRpc::new(&rpc), reg);
    let res = r.resolve(&tid(&x)).unwrap();
    assert_eq!(res.deployer.as_ref().unwrap().name, "e2e-sdk");
    assert_eq!(res.freshness(), Some(Freshness::NoProgram));
    writer.update(x, None, &PatchInput { version: Some("0.0.2".into()), ..PatchInput::default() }, &opts).await.unwrap();
    assert_eq!(r.resolve(&tid(&x)).unwrap().deployer.unwrap().revision, 2);
    assert!(r.release(&tid(&x), 2).unwrap().is_some(), "every update writes a release");
    writer.freeze(x, None, &opts).await.unwrap();
    assert!(r.resolve(&tid(&x)).unwrap().deployer.unwrap().is_frozen());
    let refused = writer.update(x, None, &PatchInput { version: Some("0.0.3".into()), ..PatchInput::default() }, &opts).await;
    assert!(matches!(refused, Err(lezreg_sdk::Error::Frozen(_))), "{refused:?}");
}
