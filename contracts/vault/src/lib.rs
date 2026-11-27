//! # SplitStreamVault
//!
//! The on-chain settlement half of splitstream-actions: a SEP-41 vault that
//! stores one Merkle root per contributor cycle and pays contributors who can
//! prove their `(address, amount)` leaf against it.
//!
//! Lifecycle:
//!
//! 1. `initialize(admin, oracle, token)` — once, stores the admin (dispute
//!    authority), the oracle (the GitHub Action's relay key) and the token.
//! 2. `deposit(from, amount)` — funds the vault.
//! 3. `post_cycle_root(cycle_id, root, total_amount)` — oracle-gated; the exact
//!    call `src/relay.ts` builds and submits. Cycle ids must increase, so a
//!    stale replayed relay can never overwrite a newer cycle.
//! 4. 24-hour challenge window — the admin may replace a bad root once, and
//!    only before any claim has been made.
//! 5. `credit_claim(contributor, cycle_id, amount, proof)` — verifies the Merkle
//!    proof against the stored root and credits a pull-payment balance.
//! 6. `withdraw(contributor)` — pays the credited balance out.
//!
//! The leaf format and sorted-pair tree are frozen and byte-identical to
//! `src/merkle.ts`; the Rust test suite pins the same cross-language golden
//! fixture (`test/fixtures/merkle.golden.json`) the TypeScript suite uses, so a
//! drift on either side fails CI.

#![no_std]
// Event names are part of the protocol surface (e.g. "cycle_root_replaced") and
// exceed `symbol_short!`'s 9-character limit, so they are built with full
// `Symbol`s. `Events::publish` is deprecated in favour of `#[contractevent]` but
// remains fully supported in SDK 29.
#![allow(deprecated)]
#[cfg(test)]
extern crate std;

mod claims;
mod errors;
mod merkle;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, token, Address, BytesN, Env, MuxedAddress, Symbol, Vec};

pub use crate::errors::SplitStreamError;
pub use crate::types::CycleInfo;

/// The SplitStream payout vault.
#[contract]
pub struct SplitStreamVault;

