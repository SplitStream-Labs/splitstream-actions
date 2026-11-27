# Change record — Soroban rebase and Wave readiness

**Why.** `splitstream-actions` was already Soroban-adjacent — it used
`@stellar/stellar-sdk`, Soroban RPC and signed `post_cycle_root` calls — but it
held no Soroban code of its own. The contract it drove lived in a sibling
repository, so what Drips reviewed was a thin TypeScript wrapper around an
artifact nobody could inspect in place. That is the gap this change closes: the
on-chain half now lives in this repository, with tests, a deployable wasm, a live
testnet instance and CI that builds both halves.

**The contract.** A Rust workspace at the root hosts `contracts/vault/`, a
`splitstream-vault` package on soroban-sdk 29.0.1. It exports eleven functions:
`initialize`, `deposit`, oracle-gated `post_cycle_root` (rejects re-posts and
enforces strictly increasing cycle ids), admin-gated
`challenge_and_replace_root` (one replacement, only inside the 24-hour window and
before any claim), `credit_claim` (Merkle proof, double-claim block, cycle total
cap, credits a pull-payment balance), and `withdraw` (zeroes the balance before
the transfer), plus the views `get_cycle_info`, `get_latest_cycle`, `get_balance`,
`has_claimed` and `get_total_credited`. `CycleInfo` keeps the exact five-field
shape that `parseCycleInfo` in `src/relay.ts` decodes, so the existing relay
works untouched; `get_latest_cycle` is the one new addition, letting a vault
report its own head. Error discriminants follow the sibling core contract for ABI
compatibility, and the Rust tests pin the same golden Merkle values as the
TypeScript suite from `test/fixtures/merkle.golden.json` — leaf
`e89c8b63…33c206`, 3-leaf root `42cb7efa…0663f94f`, 4-leaf root
`b9f5a8ae…36197b24` — so a drift in either implementation fails CI instead of
silently making every claim unclaimable. Fifteen tests cover the lifecycle,
authorisation, windows, caps and the golden fixture; `cargo fmt` and
`cargo clippy -D warnings` are clean.

**Build, CI and the merge unblock.** `.github/workflows/ci.yml` gained a
`contract` job that installs the Rust toolchain with `wasm32v1-none` and
stellar-cli v30.0.1, then runs fmt, tests, clippy and
`stellar contract build --package splitstream-vault` (the deployable artifact is
a different build path from `cargo test`, so it is exercised separately). Issue
#15 reported that the required status check never matched the job name; a push
to `main` showed the ruleset requires a check called `npm run check`, which is
exactly the name the existing job already reports, so it was left alone and the
reason is now recorded in a comment above it. `target/` and `test_snapshots/`
were added to `.gitignore`, and `Cargo.lock` is committed for reproducible
builds.

**Relay fixes.** Two real defects surfaced while running the gated testnet
suite. `relayCycleRoot` previously reported success as soon as a transaction came
back `SUCCESS`, which only proves some call executed, so `verifyPostedCycle(...)`
now reads `get_cycle_info(cycleId)` back and throws unless the stored root and
total match exactly — a missing cycle is also an error, catching a wrong
`vaultContract` — with four unit tests for match, root mismatch, total mismatch
and absence. Separately, `ledgerCloseTime()` handed the RPC's `ledgerCloseTime`
straight to `new Date`, but the RPC returns unix seconds as a string
(`"1795767464"`), which is an Invalid Date; it now validates, coerces and
multiplies by 1000, and accepts a numeric field for forward compatibility, with
four more tests. `@stellar/stellar-sdk` moved from a pinned 17.0.1 to `^17.4.0`
(same major) because the old pin could not parse the current testnet ledger
header.

**Evidence, docs and open work.** The bundled wasm is 12,101 bytes with 11
exports and is deployed and initialized on testnet at
`CBW35L2YQUIPK32XZKT2DJLXU3YNI7KRAFV5WWPR7VF6GUUEINI3ABDN` (wasm hash
`776755d6…bbc1ebc06f`), with deploy tx `031570ce…d26fae7` and initialize tx
`638a545e…d938208f8`; it is empty, holds no funds, and its throwaway
admin/oracle secret is not in the repo. The state-changing path was proven on a
separate throwaway instance (`CBG4TMFR…4UKH22`) that posted cycle `999999` and
verified the root and total by readback, leaving the recorded instance clean.
Verification run: 15 Rust tests, fmt and clippy clean, wasm build, `npm run
typecheck`, `npm test` (84 passed, 3 gated skips), `npm run check`, the live
read-only suite (20 passed, 1 skip) and the submit run. `README.md`,
`CONTRIBUTING.md`, `docs/developer-guide.md` and `docs/how-it-works.md` were
updated with the contract, its ABI, the build and deploy commands, the testnet
table and the readback, and the incorrect "this repo never holds funds" and
"the contract cannot enumerate cycles" claims were corrected. Seven scoped Wave
issues were opened with `complexity:` / `area:` / `drips-wave` labels (#27
events, #28 `get_cycle_status`, #29 Merkle property tests, #30 manifest proofs,
#31 on-chain head cross-check, #32 `verify:manifest`, #33 claim docs). Deliberately
not done: no live claim was executed, since `credit_claim` is correctly blocked
for 24 hours after a root is posted and the path is proven by Rust tests instead;
`.github/splitstream.yml` still points at the org treasury and its real oracle,
because the demo deployment must not be used there; the full-featured core vault
(fixed splits, vesting, sweeps) remains a separate deployment; a few README links
still use the old `Oyinkans0la12/...` paths; and the fee-bump bug in issue #10 is
left unfixed as well-scoped Wave work.
