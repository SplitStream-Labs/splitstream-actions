//! Merkle leaf hashing and proof verification for cycle payout manifests.
//!
//! The leaf format is FROZEN — it must match `src/merkle.ts` byte-for-byte, or
//! on-chain verification will reject every claim the action relays.
//!
//! ```text
//! leaf = sha256( xdr_encode(ScVal(Address(contributor))) || xdr_encode(ScVal(i128(amount))) )
//! ```
//!
//! Tree construction is the sorted-pair convention: at every level the two
//! 32-byte hashes are concatenated in ascending byte order before hashing, so a
//! proof is valid whichever side of a pair the proven leaf sits on, and an odd
//! node is promoted to the next level unchanged.

use soroban_sdk::{xdr::ToXdr, Address, Bytes, BytesN, Env, Vec};

/// Hash of a `(contributor, amount)` manifest entry.
pub fn leaf_hash(env: &Env, contributor: &Address, amount: i128) -> BytesN<32> {
    let mut bytes = Bytes::new(env);
    bytes.append(&contributor.to_xdr(env));
    bytes.append(&amount.to_xdr(env));
    env.crypto().sha256(&bytes).to_bytes()
}

/// Verify a Merkle proof for `leaf` against `root`.
///
/// The proof is the sequence of siblings from the leaf's level up to the root.
pub fn verify_proof(
    env: &Env,
    leaf: &BytesN<32>,
    proof: &Vec<BytesN<32>>,
    root: &BytesN<32>,
) -> bool {
    let mut hash = leaf.clone();
    for sibling in proof.iter() {
        hash = hash_pair(env, &hash, &sibling);
    }
    hash == *root
}

/// Hash of a sorted pair: concatenate the two hashes in ascending byte order
/// and SHA-256 the result. `pub(crate)` so tests can build trees with the exact
/// same convention as the verifier.
pub(crate) fn hash_pair(env: &Env, a: &BytesN<32>, b: &BytesN<32>) -> BytesN<32> {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut bytes = Bytes::new(env);
    bytes.append(&Bytes::from(first));
    bytes.append(&Bytes::from(second));
    env.crypto().sha256(&bytes).to_bytes()
}
