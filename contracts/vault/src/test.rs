//! Test suites for SplitStreamVault.
//!
//! Covers the contract half of the splitstream-actions pipeline: oracle-gated
//! cycle posting, the 24h dispute window, Merkle-proof claims, pull-payment
//! withdrawals, and the cross-language Merkle fixture shared with
//! `test/fixtures/merkle.golden.json`.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, BytesN, Env, String, Vec,
};

use crate::errors::SplitStreamError;
use crate::merkle::{hash_pair, leaf_hash, verify_proof};
use crate::types::CHALLENGE_WINDOW_SECS;
use crate::{SplitStreamVault, SplitStreamVaultClient};

/// Test fixture: an initialized vault backed by a mintable SAC.
///
/// Clients are constructed on demand (they borrow the `Env`), so the fixture
/// only holds addresses.
struct TestEnv {
    env: Env,
    admin: Address,
    oracle: Address,
    token: Address,
    contract_id: Address,
}

fn setup() -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_contract.address();
    let contract_id = env.register(SplitStreamVault, ());
    SplitStreamVaultClient::new(&env, &contract_id).initialize(&admin, &oracle, &token);
    TestEnv {
        env,
        admin,
        oracle,
        token,
        contract_id,
    }
}

impl TestEnv {
    fn client(&self) -> SplitStreamVaultClient<'_> {
        SplitStreamVaultClient::new(&self.env, &self.contract_id)
    }

    fn sac(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token)
    }

    fn token_balance(&self, addr: &Address) -> i128 {
        token::TokenClient::new(&self.env, &self.token).balance(addr)
    }

    /// Fund the vault and return a contributor with a valid leaf.
    fn fund_vault(&self, funder: &Address, amount: i128) {
        self.sac().mint(funder, &amount);
        self.client().deposit(funder, &amount);
    }
}

/// Address from a G... strkey (the fixture uses real Stellar addresses).
fn addr(env: &Env, stellar: &str) -> Address {
    Address::from_string(&String::from_str(env, stellar))
}

/// Parse a 64-character hex string into a 32-byte hash.
fn hex32(env: &Env, hex: &str) -> BytesN<32> {
    let bytes = hex.as_bytes();
    assert_eq!(bytes.len(), 64, "hex32 expects 64 characters");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = (bytes[2 * i] as char).to_digit(16).expect("hex digit") as u8;
        let lo = (bytes[2 * i + 1] as char).to_digit(16).expect("hex digit") as u8;
        *byte = (hi << 4) | lo;
    }
    BytesN::from_array(env, &out)
}

/// Builds one tree level: sorted-pair hashing, odd node promoted unchanged.
fn next_level(env: &Env, level: &Vec<BytesN<32>>) -> Vec<BytesN<32>> {
    let mut next = Vec::new(env);
    let mut i = 0;
    while i < level.len() {
        let left = level.get(i).expect("left node");
        match level.get(i + 1) {
            Some(right) => next.push_back(hash_pair(env, &left, &right)),
            None => next.push_back(left),
        }
        i += 2;
    }
    next
}

/// Root over leaves already in manifest order (ascending pubkey bytes).
fn root_of(env: &Env, leaves: &Vec<BytesN<32>>) -> BytesN<32> {
    let mut level = leaves.clone();
    while level.len() > 1 {
        level = next_level(env, &level);
    }
    level.get(0).expect("non-empty tree")
}

/// Sibling proof for the leaf at `index`.
fn proof_for(env: &Env, leaves: &Vec<BytesN<32>>, mut index: u32) -> Vec<BytesN<32>> {
    let mut level = leaves.clone();
    let mut proof = Vec::new(env);
    while level.len() > 1 {
        let sibling = if index.is_multiple_of(2) {
            index + 1
        } else {
            index - 1
        };
        if let Some(node) = level.get(sibling) {
            proof.push_back(node);
        }
        level = next_level(env, &level);
        index /= 2;
    }
    proof
}

