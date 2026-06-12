#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    Address, Env, String, Vec,
};

/// One disbursement record stored on-chain.
#[contracttype]
#[derive(Clone)]
pub struct DisbursementRecord {
    pub org_id: u64,
    pub recipient: String,
    pub amount: i128,
    pub currency: String,
    pub timestamp: u64,
    pub clawed_back: bool,
}

#[contracttype]
pub struct DisbursementLoggedEvent {
    pub org_id: u64,
    pub recipient: String,
    pub amount: i128,
    pub currency: String,
    pub index: u32,
}

#[contracttype]
pub struct ClawbackExecutedEvent {
    pub index: u32,
    pub org_id: u64,
    pub caller: Address,
}

const ADMIN_KEY: &str = "admin";
const RECORDS_KEY: &str = "records";
const CLAWBACK_WINDOW_SECS: u64 = 48 * 3600;

#[contract]
pub struct ComplianceContract;

#[contractimpl]
impl ComplianceContract {
    /// Initialize with an admin address.
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        env.storage().instance().set(&symbol_short!("admin"), &admin);
    }

    /// Log a disbursement. Returns the record index.
    pub fn log_disbursement(
        env: Env,
        org_id: u64,
        recipient: String,
        amount: i128,
        currency: String,
    ) -> u32 {
        let mut records: Vec<DisbursementRecord> = env
            .storage()
            .instance()
            .get(&symbol_short!("records"))
            .unwrap_or(Vec::new(&env));

        let record = DisbursementRecord {
            org_id,
            recipient: recipient.clone(),
            amount,
            currency: currency.clone(),
            timestamp: env.ledger().timestamp(),
            clawed_back: false,
        };
        records.push_back(record);
        let idx = records.len() - 1;
        env.storage().instance().set(&symbol_short!("records"), &records);

        env.events().publish(
            (symbol_short!("disb_log"),),
            DisbursementLoggedEvent {
                org_id,
                recipient,
                amount,
                currency,
                index: idx,
            },
        );

        idx
    }

    /// Clawback a disbursement by index (admin only, within 48 hours).
    pub fn clawback(env: Env, caller: Address, index: u32) {
        caller.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        if caller != admin {
            panic!("unauthorized");
        }

        let mut records: Vec<DisbursementRecord> = env
            .storage()
            .instance()
            .get(&symbol_short!("records"))
            .expect("no records");

        let mut record = records.get(index).expect("invalid index");
        let now = env.ledger().timestamp();
        if now - record.timestamp > CLAWBACK_WINDOW_SECS {
            panic!("clawback window expired");
        }
        if record.clawed_back {
            panic!("already clawed back");
        }
        let org_id = record.org_id;
        record.clawed_back = true;
        records.set(index, record);
        env.storage().instance().set(&symbol_short!("records"), &records);

        env.events().publish(
            (symbol_short!("clawback"),),
            ClawbackExecutedEvent {
                index,
                org_id,
                caller,
            },
        );
    }

    /// Get a disbursement record by index.
    pub fn get_record(env: Env, index: u32) -> DisbursementRecord {
        let records: Vec<DisbursementRecord> = env
            .storage()
            .instance()
            .get(&symbol_short!("records"))
            .expect("no records");
        records.get(index).expect("invalid index")
    }

    /// Get total number of records.
    pub fn record_count(env: Env) -> u32 {
        let records: Vec<DisbursementRecord> = env
            .storage()
            .instance()
            .get(&symbol_short!("records"))
            .unwrap_or(Vec::new(&env));
        records.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Events, Ledger},
        vec, IntoVal, Env, String,
    };

    #[test]
    fn test_log_and_get() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ComplianceContract);
        let client = ComplianceContractClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client.init(&admin);

        let idx = client.log_disbursement(
            &1u64,
            &String::from_str(&env, "GADDR123"),
            &1000i128,
            &String::from_str(&env, "USDC"),
        );
        assert_eq!(idx, 0);
        assert_eq!(client.record_count(), 1);

        let rec = client.get_record(&0);
        assert_eq!(rec.org_id, 1);
        assert!(!rec.clawed_back);
    }

    #[test]
    fn test_clawback_within_window() {
        let env = Env::default();
        env.ledger().set_timestamp(1000);
        let contract_id = env.register_contract(None, ComplianceContract);
        let client = ComplianceContractClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client.init(&admin);
        client.log_disbursement(
            &1u64,
            &String::from_str(&env, "GADDR"),
            &500i128,
            &String::from_str(&env, "USDC"),
        );

        env.mock_all_auths();
        client.clawback(&admin, &0);
        let rec = client.get_record(&0);
        assert!(rec.clawed_back);
    }

    #[test]
    #[should_panic(expected = "clawback window expired")]
    fn test_clawback_expired() {
        let env = Env::default();
        env.ledger().set_timestamp(1000);
        let contract_id = env.register_contract(None, ComplianceContract);
        let client = ComplianceContractClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client.init(&admin);
        client.log_disbursement(
            &1u64,
            &String::from_str(&env, "GADDR"),
            &500i128,
            &String::from_str(&env, "USDC"),
        );

        // Advance past 48h window
        env.ledger().set_timestamp(1000 + CLAWBACK_WINDOW_SECS + 1);
        env.mock_all_auths();
        client.clawback(&admin, &0);
    }

    #[test]
    fn test_log_disbursement_emits_event() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ComplianceContract);
        let client = ComplianceContractClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client.init(&admin);

        client.log_disbursement(
            &1u64,
            &String::from_str(&env, "GADDR123"),
            &1000i128,
            &String::from_str(&env, "USDC"),
        );

        let events = env.events().all();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events,
            vec![
                &env,
                (
                    contract_id.clone(),
                    (symbol_short!("disb_log"),).into_val(&env),
                    DisbursementLoggedEvent {
                        org_id: 1u64,
                        recipient: String::from_str(&env, "GADDR123"),
                        amount: 1000i128,
                        currency: String::from_str(&env, "USDC"),
                        index: 0u32,
                    }
                    .into_val(&env),
                )
            ]
        );
    }

    #[test]
    fn test_clawback_emits_event() {
        let env = Env::default();
        env.ledger().set_timestamp(1000);
        let contract_id = env.register_contract(None, ComplianceContract);
        let client = ComplianceContractClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client.init(&admin);
        client.log_disbursement(
            &2u64,
            &String::from_str(&env, "GADDR_CB"),
            &500i128,
            &String::from_str(&env, "USDC"),
        );

        env.mock_all_auths();
        client.clawback(&admin, &0);

        // Two events: one from log_disbursement, one from clawback
        let events = env.events().all();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events.get(1),
            (
                contract_id.clone(),
                (symbol_short!("clawback"),).into_val(&env),
                ClawbackExecutedEvent {
                    index: 0u32,
                    org_id: 2u64,
                    caller: admin.clone(),
                }
                .into_val(&env),
            )
        );
    }
}
