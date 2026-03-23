# LiquiFact Contracts

Soroban smart contracts for **LiquiFact** — the global invoice liquidity network on Stellar.
This repo contains the **escrow** contract that holds investor funds for tokenized invoices until settlement.

Part of the LiquiFact stack: **frontend** (Next.js) | **backend** (Express) | **contracts** (this repo).

---

## Prerequisites

- **Rust** 1.70+ (stable)
- **Soroban CLI** (optional, for deployment): [Stellar Soroban docs](https://developers.stellar.org/docs/smart-contracts/getting-started/soroban-cli)

---

## Setup

```bash
git clone <this-repo-url>
cd liquifact-contracts
cargo build
cargo test
```

---

## Development

| Command                      | Description                    |
|------------------------------|--------------------------------|
| `cargo build`                | Build all contracts            |
| `cargo test`                 | Run unit tests                 |
| `cargo fmt`                  | Format code                    |
| `cargo fmt -- --check`       | Check formatting (CI)          |

---

## Project structure

```
liquifact-contracts/
├── Cargo.toml                  # Workspace definition
├── escrow/
│   ├── Cargo.toml              # Escrow contract crate
│   └── src/
│       ├── lib.rs              # Contract implementation + interface spec
│       └── test.rs             # Full test suite (16 tests)
└── .github/workflows/
    └── ci.yml                  # CI: fmt, build, test
```

---

## Escrow Contract — Interface Specification

### Overview

The escrow contract holds investor stablecoin funds for a single tokenized invoice.
Deploy **one contract instance per invoice**.

### Lifecycle state machine

```
[open] ──fund()──► [funded] ──settle()──► [settled]
  0                    1                      2
```

| Status | Name    | Meaning                                              |
|--------|---------|------------------------------------------------------|
| `0`    | open    | Accepting investor contributions                     |
| `1`    | funded  | Funding target met; SME can receive liquidity        |
| `2`    | settled | Buyer paid; investors may redeem principal + yield   |

### Data type: `InvoiceEscrow`

Stored in contract instance storage under key `"escrow"`.

| Field            | Type      | Description                                                        |
|------------------|-----------|--------------------------------------------------------------------|
| `invoice_id`     | `Symbol`  | Unique invoice ID (≤ 9 bytes, e.g. `INV-1023`)                    |
| `sme_address`    | `Address` | SME (seller) Stellar address                                       |
| `amount`         | `i128`    | Invoice face value in smallest stablecoin unit; immutable          |
| `funding_target` | `i128`    | Minimum investment to release funds; initialized to `amount`       |
| `funded_amount`  | `i128`    | Running total of investor contributions; starts at `0`             |
| `yield_bps`      | `i64`     | Annualized yield in basis points (800 = 8.00% p.a.)               |
| `maturity`       | `u64`     | Invoice maturity as ledger timestamp (Unix seconds)                |
| `status`         | `u32`     | Lifecycle status: `0` open · `1` funded · `2` settled             |

---

### Method: `init`

```rust
pub fn init(
    env: Env,
    invoice_id: Symbol,
    sme_address: Address,
    amount: i128,
    yield_bps: i64,
    maturity: u64,
) -> InvoiceEscrow
```

Creates and persists a new escrow. Must be called once before any other method.

**Parameters**

| Parameter     | Constraints                                      |
|---------------|--------------------------------------------------|
| `invoice_id`  | Non-empty Symbol; ≤ 9 bytes; unique per SME      |
| `sme_address` | Valid Stellar account or contract address        |
| `amount`      | > 0; face value in smallest stablecoin unit      |
| `yield_bps`   | ≥ 0; annualized yield in basis points            |
| `maturity`    | Ledger timestamp; should be > current ledger time|

**Returns** — `InvoiceEscrow` with `funded_amount = 0`, `status = 0`.

**Failure conditions**

| Condition              | Behaviour                                    |
|------------------------|----------------------------------------------|
| Called a second time   | Silently overwrites existing escrow state    |
| `amount <= 0`          | No runtime guard; caller's responsibility    |

**State transition** — `(none)` → `status = 0`

---

### Method: `get_escrow`

```rust
pub fn get_escrow(env: Env) -> InvoiceEscrow
```

Read-only view of current escrow state. Does not modify storage.

**Returns** — Clone of the stored `InvoiceEscrow`.

**Failure conditions**

| Condition           | Behaviour                          |
|---------------------|------------------------------------|
| `init` not called   | Panics: `"Escrow not initialized"` |

---

### Method: `fund`

```rust
pub fn fund(env: Env, _investor: Address, amount: i128) -> InvoiceEscrow
```

Records an investor contribution. Transitions to `status = 1` when
`funded_amount >= funding_target`.

> **Production note:** Must be called atomically with a SEP-41 token `transfer`
> from `investor` to the contract address. This version records accounting only.

**Parameters**

| Parameter   | Constraints                                  |
|-------------|----------------------------------------------|
| `_investor` | Investor's Stellar address (for audit trail) |
| `amount`    | > 0 recommended; partial funding is allowed  |

**Returns** — Updated `InvoiceEscrow`.

**Failure conditions**

| Condition                 | Behaviour                               |
|---------------------------|-----------------------------------------|
| `status != 0`             | Panics: `"Escrow not open for funding"` |
| `init` not called         | Panics: `"Escrow not initialized"`      |
| `funded_amount` overflows | Rust panics (debug) / wraps (release)   |

**State transitions**

- `status 0` → `status 0` while `funded_amount < funding_target`
- `status 0` → `status 1` when `funded_amount >= funding_target`

---

### Method: `settle`

```rust
pub fn settle(env: Env) -> InvoiceEscrow
```

Marks the escrow as settled (buyer has paid). Investors are entitled to
`principal + (principal × yield_bps / 10_000)` after this point.

> **Production note:** Should be callable only by an authorized oracle or
> multi-sig. This version trusts any caller.

**Returns** — Updated `InvoiceEscrow` with `status = 2`.

**Failure conditions**

| Condition         | Behaviour                                           |
|-------------------|-----------------------------------------------------|
| `status != 1`     | Panics: `"Escrow must be funded before settlement"` |
| `init` not called | Panics: `"Escrow not initialized"`                  |

**State transition** — `status 1` → `status 2`

---

### Yield calculation (off-chain reference)

```
investor_return = principal + floor(principal × yield_bps / 10_000)
```

Example: 10,000 USDC at 800 bps (8%) → return = 10,800 USDC.

---

## Security Notes

- **Token custody** — `fund()` does not move tokens. A production integration
  must pair each call with a SEP-41 `transfer` in the same transaction.
- **Settlement authorization** — `settle()` has no access control in this
  version. Add an `admin: Address` field and `admin.require_auth()` before
  deploying to mainnet.
- **Re-initialization** — Calling `init()` twice overwrites state. Guard with
  a storage existence check if re-initialization must be prevented.
- **Integer overflow** — `funded_amount` accumulates `i128` values. Overflow
  is unreachable in practice (max i128 ≈ 1.7 × 10³⁸) but release builds wrap
  silently; use `checked_add` for defense-in-depth.
- **Maturity enforcement** — The contract does not enforce `maturity` on-chain.
  Time-based guards should be added via `env.ledger().timestamp()` comparisons.

---

## Test coverage

16 tests covering the full lifecycle, all failure conditions, and edge cases.

| Category              | Tests |
|-----------------------|-------|
| Happy-path lifecycle  | 5     |
| Field integrity       | 3     |
| Edge cases            | 2     |
| Panic / failure paths | 6     |

Run with:

```bash
cargo test
```

---

## CI/CD

GitHub Actions runs on every push and pull request to `main`:

- **Format** — `cargo fmt --all -- --check`
- **Build** — `cargo build`
- **Tests** — `cargo test`

---

## Contributing

1. Fork the repo and clone your fork.
2. Create a branch from `main`: `git checkout -b feature/your-feature`.
3. Follow existing patterns in `escrow/src/lib.rs`.
4. Add or update tests in `escrow/src/test.rs`.
5. Format with `cargo fmt`.
6. Verify: `cargo fmt --all -- --check && cargo build && cargo test`.
7. Commit with clear messages (e.g. `feat(escrow): X`, `test(escrow): Y`).
8. Push and open a Pull Request to `main`.

---

## License

MIT (see root LiquiFact project for full license).