/// The three-leaf golden manifest, in ascending pubkey-byte order.
const GOLDEN3: [(&str, i128); 3] = [
    (
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        1_052_631_578,
    ),
    (
        "GCWSW5ZMOHGEBLUFKL7PZGPZKW3OMMDCAT4OMBFKWHXNIUGWGG43HQ7C",
        2_105_263_157,
    ),
    (
        "GC3BLRQMHJGDIBAVHA2WQOY6Q7CBLAZUNZPIR5NYAX6XTQZUSTVRHHPK",
        526_315_789,
    ),
];
const GOLDEN3_ROOT: &str = "42cb7efad0838a517c414ed4fe8fe58560bb0aa976c1fe0102317f890663f94f";

/// The four-leaf golden manifest, in ascending pubkey-byte order.
const GOLDEN4: [(&str, i128); 4] = [
    (
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        1_052_631_578,
    ),
    (
        "GBE4DJISDQVWPGUIKLPLAFIGYVWIBRGYITEWWLL5VIAWY3HCH55H2MLN",
        1_315_789_473,
    ),
    (
        "GCWSW5ZMOHGEBLUFKL7PZGPZKW3OMMDCAT4OMBFKWHXNIUGWGG43HQ7C",
        2_105_263_157,
    ),
    (
        "GC3BLRQMHJGDIBAVHA2WQOY6Q7CBLAZUNZPIR5NYAX6XTQZUSTVRHHPK",
        526_315_789,
    ),
];
const GOLDEN4_ROOT: &str = "b9f5a8aefae7496b17f79c1651af0db64b2ed21e1c07d0209d20143d36197b24";

fn leaves_of(env: &Env, entries: &[(&str, i128)]) -> Vec<BytesN<32>> {
    let mut leaves = Vec::new(env);
    for (stellar, amount) in entries {
        leaves.push_back(leaf_hash(env, &addr(env, stellar), *amount));
    }
    leaves
}

// ── Cross-language Merkle fixture ──────────────────────────────────────────

#[test]
fn leaf_hash_matches_the_typescript_golden_fixture() {
    let env = Env::default();
    let contributor = addr(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );
    let leaf = leaf_hash(&env, &contributor, 1_052_631_578);
    assert_eq!(
        leaf,
        hex32(
            &env,
            "e89c8b63a02273483139b1e3ebeda17e3354993ebf3a8da9400a52d79c33c206"
        )
    );
}

#[test]
fn tree_roots_match_the_typescript_golden_fixture() {
    let env = Env::default();
    assert_eq!(
        root_of(&env, &leaves_of(&env, &GOLDEN3)),
        hex32(&env, GOLDEN3_ROOT)
    );
    assert_eq!(
        root_of(&env, &leaves_of(&env, &GOLDEN4)),
        hex32(&env, GOLDEN4_ROOT)
    );
}

#[test]
fn verify_proof_accepts_a_valid_path_and_rejects_wrong_inputs() {
    let env = Env::default();
    let leaves = leaves_of(&env, &GOLDEN3);
    let root = root_of(&env, &leaves);

    for (index, (stellar, amount)) in GOLDEN3.iter().enumerate() {
        let leaf = leaf_hash(&env, &addr(&env, stellar), *amount);
        let proof = proof_for(&env, &leaves, index as u32);
        assert!(verify_proof(&env, &leaf, &proof, &root));
        // Same leaf, wrong amount → different leaf hash → proof must fail.
        let tampered = leaf_hash(&env, &addr(&env, stellar), *amount + 1);
        assert!(!verify_proof(&env, &tampered, &proof, &root));
    }
}

// ── Initialization ─────────────────────────────────────────────────────────

#[test]
fn initialize_sets_configuration_and_rejects_a_second_call() {
    let t = setup();
    let client = t.client();
    assert_eq!(client.get_total_credited(), 0);
    assert_eq!(client.get_latest_cycle(), None);
    assert_eq!(
        client.try_initialize(&t.admin, &t.oracle, &t.token),
        Err(Ok(SplitStreamError::AlreadyInitialized))
    );
}

// ── Cycle posting ──────────────────────────────────────────────────────────

