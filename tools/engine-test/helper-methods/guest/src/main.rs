//! Forwarder: sends exactly the sync messages its root message lists, with the PDA seeds it is
//! told to attach. As an "attacker" it relays registry messages inside a transaction someone
//! else signed (engine test 3); as an "owner program" its seeds authorize PDA headers it owns
//! (engine test 9).
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Send {
    pub to_account: [u8; 32],
    pub to_program: [u8; 32],
    pub data: Vec<u8>,
    pub pda_seeds: Vec<[u8; 32]>,
}

fn handle(_input: &ReceiveInput, sends: Vec<Send>) -> Response {
    let mut r = Response::keep_state();
    for s in sends {
        r = r.send(SyncMessage {
            to: Actor::new(AccountId::new(s.to_account), AccountId::new(s.to_program)),
            message: s.data,
            pda_seeds: s.pda_seeds.into_iter().map(PdaSeed::new).collect(),
        });
    }
    r
}

lee_core::actor_entrypoint!(handle);