#[contractimpl]
impl SplitStreamVault {
    /// One-time initialization. Stores the admin, oracle (CI relay) and the
    /// SEP-41 token in instance storage.
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        token: Address,
    ) -> Result<(), SplitStreamError> {
        // Checked before `require_auth` so a second call reports the real
        // reason for the failure rather than an auth trap for a different key.
        if storage::is_initialized(&env) {
            return Err(SplitStreamError::AlreadyInitialized);
        }
        admin.require_auth();
        storage::set_admin(&env, &admin);
        storage::set_oracle(&env, &oracle);
        storage::set_token(&env, &token);
        storage::bump_instance(&env);

        env.events().publish(
            (
                Symbol::new(&env, "initialized"),
                admin.clone(),
                oracle.clone(),
                token.clone(),
            ),
            (),
        );
        Ok(())
    }

    /// Deposit pooled funds into the vault from `from` via the token contract.
    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), SplitStreamError> {
        let token = storage::get_token(&env)?;
        from.require_auth();
        storage::bump_instance(&env);
        if amount <= 0 {
            return Err(SplitStreamError::InvalidAmount);
        }

        let contract = env.current_contract_address();
        let to: MuxedAddress = (&contract).into();
        token::TokenClient::new(&env, &token).transfer(&from, &to, &amount);

        env.events()
            .publish((Symbol::new(&env, "deposit"), from.clone(), amount), ());
        Ok(())
    }

    /// Post a cycle's payout-manifest root (oracle-gated). One post per cycle;
    /// corrections go through `challenge_and_replace_root`.
    ///
    /// Cycle ids must strictly increase. The action derives the next id from its
    /// committed manifest audit trail, and this guard makes a replayed or
    /// out-of-order relay fail on-chain instead of silently rolling the
    /// treasury back to an older cycle.
    pub fn post_cycle_root(
        env: Env,
        cycle_id: u64,
        root: BytesN<32>,
        total_amount: i128,
    ) -> Result<(), SplitStreamError> {
        let oracle = storage::get_oracle(&env)?;
        oracle.require_auth();
        storage::bump_instance(&env);

        if storage::get_cycle_info(&env, cycle_id).is_some() {
            return Err(SplitStreamError::CycleAlreadyPosted);
        }
        if let Some(latest) = storage::get_latest_cycle(&env) {
            if cycle_id <= latest {
                return Err(SplitStreamError::CycleIdNotMonotonic);
            }
        }
        if total_amount <= 0 {
            return Err(SplitStreamError::InvalidAmount);
        }

        let info = types::CycleInfo {
            root: root.clone(),
            total_amount,
            posted_at: env.ledger().timestamp(),
            claims_started: false,
            replaced: false,
        };
        storage::set_cycle_info(&env, cycle_id, &info);
        storage::set_latest_cycle(&env, cycle_id);

        env.events().publish(
            (
                Symbol::new(&env, "cycle_posted"),
                cycle_id,
                root.clone(),
                total_amount,
            ),
            (),
        );
        Ok(())
    }

    /// Challenge and replace a cycle's root within the dispute window
    /// (admin-gated — deliberately not the oracle, so the automated relay
    /// cannot override its own dispute window).
    ///
    /// Fails once claims have started, once the 24h window has closed, or on a
    /// second replacement. The original `posted_at` is preserved: the window
    /// bounds total dispute time to 24h from the original post.
    pub fn challenge_and_replace_root(
        env: Env,
        cycle_id: u64,
        new_root: BytesN<32>,
        new_total_amount: i128,
    ) -> Result<(), SplitStreamError> {
        let admin = storage::get_admin(&env)?;
        admin.require_auth();
        storage::bump_instance(&env);

        let mut cycle =
            storage::get_cycle_info(&env, cycle_id).ok_or(SplitStreamError::CycleNotFound)?;

        if cycle.claims_started {
            return Err(SplitStreamError::ClaimsAlreadyStarted);
        }
        let elapsed = env.ledger().timestamp().saturating_sub(cycle.posted_at);
        if elapsed >= types::CHALLENGE_WINDOW_SECS {
            return Err(SplitStreamError::ChallengeWindowClosed);
        }
        if cycle.replaced {
            return Err(SplitStreamError::RootAlreadyReplaced);
        }
        if new_total_amount <= 0 {
            return Err(SplitStreamError::InvalidAmount);
        }

        cycle.root = new_root.clone();
        cycle.total_amount = new_total_amount;
        cycle.replaced = true;
        storage::set_cycle_info(&env, cycle_id, &cycle);

        env.events().publish(
            (
                Symbol::new(&env, "cycle_root_replaced"),
                cycle_id,
                new_root.clone(),
                new_total_amount,
            ),
            (),
        );
        Ok(())
    }

    /// Prove a `(contributor, amount)` entry against the current cycle root
    /// and credit the contributor's pull-payment balance.
    ///
    /// Enforces the challenge window: claims are rejected until 24h have
    /// elapsed since `posted_at`. The first successful claim sets
    /// `claims_started`, making the root final.
    pub fn credit_claim(
        env: Env,
        contributor: Address,
        cycle_id: u64,
        amount: i128,
        proof: Vec<BytesN<32>>,
    ) -> Result<(), SplitStreamError> {
        claims::credit_claim(&env, &contributor, cycle_id, amount, &proof)
    }

    /// Withdraw the contributor's full claimable balance.
    pub fn withdraw(env: Env, contributor: Address) -> Result<(), SplitStreamError> {
        claims::withdraw(&env, &contributor)
    }

    // ── View functions (read-only, no auth) ────────────────────────────────

    /// Challenge-window state for a cycle, if posted. Shape matches the
    /// `CycleInfo` map decoded by `parseCycleInfo` in `src/relay.ts`.
    pub fn get_cycle_info(env: Env, cycle_id: u64) -> Option<CycleInfo> {
        storage::get_cycle_info(&env, cycle_id)
    }

    /// Highest cycle id posted so far. Unlike the deployed full vault, this
    /// contract can enumerate its own head, so operators can cross-check the
    /// next cycle id against chain state.
    pub fn get_latest_cycle(env: Env) -> Option<u64> {
        storage::get_latest_cycle(&env)
    }

    /// Claimable pull-payment balance for a contributor.
    pub fn get_balance(env: Env, contributor: Address) -> i128 {
        storage::get_balance(&env, &contributor)
    }

    /// Whether a contributor has already claimed for a cycle.
    pub fn has_claimed(env: Env, cycle_id: u64, contributor: Address) -> bool {
        storage::get_cycle_claimed(&env, cycle_id, &contributor)
    }

    /// Sum of every contributor's outstanding pull-payment balance.
    pub fn get_total_credited(env: Env) -> i128 {
        storage::get_credited_total(&env)
    }
}
