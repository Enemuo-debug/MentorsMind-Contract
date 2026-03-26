#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Symbol, IntoVal, Vec,
};
use soroban_sdk::token::TokenInterface;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidSchedule = 4,
    NothingToClaim = 5,
    ScheduleNotFound = 6,
    InsufficientBalance = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSchedule {
    pub beneficiary: Address,
    pub total: i128,
    pub claimed: i128,
    pub cliff_end: u64,
    pub vesting_end: u64,
    pub start: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleCreatedEventData {
    pub schedule_id: u32,
    pub beneficiary: Address,
    pub total_amount: i128,
    pub cliff_end: u64,
    pub vesting_end: u64,
    pub start: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokensClaimedEventData {
    pub schedule_id: u32,
    pub beneficiary: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleRevokedEventData {
    pub schedule_id: u32,
    pub beneficiary: Address,
    pub refunded_amount: i128,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    NextScheduleId,
    Schedule(u32),
    BeneficiarySchedules(Address),
    Balance(Address),
    TotalSupply,
}

#[contract]
pub struct VestingContract;

#[contractimpl]
impl VestingContract {
    /// Initialize the vesting contract with admin and token addresses.
    pub fn initialize(env: Env, admin: Address, token: Address) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("Already initialized");
        }
        
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Token, &token);
        env.storage().persistent().set(&DataKey::NextScheduleId, &0u32);
    }

    /// Create a new vesting schedule.
    /// Returns the schedule ID.
    pub fn create_schedule(
        env: Env,
        beneficiary: Address,
        total_amount: i128,
        cliff_seconds: u64,
        vesting_seconds: u64,
        start: u64,
    ) -> u32 {
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).expect("Not initialized");
        admin.require_auth();

        if total_amount <= 0 {
            panic!("Total amount must be positive");
        }

        if vesting_seconds == 0 {
            panic!("Vesting duration must be positive");
        }

        if cliff_seconds > vesting_seconds {
            panic!("Cliff cannot be longer than total vesting period");
        }

        let current_time = env.ledger().timestamp();
        let schedule_start = if start == 0 { current_time } else { start };
        
        let schedule = VestingSchedule {
            beneficiary: beneficiary.clone(),
            total: total_amount,
            claimed: 0,
            cliff_end: schedule_start + cliff_seconds,
            vesting_end: schedule_start + vesting_seconds,
            start: schedule_start,
        };

        let next_id: u32 = env.storage().persistent().get(&DataKey::NextScheduleId).unwrap_or(0);
        let schedule_id = next_id + 1;
        
        env.storage().persistent().set(&DataKey::Schedule(schedule_id), &schedule);
        env.storage().persistent().set(&DataKey::NextScheduleId, &schedule_id);

        // Add to beneficiary's schedule list
        let mut schedules: Vec<u32> = env.storage().persistent()
            .get(&DataKey::BeneficiarySchedules(beneficiary.clone()))
            .unwrap_or(Vec::new(&env));
        schedules.push_back(schedule_id);
        env.storage().persistent().set(&DataKey::BeneficiarySchedules(beneficiary), &schedules);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "VestingContract"), Symbol::new(&env, "ScheduleCreated")),
            ScheduleCreatedEventData {
                schedule_id,
                beneficiary: schedule.beneficiary.clone(),
                total_amount: schedule.total,
                cliff_end: schedule.cliff_end,
                vesting_end: schedule.vesting_end,
                start: schedule.start,
            },
        );

        schedule_id
    }

    /// Calculate the claimable amount for a given schedule.
    pub fn claimable_amount(env: Env, schedule_id: u32) -> i128 {
        let schedule: VestingSchedule = env.storage().persistent()
            .get(&DataKey::Schedule(schedule_id))
            .expect("Schedule not found");

        let current_time = env.ledger().timestamp();
        
        if current_time < schedule.cliff_end {
            return 0;
        }

        if current_time >= schedule.vesting_end {
            return schedule.total - schedule.claimed;
        }

        // Linear vesting between cliff and end
        let vested_period = current_time - schedule.start;
        let total_period = schedule.vesting_end - schedule.start;
        let vested_amount = (schedule.total * vested_period as i128) / total_period as i128;
        
        vested_amount - schedule.claimed
    }

    /// Claim available tokens from a vesting schedule.
    /// Only the beneficiary can call this.
    pub fn claim(env: Env, schedule_id: u32) {
        let mut schedule: VestingSchedule = env.storage().persistent()
            .get(&DataKey::Schedule(schedule_id))
            .expect("Schedule not found");

        schedule.beneficiary.require_auth();

        let claimable = Self::claimable_amount(env.clone(), schedule_id);
        
        if claimable <= 0 {
            panic!("Nothing to claim");
        }

        let token: Address = env.storage().persistent().get(&DataKey::Token).expect("Not initialized");
        let token_client = TokenInterface::new(&env, &token);
        
        // Check contract has enough tokens
        let contract_balance = token_client.balance(&env.current_contract_address());
        if contract_balance < claimable {
            panic!("Insufficient balance in vesting contract");
        }

        // Update claimed amount
        schedule.claimed += claimable;
        env.storage().persistent().set(&DataKey::Schedule(schedule_id), &schedule);

        // Transfer tokens to beneficiary
        token_client.transfer(&env.current_contract_address(), &schedule.beneficiary, &claimable);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "VestingContract"), Symbol::new(&env, "TokensClaimed")),
            TokensClaimedEventData {
                schedule_id,
                beneficiary: schedule.beneficiary.clone(),
                amount: claimable,
            },
        );
    }

    /// Revoke a vesting schedule and return unvested tokens to treasury.
    /// Only admin can call this.
    pub fn revoke(env: Env, schedule_id: u32) {
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).expect("Not initialized");
        admin.require_auth();

        let mut schedule: VestingSchedule = env.storage().persistent()
            .get(&DataKey::Schedule(schedule_id))
            .expect("Schedule not found");

        let current_time = env.ledger().timestamp();
        let vested_amount = if current_time < schedule.cliff_end {
            0
        } else if current_time >= schedule.vesting_end {
            schedule.total
        } else {
            let vested_period = current_time - schedule.start;
            let total_period = schedule.vesting_end - schedule.start;
            (schedule.total * vested_period as i128) / total_period as i128
        };

        let unvested_amount = schedule.total - vested_amount;
        let refund_amount = vested_amount - schedule.claimed;

        let token: Address = env.storage().persistent().get(&DataKey::Token).expect("Not initialized");
        let token_client = TokenInterface::new(&env, &token);

        // Return unvested tokens to treasury (admin)
        if unvested_amount > 0 {
            token_client.transfer(&env.current_contract_address(), &admin, &unvested_amount);
        }

        // Mark schedule as revoked by removing it
        env.storage().persistent().remove(&DataKey::Schedule(schedule_id));

        // Remove from beneficiary's schedule list
        let mut schedules: Vec<u32> = env.storage().persistent()
            .get(&DataKey::BeneficiarySchedules(schedule.beneficiary.clone()))
            .unwrap_or(Vec::new(&env));
        
        schedules = schedules.filter(|&id| id != schedule_id);
        env.storage().persistent().set(&DataKey::BeneficiarySchedules(schedule.beneficiary), &schedules);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "VestingContract"), Symbol::new(&env, "ScheduleRevoked")),
            ScheduleRevokedEventData {
                schedule_id,
                beneficiary: schedule.beneficiary.clone(),
                refunded_amount: refund_amount,
            },
        );
    }

    /// Get all schedule IDs for a beneficiary.
    pub fn get_schedules_by_beneficiary(env: Env, addr: Address) -> Vec<u32> {
        env.storage().persistent()
            .get(&DataKey::BeneficiarySchedules(addr))
            .unwrap_or(Vec::new(&env))
    }

    /// Get schedule details by ID.
    pub fn get_schedule(env: Env, schedule_id: u32) -> VestingSchedule {
        env.storage().persistent()
            .get(&DataKey::Schedule(schedule_id))
            .expect("Schedule not found")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod test {
    extern crate std;
    use super::*;
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke, Events};
    use soroban_sdk::{Env, IntoVal, Symbol, vec};

    fn create_mock_token(env: &Env) -> Address {
        let token_contract_id = env.register_contract(None, MockToken);
        let token_client = MockTokenClient::new(env, &token_contract_id);
        
        let admin = Address::generate(env);
        token_client.initialize(&admin);
        
        token_contract_id
    }

    fn create_vesting_contract(env: &Env, token: Address, admin: Address) -> Address {
        let vesting_contract_id = env.register_contract(None, VestingContract);
        let vesting_client = VestingContractClient::new(env, &vesting_contract_id);
        
        vesting_client.initialize(&admin, &token);
        
        vesting_contract_id
    }

    #[test]
    fn test_initialization() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        
        let vesting_contract_id = env.register_contract(None, VestingContract);
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        vesting_client.initialize(&admin, &token);
        
        // Should panic if initialized again
        env.mock_all_auths();
        let result = std::panic::catch_unwind(|| {
            vesting_client.initialize(&admin, &token);
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_create_schedule() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin.clone());
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        assert_eq!(schedule_id, 1);
        
        let schedule = vesting_client.get_schedule(&schedule_id);
        assert_eq!(schedule.beneficiary, beneficiary);
        assert_eq!(schedule.total, 1000);
        assert_eq!(schedule.claimed, 0);
        
        let beneficiary_schedules = vesting_client.get_schedules_by_beneficiary(&beneficiary);
        assert_eq!(beneficiary_schedules.len(), 1);
        assert_eq!(beneficiary_schedules.get(0).unwrap(), schedule_id);
    }

    #[test]
    fn test_claimable_amount_cliff_not_reached() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin);
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Should be 0 before cliff
        let claimable = vesting_client.claimable_amount(&schedule_id);
        assert_eq!(claimable, 0);
    }

    #[test]
    fn test_claimable_amount_partial_vest() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin);
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Advance time to 50% through vesting (after cliff)
        env.ledger().set_timestamp(600); // 100 cliff + 500 vested
        
        let claimable = vesting_client.claimable_amount(&schedule_id);
        assert_eq!(claimable, 500); // 50% of 1000
    }

    #[test]
    fn test_claimable_amount_full_vest() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin);
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Advance time past vesting end
        env.ledger().set_timestamp(2000);
        
        let claimable = vesting_client.claimable_amount(&schedule_id);
        assert_eq!(claimable, 1000); // Full amount
    }

    #[test]
    fn test_claim_tokens() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin.clone());
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        // Mint tokens to vesting contract
        let token_client = MockTokenClient::new(&env, &token);
        token_client.mint(&vesting_contract_id, &1000);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Advance time past cliff
        env.ledger().set_timestamp(600);
        
        // Claim tokens
        vesting_client.claim(&schedule_id);
        
        let schedule = vesting_client.get_schedule(&schedule_id);
        assert_eq!(schedule.claimed, 500);
        assert_eq!(token_client.balance(&beneficiary), 500);
    }

    #[test]
    fn test_revoke_mid_vest() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin.clone());
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        // Mint tokens to vesting contract
        let token_client = MockTokenClient::new(&env, &token);
        token_client.mint(&vesting_contract_id, &1000);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Advance time to 50% through vesting
        env.ledger().set_timestamp(600);
        
        // Revoke schedule
        vesting_client.revoke(&schedule_id);
        
        // Admin should receive unvested tokens (500)
        assert_eq!(token_client.balance(&admin), 500);
        
        // Beneficiary should no longer have schedules
        let beneficiary_schedules = vesting_client.get_schedules_by_beneficiary(&beneficiary);
        assert_eq!(beneficiary_schedules.len(), 0);
    }

    #[test]
    #[should_panic(expected = "Nothing to claim")]
    fn test_claim_nothing_to_claim() {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let token = create_mock_token(&env);
        let vesting_contract_id = create_vesting_contract(&env, token, admin);
        let vesting_client = VestingContractClient::new(&env, &vesting_contract_id);
        
        let schedule_id = vesting_client.create_schedule(
            &beneficiary,
            &1000,
            &100,  // cliff
            &1000, // vesting
            &0,     // start immediately
        );
        
        // Try to claim before cliff
        vesting_client.claim(&schedule_id);
    }
}