#[test]
fn post_cycle_root_records_the_cycle_and_latest_id() {
    let t = setup();
    let client = t.client();
    let root = hex32(&t.env, GOLDEN3_ROOT);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&7, &root, &3_684_210_524);

    let info = client.get_cycle_info(&7).expect("cycle posted");
    assert_eq!(info.root, root);
    assert_eq!(info.total_amount, 3_684_210_524);
    assert_eq!(info.posted_at, 1_000);
    assert!(!info.claims_started);
    assert!(!info.replaced);
    assert_eq!(client.get_latest_cycle(), Some(7));
    assert_eq!(client.get_cycle_info(&8), None);
}

#[test]
fn post_cycle_root_is_single_use_and_monotonic() {
    let t = setup();
    let client = t.client();
    let root = hex32(&t.env, GOLDEN3_ROOT);

    client.post_cycle_root(&7, &root, &100);
    assert_eq!(
        client.try_post_cycle_root(&7, &root, &100),
        Err(Ok(SplitStreamError::CycleAlreadyPosted))
    );
    assert_eq!(
        client.try_post_cycle_root(&6, &root, &100),
        Err(Ok(SplitStreamError::CycleIdNotMonotonic))
    );
    // A newer cycle is accepted.
    client.post_cycle_root(&8, &root, &100);
    assert_eq!(client.get_latest_cycle(), Some(8));
}

#[test]
fn post_cycle_root_rejects_a_zero_total() {
    let t = setup();
    let root = hex32(&t.env, GOLDEN3_ROOT);
    assert_eq!(
        t.client().try_post_cycle_root(&1, &root, &0),
        Err(Ok(SplitStreamError::InvalidAmount))
    );
}

// ── Challenge window ───────────────────────────────────────────────────────

#[test]
fn challenge_window_replaces_a_root_once_before_claims() {
    let t = setup();
    let client = t.client();
    let original = hex32(&t.env, GOLDEN3_ROOT);
    let replacement = hex32(&t.env, GOLDEN4_ROOT);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &original, &100);

    t.env.ledger().set_timestamp(1_000 + 3_600);
    client.challenge_and_replace_root(&1, &replacement, &50);
    let info = client.get_cycle_info(&1).expect("cycle posted");
    assert_eq!(info.root, replacement);
    assert_eq!(info.total_amount, 50);
    assert_eq!(
        info.posted_at, 1_000,
        "posted_at must not restart the window"
    );
    assert!(info.replaced);

    assert_eq!(
        client.try_challenge_and_replace_root(&1, &original, &100),
        Err(Ok(SplitStreamError::RootAlreadyReplaced))
    );
}

#[test]
fn challenge_window_closes_after_24h() {
    let t = setup();
    let client = t.client();
    let root = hex32(&t.env, GOLDEN3_ROOT);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &100);

    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);
    assert_eq!(
        client.try_challenge_and_replace_root(&1, &root, &100),
        Err(Ok(SplitStreamError::ChallengeWindowClosed))
    );
}

#[test]
fn challenge_window_locks_once_claims_start() {
    let t = setup();
    let client = t.client();
    let leaves = leaves_of(&t.env, &GOLDEN3);
    let root = root_of(&t.env, &leaves);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &3_684_210_524);
    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);

    let contributor = addr(&t.env, GOLDEN3[0].0);
    let proof = proof_for(&t.env, &leaves, 0);
    client.credit_claim(&contributor, &1, &GOLDEN3[0].1, &proof);

    assert_eq!(
        client.try_challenge_and_replace_root(&1, &root, &100),
        Err(Ok(SplitStreamError::ClaimsAlreadyStarted))
    );
}

// ── Claims ─────────────────────────────────────────────────────────────────

#[test]
fn claim_is_rejected_before_the_challenge_window_elapses() {
    let t = setup();
    let client = t.client();
    let leaves = leaves_of(&t.env, &GOLDEN3);
    let root = root_of(&t.env, &leaves);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &3_684_210_524);
    t.env
        .ledger()
        .set_timestamp(1_000 + CHALLENGE_WINDOW_SECS - 1);

    let contributor = addr(&t.env, GOLDEN3[0].0);
    let proof = proof_for(&t.env, &leaves, 0);
    assert_eq!(
        client.try_credit_claim(&contributor, &1, &GOLDEN3[0].1, &proof),
        Err(Ok(SplitStreamError::ClaimsNotYetOpen))
    );
}

