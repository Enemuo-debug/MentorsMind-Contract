#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token, Address, Env, Symbol,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EscrowStatus {
    Active,
    Released,
    Disputed,
    Refunded,
    /// Dispute was resolved by admin arbitration via `resolve_dispute`.
    Resolved,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Escrow {
    pub id: u64,
    pub mentor: Address,
    pub learner: Address,
    pub amount: i128,
    pub session_id: Symbol,
    pub status: EscrowStatus,
    pub created_at: u64,
    pub token_address: Address,
    /// Platform fee deducted at release time (0 until released).
    pub platform_fee: i128,
    /// Amount actually sent to mentor after fee (0 until released).
    pub net_amount: i128,
    /// Unix timestamp (seconds) at which the session ends.
    pub session_end_time: u64,
    /// Seconds after `session_end_time` before auto-release may trigger.
    pub auto_release_delay: u64,
    /// Reason symbol provided when a dispute was opened (default: empty symbol).
    pub dispute_reason: Symbol,
    /// Unix timestamp (seconds) at which `resolve_dispute` was called (0 until resolved).
    pub resolved_at: u64,
}

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

const ESCROW_COUNT: Symbol = symbol_short!("ESC_CNT");
const ADMIN: Symbol = symbol_short!("ADMIN");
const TREASURY: Symbol = symbol_short!("TREASURY");
const FEE_BPS: Symbol = symbol_short!("FEE_BPS");
/// Default auto-release delay in seconds (configurable at init).
const AUTO_REL_DLY: Symbol = symbol_short!("AR_DELAY");
const SESSION_KEY: Symbol = symbol_short!("SESSION");

/// Maximum configurable fee: 10% = 1 000 basis points.
const MAX_FEE_BPS: u32 = 1_000;

/// Default auto-release delay: 72 hours in seconds.
const DEFAULT_AUTO_RELEASE_DELAY: u64 = 72 * 60 * 60;

// Approved token registry key prefix: ("APRV_TOK", address) → bool
const APPROVED_TOKEN_KEY: Symbol = symbol_short!("APRV_TOK");

// ---------------------------------------------------------------------------
// TTL constants (in ledgers; ~5 s/ledger → 1 000 000 ≈ 57 days)
// ---------------------------------------------------------------------------