// Mock token for testing
#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("Already initialized");
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::TotalSupply, &0i128);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).expect("Not initialized");
        admin.require_auth();

        let total_supply: i128 = env.storage().persistent().get(&DataKey::TotalSupply).unwrap_or(0);
        let new_total_supply = total_supply.checked_add(amount).expect("Overflow");

        let balance = Self::balance(env.clone(), to.clone());
        env.storage().persistent().set(&DataKey::Balance(to.clone()), &(balance + amount));
        env.storage().persistent().set(&DataKey::TotalSupply, &new_total_supply);
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(id))
            .unwrap_or(0)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();

        let from_balance = Self::balance(env.clone(), from.clone());
        if from_balance < amount {
            panic!("Insufficient balance");
        }

        let to_balance = Self::balance(env.clone(), to.clone());

        env.storage().persistent().set(&DataKey::Balance(from.clone()), &(from_balance - amount));
        env.storage().persistent().set(&DataKey::Balance(to.clone()), &(to_balance + amount));
    }
}

#[contractimpl]
impl TokenInterface for MockToken {
    fn allowance(_env: Env, _from: Address, _spender: Address) -> i128 {
        0
    }

    fn approve(_env: Env, _from: Address, _spender: Address, _amount: i128, _expiration_ledger: u32) {
        panic!("Not implemented");
    }

    fn balance(env: Env, id: Address) -> i128 {
        Self::balance(env, id)
    }

    fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        Self::transfer(env, from, to, amount);
    }

    fn transfer_from(_env: Env, _spender: Address, _from: Address, _to: Address, _amount: i128) {
        panic!("Not implemented");
    }

    fn burn(_env: Env, _from: Address, _amount: i128) {
        panic!("Not implemented");
    }

    fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {
        panic!("Not implemented");
    }

    fn decimals(_env: Env) -> u32 {
        7
    }

    fn name(_env: Env) -> String {
        String::from_str(&_env, "Mock Token")
    }

    fn symbol(_env: Env) -> String {
        String::from_str(&_env, "MOCK")
    }
}
