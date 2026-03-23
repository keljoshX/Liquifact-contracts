//! # LiquiFact Escrow Contract
//!
//! A Soroban smart contract on Stellar that holds investor funds for a
//! tokenized invoice until settlement.
//!
//! ## Lifecycle
//!
//! ```text
//! [open] --fund()--> [funded] --settle()--> [settled]
//!   0                   1                      2
//! ```
//!
//! 1. **open (0)** — Escrow is initialized and accepting investor funds.
//! 2. **funded (1)** — `funded_amount >= funding_target`; SME can receive liquidity.
//! 3. **settled (2)** — Buyer has paid; investors receive principal + yield.
//!
//! ## Security Assumptions
//!
//! - In production, `fund()` must be preceded by an authorized token transfer
//!   (e.g. SEP-41 token `transfer` call). The current implementation records
//!   the accounting only; token custody is handled by the calling layer.
//! - `settle()` must be gated by an authorized oracle or buyer confirmation in
//!   production. The current implementation trusts the caller.
//! - All arithmetic uses `i128` to prevent overflow on large invoice amounts.
//! - Yield calculation (`yield_bps`) is stored as basis points (1 bps = 0.01%).
//!   Maximum representable yield: `i64::MAX` bps — well beyond any realistic rate.
//!
//! ## External Integrator Notes
//!
//! - Storage key: `"escrow"` (instance storage, single escrow per contract instance).
//! - Deploy one contract instance per invoice.
//! - All monetary values are in the smallest unit of the chosen stablecoin
//!   (e.g. stroops for XLM, or 7-decimal units for USDC on Stellar).
//! - `maturity` is a ledger timestamp (`u64`). Enforcement of maturity windows
//!   is the responsibility of the calling layer in this version.

use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, Symbol};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Full state of a single invoice escrow.
///
/// Stored in contract instance storage under the key `"escrow"`.
/// One `InvoiceEscrow` exists per deployed contract instance.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvoiceEscrow {
    /// Unique invoice identifier supplied by the originator (e.g. `INV-1023`).
    /// Maximum length: 9 bytes (Soroban `Symbol` constraint).
    pub invoice_id: Symbol,

    /// Stellar address of the SME (seller) that will receive liquidity once
    /// the funding target is met.
    pub sme_address: Address,

    /// Face value of the invoice in the smallest stablecoin unit.
    /// Must be > 0. Set once at `init`; immutable thereafter.
    pub amount: i128,

    /// Minimum total investment required before the SME can be paid.
    /// Initialized to `amount`; immutable after `init`.
    pub funding_target: i128,

    /// Running total of all investor contributions recorded via `fund()`.
    /// Starts at 0; monotonically increases; never exceeds `i128::MAX`.
    pub funded_amount: i128,

    /// Annualized yield in basis points (1 bps = 0.01%).
    /// Example: `800` = 8.00% p.a.
    /// Used by the settlement layer to compute investor returns.
    pub yield_bps: i64,

    /// Ledger timestamp at which the invoice matures (buyer payment due).
    /// Expressed as seconds since the Unix epoch (Stellar ledger `close_time`).
    pub maturity: u64,

    /// Escrow lifecycle status:
    /// - `0` — open: accepting investor funds
    /// - `1` — funded: funding target met; awaiting buyer payment
    /// - `2` — settled: buyer paid; investors may redeem principal + yield
    pub status: u32,
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

/// LiquiFact escrow contract entry point.
#[contract]
pub struct LiquifactEscrow;

#[contractimpl]
impl LiquifactEscrow {
    // -----------------------------------------------------------------------
    // init
    // -----------------------------------------------------------------------

    /// Initialize a new invoice escrow.
    ///
    /// Creates and persists an [`InvoiceEscrow`] in instance storage.
    /// Must be called exactly once per contract instance before any other
    /// method.
    ///
    /// # Parameters
    ///
    /// | Parameter    | Type      | Constraints                          |
    /// |--------------|-----------|--------------------------------------|
    /// | `invoice_id` | `Symbol`  | Non-empty; ≤ 9 bytes; unique per SME |
    /// | `sme_address`| `Address` | Valid Stellar account or contract     |
    /// | `amount`     | `i128`    | > 0; face value in smallest unit     |
    /// | `yield_bps`  | `i64`     | ≥ 0; annualized yield in basis points|
    /// | `maturity`   | `u64`     | > current ledger time (not enforced) |
    ///
    /// # Returns
    ///
    /// The newly created [`InvoiceEscrow`] with:
    /// - `funded_amount = 0`
    /// - `funding_target = amount`
    /// - `status = 0` (open)
    ///
    /// # Failure Conditions
    ///
    /// | Condition                        | Behaviour                        |
    /// |----------------------------------|----------------------------------|
    /// | Called a second time             | Overwrites existing escrow state |
    /// | `amount <= 0`                    | No runtime check; caller's responsibility |
    ///
    /// # State Transition
    ///
    /// `(none)` → `status = 0` (open)
    pub fn init(
        env: Env,
        invoice_id: Symbol,
        sme_address: Address,
        amount: i128,
        yield_bps: i64,
        maturity: u64,
    ) -> InvoiceEscrow {
        let escrow = InvoiceEscrow {
            invoice_id: invoice_id.clone(),
            sme_address: sme_address.clone(),
            amount,
            funding_target: amount,
            funded_amount: 0,
            yield_bps,
            maturity,
            status: 0,
        };
        env.storage()
            .instance()
            .set(&symbol_short!("escrow"), &escrow);
        escrow
    }