#[test]
fn claim_with_a_valid_proof_credits_a_pull_payment_balance() {
    let t = setup();
    let client = t.client();
    let leaves = leaves_of(&t.env, &GOLDEN3);
    let root = root_of(&t.env, &leaves);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &3_684_210_524);
    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);

    let contributor = addr(&t.env, GOLDEN3[1].0);
    let proof = proof_for(&t.env, &leaves, 1);
    client.credit_claim(&contributor, &1, &GOLDEN3[1].1, &proof);

    assert_eq!(client.get_balance(&contributor), GOLDEN3[1].1);
    assert_eq!(client.get_total_credited(), GOLDEN3[1].1);
    assert!(client.has_claimed(&1, &contributor));
    let info = client.get_cycle_info(&1).expect("cycle posted");
    assert!(info.claims_started);
}

#[test]
fn claim_rejects_a_wrong_amount_double_claim_and_unknown_cycle() {
    let t = setup();
    let client = t.client();
    let leaves = leaves_of(&t.env, &GOLDEN3);
    let root = root_of(&t.env, &leaves);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &3_684_210_524);
    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);

    let contributor = addr(&t.env, GOLDEN3[2].0);
    let proof = proof_for(&t.env, &leaves, 2);

    assert_eq!(
        client.try_credit_claim(&contributor, &1, &(GOLDEN3[2].1 + 1), &proof),
        Err(Ok(SplitStreamError::InvalidProof))
    );
    assert_eq!(
        client.try_credit_claim(&contributor, &2, &GOLDEN3[2].1, &proof),
        Err(Ok(SplitStreamError::CycleNotFound))
    );

    client.credit_claim(&contributor, &1, &GOLDEN3[2].1, &proof);
    assert_eq!(
        client.try_credit_claim(&contributor, &1, &GOLDEN3[2].1, &proof),
        Err(Ok(SplitStreamError::AlreadyClaimed))
    );
}

#[test]
fn claim_can_never_exceed_the_posted_cycle_total() {
    let t = setup();
    let client = t.client();
    let leaves = leaves_of(&t.env, &GOLDEN3);
    let root = root_of(&t.env, &leaves);

    // Deliberately under-fund the cycle: total_amount < the leaves' sum.
    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &1_000);
    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);

    let contributor = addr(&t.env, GOLDEN3[0].0);
    let proof = proof_for(&t.env, &leaves, 0);
    assert_eq!(
        client.try_credit_claim(&contributor, &1, &GOLDEN3[0].1, &proof),
        Err(Ok(SplitStreamError::CycleTotalExceeded))
    );
}

// ── Withdrawals ────────────────────────────────────────────────────────────

#[test]
fn withdraw_pays_the_credited_balance_and_zeroes_it() {
    let t = setup();
    let client = t.client();
    let funder = Address::generate(&t.env);
    // A generated account rather than a fixture G... key: the SAC has no
    // trustline for stand-in strkey addresses, so a real payout needs a real
    // test account. A single-leaf manifest needs an empty proof.
    let contributor = Address::generate(&t.env);
    let amount = 1_052_631_578i128;
    let mut leaves = Vec::new(&t.env);
    leaves.push_back(leaf_hash(&t.env, &contributor, amount));
    let root = root_of(&t.env, &leaves);

    t.fund_vault(&funder, amount);
    assert_eq!(t.token_balance(&t.contract_id), amount);

    t.env.ledger().set_timestamp(1_000);
    client.post_cycle_root(&1, &root, &amount);
    t.env.ledger().set_timestamp(1_000 + CHALLENGE_WINDOW_SECS);

    let proof = Vec::new(&t.env);
    client.credit_claim(&contributor, &1, &amount, &proof);

    client.withdraw(&contributor);
    assert_eq!(t.token_balance(&contributor), amount);
    assert_eq!(client.get_balance(&contributor), 0);
    assert_eq!(client.get_total_credited(), 0);

    assert_eq!(
        client.try_withdraw(&contributor),
        Err(Ok(SplitStreamError::InsufficientBalance))
    );
}