const ESCROW_TTL_THRESHOLD: u32 = 500_000;
const ESCROW_TTL_BUMP: u32 = 1_000_000;

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    // -----------------------------------------------------------------------
    // Admin / initialization
    // -----------------------------------------------------------------------

    /// Initialize the contract with an admin, treasury, initial fee, approved
    /// tokens, and an optional auto-release delay.
    ///
    /// - `fee_bps`: platform fee in basis points (e.g. 500 = 5%). Must be ≤ 1 000 (10%).
    /// - `treasury`: address that receives the platform fee on every release.
    /// - `auto_release_delay_secs`: seconds after session end before funds
    ///   auto-release to the mentor. Pass `0` to use the default (72 hours).
    /// - Approved tokens must satisfy SEP-41 (XLM, USDC, PYUSD, …).
    ///
    /// Calling this a second time will panic — persistent storage ensures the
    /// `ADMIN` key survives ledger archival so the guard cannot be bypassed.
    pub fn initialize(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
        approved_tokens: soroban_sdk::Vec<Address>,
        auto_release_delay_secs: u64,
    ) {
        if env.storage().persistent().has(&ADMIN) {
            panic!("Already initialized");
        }

        if fee_bps > MAX_FEE_BPS {
            panic!("Fee exceeds maximum (1000 bps)");
        }

        env.storage().persistent().set(&ADMIN, &admin);
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        env.storage().persistent().set(&TREASURY, &treasury);
        env.storage()
            .persistent()
            .extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        env.storage().persistent().set(&FEE_BPS, &fee_bps);
        env.storage()
            .persistent()
            .extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        env.storage().persistent().set(&ESCROW_COUNT, &0u64);
        env.storage()
            .persistent()
            .extend_ttl(&ESCROW_COUNT, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // Store configurable auto-release delay; fall back to 72 hours if 0.
        let delay = if auto_release_delay_secs == 0 {
            DEFAULT_AUTO_RELEASE_DELAY
        } else {
            auto_release_delay_secs
        };
        env.storage().persistent().set(&AUTO_REL_DLY, &delay);
        env.storage()
            .persistent()
            .extend_ttl(&AUTO_REL_DLY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // Register each approved token
        for token_addr in approved_tokens.iter() {
            Self::_set_token_approved(&env, &token_addr, true);
        }
    }

    /// Update the platform fee — admin only, capped at 1 000 bps (10%).
    /// Update the fee basis points (admin only).
    /// 
    /// Auth: Only the admin can update fees.
    /// The admin address is retrieved from persistent storage.
    /// 
    /// Panics if:
    /// - Contract is not initialized
    /// - Caller is not the admin
    /// - Caller fails authorization check
    /// - New fee exceeds maximum (1000 bps = 10%)
    pub fn update_fee(env: Env, new_fee_bps: u32) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&ADMIN)
            .expect("Not initialized");
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        if new_fee_bps > MAX_FEE_BPS {
            panic!("Fee exceeds maximum (1000 bps)");
        }

        env.storage().persistent().set(&FEE_BPS, &new_fee_bps);
        env.storage()
            .persistent()
            .extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
    }

    /// Update the treasury address — admin only.
    /// 
    /// Auth: Only the admin can update the treasury address.
    /// The admin address is retrieved from persistent storage.
    /// 
    /// Panics if:
    /// - Contract is not initialized
    /// - Caller is not the admin
    /// - Caller fails authorization check
    pub fn update_treasury(env: Env, new_treasury: Address) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&ADMIN)
            .expect("Not initialized");
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        env.storage().persistent().set(&TREASURY, &new_treasury);
        env.storage()
            .persistent()
            .extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
    }

    /// Add or remove an approved token (admin only).
    /// 
    /// Auth: Only the admin can manage approved tokens.
    /// The admin address is retrieved from persistent storage.
    /// 
    /// Panics if:
    /// - Contract is not initialized
    /// - Caller is not the admin
    /// - Caller fails authorization check
    pub fn set_approved_token(env: Env, token_address: Address, approved: bool) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&ADMIN)
            .expect("Not initialized");
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        Self::_set_token_approved(&env, &token_address, approved);
    }

    // -----------------------------------------------------------------------
    // Escrow lifecycle
    // -----------------------------------------------------------------------

    /// Create a new escrow.
    ///
    /// Auth: Only the learner can create an escrow for themselves.
    /// The learner must provide valid authorization.
    ///
    /// Transfers `amount` tokens from `learner` to the contract.
    ///
    /// - `session_end_time`: unix timestamp (seconds) marking when the session
    ///   ends. After this plus the contract's `auto_release_delay`, anyone may
    ///   call `try_auto_release` to release funds to the mentor.
    ///
    /// Panics if:
    /// - `amount` ≤ 0
    /// - `token_address` is not on the approved list
    /// - learner's on-chain balance is insufficient
    /// - Caller is not the learner
    /// - Caller fails authorization check
    pub fn create_escrow(
        env: Env,
        mentor: Address,
        learner: Address,
        amount: i128,
        session_id: Symbol,
        token_address: Address,
        session_end_time: u64,
    ) -> u64 {
        // --- Validate amount ---
        if amount <= 0 {
            panic!("Amount must be greater than zero");
        }

        // --- Validate approved token ---
        if !Self::_is_token_approved(&env, &token_address) {
            panic!("Token not approved");
        }

        // --- Require learner authorization ---
        learner.require_auth();

        // --- Balance check (SEP-41: balance()) ---
        let token_client = token::Client::new(&env, &token_address);
        let learner_balance = token_client.balance(&learner);
        if learner_balance < amount {
            panic!("Insufficient token balance");
        }

        // --- Retrieve global auto-release delay ---
        let auto_release_delay: u64 = env
            .storage()
            .persistent()
            .get(&AUTO_REL_DLY)
            .unwrap_or(DEFAULT_AUTO_RELEASE_DELAY);
        env.storage()
            .persistent()
            .extend_ttl(&AUTO_REL_DLY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // --- Check session_id uniqueness ---
        let session_key = (SESSION_KEY, session_id.clone());
        if env.storage().persistent().has(&session_key) {
            panic!("Session ID already exists");
        }
        env.storage().persistent().set(&session_key, &true);
        env.storage().persistent().extend_ttl(&session_key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // --- Increment and persist escrow counter ---
        let mut count: u64 = env.storage().persistent().get(&ESCROW_COUNT).unwrap_or(0);
        count = count.checked_add(1).expect("Counter overflow");
        env.storage().persistent().set(&ESCROW_COUNT, &count);
        env.storage()
            .persistent()
            .extend_ttl(&ESCROW_COUNT, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // --- Transfer tokens from learner → contract ---
        token_client.transfer(&learner, &env.current_contract_address(), &amount);

        // --- Persist escrow ---
        let escrow = Escrow {
            id: count,
            mentor: mentor.clone(),
            learner: learner.clone(),
            amount,
            session_id: session_id.clone(),
            status: EscrowStatus::Active,
            created_at: env.ledger().timestamp(),
            token_address: token_address.clone(),
            platform_fee: 0,
            net_amount: 0,
            session_end_time,
            auto_release_delay,
            dispute_reason: symbol_short!(""),
            resolved_at: 0,
        };

        let key = (symbol_short!("ESCROW"), count);
        env.storage().persistent().set(&key, &escrow);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // --- Emit event (includes token_address and session_end_time) ---
        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("created"), count),
            (
                mentor,
                learner,
                amount,
                session_id,
                token_address,
                session_end_time,
            ),
        );

        count
    }

    /// Release funds to the mentor (called by learner or admin).
    ///
    /// Calculates the platform fee (`gross * fee_bps / 10_000`), transfers the
    /// fee to the treasury, and transfers the remainder to the mentor.
    /// Both amounts are stored on the escrow record and emitted in the event.
    /// Release funds to the mentor.
    /// 
    /// Auth: Only the learner or admin can release funds.
    /// The caller must provide valid authorization.
    /// 
    /// Panics if:
    /// - Escrow does not exist
    /// - Escrow is not in Active status  
    /// - Caller is not the learner or admin
    /// - Caller fails authorization check
    pub fn release_funds(env: Env, caller: Address, escrow_id: u64) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        let admin: Address = env
            .storage()
            .persistent()
            .get(&ADMIN)
            .expect("Admin not found");
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        // Auth check: caller must be learner OR admin
        caller.require_auth();
        if caller != escrow.learner && caller != admin {
            panic!("Caller not authorized");
        }

        Self::_do_release(&env, &mut escrow, &key);
    }
    pub fn release_partial(env: Env, caller: Address, escrow_id: u64, amount_to_release: i128) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage().persistent().extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env.storage().persistent().get(&key).expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        if amount_to_release <= 0 || amount_to_release > escrow.amount {
            panic!("Invalid release amount");
        }

        let admin: Address = env.storage().persistent().get(&ADMIN).expect("Admin not found");
        env.storage().persistent().extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        caller.require_auth();
        if caller != escrow.learner && caller != admin {
            panic!("Caller not authorized");
        }

        let fee_bps: u32 = env.storage().persistent().get(&FEE_BPS).unwrap_or(0u32);
        env.storage().persistent().extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let platform_fee: i128 = amount_to_release.checked_mul(fee_bps as i128).expect("Overflow").checked_div(10_000).expect("Division error");
        let net_amount: i128 = amount_to_release.checked_sub(platform_fee).expect("Underflow");

        let treasury: Address = env.storage().persistent().get(&TREASURY).expect("Treasury not found");
        env.storage().persistent().extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let token_client = soroban_sdk::token::Client::new(&env, &escrow.token_address);

        if platform_fee > 0 {
            token_client.transfer(&env.current_contract_address(), &treasury, &platform_fee);
        }

        token_client.transfer(&env.current_contract_address(), &escrow.mentor, &net_amount);

        escrow.amount = escrow.amount.checked_sub(amount_to_release).expect("Underflow");
        escrow.platform_fee = escrow.platform_fee.checked_add(platform_fee).expect("Overflow");
        escrow.net_amount = escrow.net_amount.checked_add(net_amount).expect("Overflow");

        if escrow.amount == 0 {
            escrow.status = EscrowStatus::Released;
        }

        env.storage().persistent().set(&key, &escrow);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("rel_part"), escrow.id),
            (escrow.mentor.clone(), amount_to_release, net_amount, platform_fee, escrow.token_address.clone(), escrow.amount),
        );
    }

    pub fn admin_release(env: Env, escrow_id: u64) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage().persistent().extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env.storage().persistent().get(&key).expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        let admin: Address = env.storage().persistent().get(&ADMIN).expect("Admin not found");
        env.storage().persistent().extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        env.events().publish((symbol_short!("Escrow"), symbol_short!("adm_rel"), escrow_id), (escrow_id, env.ledger().timestamp()));

        Self::_do_release(&env, &mut escrow, &key);
    }


    /// Permissionless auto-release.
    ///
    /// Anyone may call this once `env.ledger().timestamp() >=
    /// escrow.session_end_time + escrow.auto_release_delay` and the escrow is
    /// still `Active`. Funds are released to the mentor using the same fee
    /// logic as `release_funds`.
    ///
    /// Panics if:
    /// - Escrow does not exist.
    /// - Escrow status is not `Active`.
    /// - The auto-release window has not yet elapsed.
    pub fn try_auto_release(env: Env, escrow_id: u64) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        let now = env.ledger().timestamp();
        let release_after = escrow
            .session_end_time
            .checked_add(escrow.auto_release_delay)
            .expect("Timestamp overflow");

        if now < release_after {
            panic!("Auto-release window has not elapsed");
        }

        // Emit a dedicated `auto_released` event *before* the internal release
        // so listeners can distinguish this path from a manual release.
        env.events()
            .publish((symbol_short!("Escrow"), symbol_short!("auto_rel"), escrow_id), (escrow_id, now));

        Self::_do_release(&env, &mut escrow, &key);
    }
    pub fn release_partial(env: Env, caller: Address, escrow_id: u64, amount_to_release: i128) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage().persistent().extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env.storage().persistent().get(&key).expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        if amount_to_release <= 0 || amount_to_release > escrow.amount {
            panic!("Invalid release amount");
        }

        let admin: Address = env.storage().persistent().get(&ADMIN).expect("Admin not found");
        env.storage().persistent().extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        caller.require_auth();
        if caller != escrow.learner && caller != admin {
            panic!("Caller not authorized");
        }

        let fee_bps: u32 = env.storage().persistent().get(&FEE_BPS).unwrap_or(0u32);
        env.storage().persistent().extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let platform_fee: i128 = amount_to_release.checked_mul(fee_bps as i128).expect("Overflow").checked_div(10_000).expect("Division error");
        let net_amount: i128 = amount_to_release.checked_sub(platform_fee).expect("Underflow");

        let treasury: Address = env.storage().persistent().get(&TREASURY).expect("Treasury not found");
        env.storage().persistent().extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let token_client = soroban_sdk::token::Client::new(&env, &escrow.token_address);

        if platform_fee > 0 {
            token_client.transfer(&env.current_contract_address(), &treasury, &platform_fee);
        }

        token_client.transfer(&env.current_contract_address(), &escrow.mentor, &net_amount);

        escrow.amount = escrow.amount.checked_sub(amount_to_release).expect("Underflow");
        escrow.platform_fee = escrow.platform_fee.checked_add(platform_fee).expect("Overflow");
        escrow.net_amount = escrow.net_amount.checked_add(net_amount).expect("Overflow");

        if escrow.amount == 0 {
            escrow.status = EscrowStatus::Released;
        }

        env.storage().persistent().set(&key, &escrow);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("rel_part"), escrow.id),
            (escrow.mentor.clone(), amount_to_release, net_amount, platform_fee, escrow.token_address.clone(), escrow.amount),
        );
    }

    pub fn admin_release(env: Env, escrow_id: u64) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage().persistent().extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env.storage().persistent().get(&key).expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        let admin: Address = env.storage().persistent().get(&ADMIN).expect("Admin not found");
        env.storage().persistent().extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        env.events().publish((symbol_short!("Escrow"), symbol_short!("adm_rel"), escrow_id), (escrow_id, env.ledger().timestamp()));

        Self::_do_release(&env, &mut escrow, &key);
    }


    /// Open a dispute (called by mentor or learner).
    ///
    /// - `reason`: a short symbol describing the dispute (e.g. `symbol_short!("NO_SHOW")`).
    ///   Stored on the escrow for admin review.
    ///
    /// Panics if:
    /// - Escrow does not exist.
    /// - Escrow is not `Active`.
    /// - Caller is neither mentor nor learner.
    /// Dispute an active escrow.
    /// 
    /// Auth: Only the mentor or learner can dispute their escrow.
    /// The caller must provide valid authorization.
    /// 
    /// Panics if:
    /// - Escrow does not exist
    /// - Escrow is not in Active status
    /// - Caller is not the mentor or learner
    /// - Caller fails authorization check
    pub fn dispute(env: Env, caller: Address, escrow_id: u64, reason: Symbol) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Escrow not active");
        }

        // Auth check: caller must be mentor OR learner
        caller.require_auth();
        if caller != escrow.mentor && caller != escrow.learner {
            panic!("Caller not authorized to dispute");
        }

        escrow.status = EscrowStatus::Disputed;
        escrow.dispute_reason = reason.clone();
        env.storage().persistent().set(&key, &escrow);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("disp_opnd"), escrow_id),
            (escrow_id, caller, reason, escrow.token_address),
        );
    }

    /// Resolve a disputed escrow by splitting funds between mentor and learner.
    ///
    /// Admin only. Can only be called on `Disputed` escrows.
    ///
    /// - `mentor_pct`: percentage (0–100) of `escrow.amount` sent to the mentor.
    ///   The remainder (`100 - mentor_pct`) goes to the learner. No platform fee
    ///   is deducted — the full escrowed amount is split between the parties.
    ///
    /// Examples:
    /// - `mentor_pct = 100` → full amount to mentor, nothing to learner.
    /// - `mentor_pct = 50`  → half to each party.
    /// - `mentor_pct = 0`   → full amount to learner, nothing to mentor.
    ///
    /// Stores the mentor's share in `escrow.net_amount`, the learner's share
    /// in `escrow.platform_fee` (repurposed as learner_amount for the resolved
    /// state), and records `resolved_at` timestamp.
    ///
    /// Panics if:
    /// - Contract is not initialized.
    /// - Escrow does not exist.
    /// - Escrow status is not `Disputed`.
    /// - `mentor_pct` > 100.
    /// Resolve a disputed escrow by splitting funds (admin only).
    /// 
    /// Auth: Only the admin can resolve disputes.
    /// The admin address is retrieved from persistent storage.
    /// 
    /// Panics if:
    /// - Contract is not initialized
    /// - Caller is not the admin
    /// - Caller fails authorization check
    /// - Escrow does not exist
    /// - Escrow is not in Disputed status
    /// - mentor_pct is greater than 100
    pub fn resolve_dispute(env: Env, escrow_id: u64, release_to_mentor: bool) {
        // --- Admin auth ---
        let admin: Address = env.storage().persistent().get(&ADMIN).expect("Not initialized");
        env.storage().persistent().extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        // --- Load escrow ---
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage().persistent().extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env.storage().persistent().get(&key).expect("Escrow not found");

        if escrow.status != EscrowStatus::Disputed {
            panic!("Escrow is not in Disputed status");
        }

        let now = env.ledger().timestamp();

        if release_to_mentor {
            Self::_do_release(&env, &mut escrow, &key);
            escrow.status = EscrowStatus::Resolved;
            escrow.resolved_at = now;
            env.storage().persistent().set(&key, &escrow);

            env.events().publish(
                (symbol_short!("Escrow"), symbol_short!("disp_res"), escrow_id),
                (escrow_id, release_to_mentor, escrow.net_amount, 0i128, escrow.token_address.clone(), now),
            );
        } else {
            let token_client = soroban_sdk::token::Client::new(&env, &escrow.token_address);
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.learner,
                &escrow.amount,
            );
            escrow.status = EscrowStatus::Resolved;
            escrow.net_amount = 0;
            escrow.platform_fee = escrow.amount; // Repurposed for learner share
            escrow.resolved_at = now;
            env.storage().persistent().set(&key, &escrow);

            env.events().publish(
                (symbol_short!("Escrow"), symbol_short!("disp_res"), escrow_id),
                (escrow_id, release_to_mentor, 0i128, escrow.amount, escrow.token_address.clone(), now),
            );
        }
    }

    /// Refund tokens to the learner (admin only).
    ///
    /// Can be called on `Active` or `Disputed` escrows; panics if already
    /// `Released`, `Refunded`, or `Resolved`.
    /// Transfers `escrow.amount` tokens from contract → learner.
    /// Refund an escrow to the learner (admin only).
    /// 
    /// Auth: Only the admin can issue refunds.
    /// The admin address is retrieved from persistent storage.
    /// 
    /// Panics if:
    /// - Contract is not initialized
    /// - Caller is not the admin
    /// - Caller fails authorization check
    /// - Escrow does not exist
    /// - Escrow is already Released, Refunded, or Resolved
    pub fn refund(env: Env, escrow_id: u64) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&ADMIN)
            .expect("Admin not found");
        env.storage()
            .persistent()
            .extend_ttl(&ADMIN, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        admin.require_auth();

        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        if escrow.status == EscrowStatus::Released
            || escrow.status == EscrowStatus::Refunded
            || escrow.status == EscrowStatus::Resolved
        {
            panic!("Cannot refund");
        }

        // Transfer tokens: contract → learner
        let token_client = token::Client::new(&env, &escrow.token_address);
        token_client.transfer(
            &env.current_contract_address(),
            &escrow.learner,
            &escrow.amount,
        );

        escrow.status = EscrowStatus::Refunded;
        env.storage().persistent().set(&key, &escrow);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("refunded"), escrow_id),
            (escrow.learner.clone(), escrow.amount, escrow.token_address),
        );
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    pub fn get_escrow(env: Env, escrow_id: u64) -> Escrow {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        env.storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found")
    }

    pub fn get_escrow_count(env: Env) -> u64 {
        env.storage()
            .persistent()
            .extend_ttl(&ESCROW_COUNT, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        env.storage().persistent().get(&ESCROW_COUNT).unwrap_or(0)
    }

    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        env.storage().persistent().get(&FEE_BPS).unwrap_or(0)
    }

    pub fn get_treasury(env: Env) -> Address {
        env.storage()
            .persistent()
            .extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        env.storage()
            .persistent()
            .get(&TREASURY)
            .expect("Treasury not set")
    }

    pub fn get_auto_release_delay(env: Env) -> u64 {
        env.storage()
            .persistent()
            .extend_ttl(&AUTO_REL_DLY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
        env.storage()
            .persistent()
            .get(&AUTO_REL_DLY)
            .unwrap_or(DEFAULT_AUTO_RELEASE_DELAY)
    }

    pub fn is_token_approved(env: Env, token_address: Address) -> bool {
        Self::_is_token_approved(&env, &token_address)
    }

    /// Get all escrows for a specific mentor.
    ///
    /// Iterates through all escrows and returns those where the mentor matches.
    /// This is a query function with no authorization requirements.
    pub fn get_escrows_by_mentor(env: Env, mentor: Address) -> Vec<Escrow> {
        let count = env.storage().persistent().get(&ESCROW_COUNT).unwrap_or(0u64);
        let mut result = Vec::new(&env);

        for i in 1..=count {
            let key = (symbol_short!("ESCROW"), i);
            if let Some(escrow) = env.storage().persistent().get::<_, Escrow>(&key) {
                if escrow.mentor == mentor {
                    result.push_back(escrow);
                }
            }
        }

        result
    }

    /// Submit a review for a completed escrow (learner only).
    ///
    /// Records a review reason on the escrow after funds have been released.
    /// This is a lightweight operation that stores the review reason.
    /// In production, this would trigger a cross-contract call to the verification contract.
    pub fn submit_review(env: Env, caller: Address, escrow_id: u64, reason: Symbol) {
        let key = (symbol_short!("ESCROW"), escrow_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow not found");

        // Only learner can submit review
        caller.require_auth();
        if caller != escrow.learner {
            panic!("Only learner can submit review");
        }

        // Can only review released escrows
        if escrow.status != EscrowStatus::Released {
            panic!("Can only review released escrows");
        }

        // Store review reason in a separate key
        let review_key = (symbol_short!("REVIEW"), escrow_id);
        env.storage().persistent().set(&review_key, &reason);
        env.storage()
            .persistent()
            .extend_ttl(&review_key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("review"), escrow_id),
            (escrow_id, caller, reason, escrow.mentor),
        );
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Shared release logic used by both `release_funds` and `try_auto_release`.
    ///
    /// Computes the platform fee, transfers fee → treasury and net → mentor,
    /// then persists the updated escrow with `Released` status.
    fn _do_release(env: &Env, escrow: &mut Escrow, key: &(Symbol, u64)) {
        let release_amount = escrow.amount;
        let fee_bps: u32 = env.storage().persistent().get(&FEE_BPS).unwrap_or(0u32);
        env.storage()
            .persistent()
            .extend_ttl(&FEE_BPS, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let platform_fee: i128 = release_amount
            .checked_mul(fee_bps as i128)
            .expect("Overflow")
            .checked_div(10_000)
            .expect("Division error");
        let net_amount: i128 = release_amount
            .checked_sub(platform_fee)
            .expect("Underflow");

        let treasury: Address = env
            .storage()
            .persistent()
            .get(&TREASURY)
            .expect("Treasury not found");
        env.storage()
            .persistent()
            .extend_ttl(&TREASURY, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);

        let token_client = soroban_sdk::token::Client::new(env, &escrow.token_address);

        if platform_fee > 0 {
            token_client.transfer(&env.current_contract_address(), &treasury, &platform_fee);
        }

        token_client.transfer(&env.current_contract_address(), &escrow.mentor, &net_amount);

        escrow.status = EscrowStatus::Released;
        escrow.platform_fee = escrow.platform_fee.checked_add(platform_fee).expect("Overflow");
        escrow.net_amount = escrow.net_amount.checked_add(net_amount).expect("Overflow");
        escrow.amount = 0; // all remaining amount is released
        env.storage().persistent().set(key, escrow);

        env.events().publish(
            (symbol_short!("Escrow"), symbol_short!("released"), escrow.id),
            (
                escrow.mentor.clone(),
                release_amount,
                net_amount,
                platform_fee,
                escrow.token_address.clone(),
            ),
        );
    }

    fn _set_token_approved(env: &Env, token_address: &Address, approved: bool) {
        let key = (APPROVED_TOKEN_KEY, token_address.clone());
        env.storage().persistent().set(&key, &approved);
        env.storage()
            .persistent()
            .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
    }

    fn _is_token_approved(env: &Env, token_address: &Address) -> bool {
        let key = (APPROVED_TOKEN_KEY, token_address.clone());
        env.storage()
            .persistent()
            .get::<_, bool>(&key)
            .unwrap_or(false)
    }
}\n