    // -----------------------------------------------------------------------
    // get_escrow
    // -----------------------------------------------------------------------

    /// Retrieve the current escrow state.
    ///
    /// Read-only; does not modify storage.
    ///
    /// # Returns
    ///
    /// A clone of the stored [`InvoiceEscrow`].
    ///
    /// # Failure Conditions
    ///
    /// | Condition              | Behaviour                  |
    /// |------------------------|----------------------------|
    /// | `init` not yet called  | Panics: "Escrow not initialized" |
    pub fn get_escrow(env: Env) -> InvoiceEscrow {
        env.storage()
            .instance()
            .get(&symbol_short!("escrow"))
            .unwrap_or_else(|| panic!("Escrow not initialized"))
    }

    // -----------------------------------------------------------------------
    // fund
    // -----------------------------------------------------------------------

    /// Record an investor funding contribution.
    ///
    /// Adds `amount` to `funded_amount`. If the running total meets or exceeds
    /// `funding_target`, the escrow transitions to `status = 1` (funded).
    ///
    /// > **Production note:** In a live deployment this method must be called
    /// > atomically with a SEP-41 token `transfer` from `investor` to the
    /// > contract address. The current implementation records accounting only.
    ///
    /// # Parameters
    ///
    /// | Parameter   | Type      | Constraints                        |
    /// |-------------|-----------|------------------------------------|
    /// | `_investor` | `Address` | Investor's Stellar address (logged) |
    /// | `amount`    | `i128`    | > 0; partial funding is allowed    |
    ///
    /// # Returns
    ///
    /// Updated [`InvoiceEscrow`] after applying the contribution.
    ///
    /// # Failure Conditions
    ///
    /// | Condition                  | Behaviour                              |
    /// |----------------------------|----------------------------------------|
    /// | `status != 0`              | Panics: "Escrow not open for funding"  |
    /// | `init` not called          | Panics: "Escrow not initialized"       |
    /// | `amount <= 0`              | No runtime check; caller's responsibility |
    /// | `funded_amount` overflows  | Rust panics on debug; wraps on release |
    ///
    /// # State Transition
    ///
    /// - `status 0` → `status 0` while `funded_amount < funding_target`
    /// - `status 0` → `status 1` when `funded_amount >= funding_target`
    pub fn fund(env: Env, _investor: Address, amount: i128) -> InvoiceEscrow {
        let mut escrow = Self::get_escrow(env.clone());
        assert!(escrow.status == 0, "Escrow not open for funding");
        escrow.funded_amount += amount;
        if escrow.funded_amount >= escrow.funding_target {
            escrow.status = 1;
        }
        env.storage()
            .instance()
            .set(&symbol_short!("escrow"), &escrow);
        escrow
    }

    // -----------------------------------------------------------------------
    // settle
    // -----------------------------------------------------------------------

    /// Mark the escrow as settled (buyer has paid the invoice).
    ///
    /// Transitions status to `2` (settled). After this point investors are
    /// entitled to redeem `principal + (principal * yield_bps / 10_000)`.
    ///
    /// > **Production note:** This method should be callable only by an
    /// > authorized oracle or multi-sig in a live deployment. The current
    /// > implementation trusts any caller.
    ///
    /// # Returns
    ///
    /// Updated [`InvoiceEscrow`] with `status = 2`.
    ///
    /// # Failure Conditions
    ///
    /// | Condition                  | Behaviour                                      |
    /// |----------------------------|------------------------------------------------|
    /// | `status != 1`              | Panics: "Escrow must be funded before settlement" |
    /// | `init` not called          | Panics: "Escrow not initialized"               |
    ///
    /// # State Transition
    ///
    /// `status 1` (funded) → `status 2` (settled)
    pub fn settle(env: Env) -> InvoiceEscrow {
        let mut escrow = Self::get_escrow(env.clone());
        assert!(
            escrow.status == 1,
            "Escrow must be funded before settlement"
        );
        escrow.status = 2;
        env.storage()
            .instance()
            .set(&symbol_short!("escrow"), &escrow);
        escrow
    }
}

#[cfg(test)]
mod test;
