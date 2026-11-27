//! Contract errors for `SplitStreamVault`.
//!
//! Discriminants that overlap with the full splitstream-core vault keep the
//! same numbers, so clients can share one error table across both deployments.

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum SplitStreamError {
    /// The contract has not been initialized.
    NotInitialized = 1,
    /// The contract has already been initialized.
    AlreadyInitialized = 2,
    /// A root has already been posted for this cycle.
    CycleAlreadyPosted = 4,
    /// No cycle exists for this id.
    CycleNotFound = 5,
    /// The contributor has already claimed for this cycle.
    AlreadyClaimed = 6,
    /// The submitted Merkle proof does not match the stored root.
    InvalidProof = 7,
    /// No claimable balance to withdraw.
    InsufficientBalance = 8,
    /// The amount is not valid for this operation.
    InvalidAmount = 12,
    /// The challenge window has not elapsed; claims are not yet open.
    ClaimsNotYetOpen = 13,
    /// The challenge window has closed; the root can no longer be replaced.
    ChallengeWindowClosed = 14,
    /// The root for this cycle has already been replaced once.
    RootAlreadyReplaced = 15,
    /// Claims have already started against this cycle's root.
    ClaimsAlreadyStarted = 16,
    /// Claims for this cycle would exceed the posted `total_amount`.
    CycleTotalExceeded = 19,
    /// Cycle ids must increase; this id is older than the latest posted cycle.
    CycleIdNotMonotonic = 20,
}
