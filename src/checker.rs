use crate::sql::DbClient;
use alloy::contract::{ContractInstance, Interface};
use alloy::dyn_abi::DynSolValue;
use alloy::json_abi::JsonAbi;
use alloy::primitives::{Address, FixedBytes};
use alloy::providers::Provider;
use alloy::rpc::types::{BlockNumberOrTag::Latest, Filter, Log};
use alloy::sol;
use eyre::Result;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use tiny_keccak::Keccak;

#[derive(Debug, Deserialize)]
struct TransactionInfo {
    from: String,
    to: String,
    value: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
struct EtherscanResponse {
    status: String,
    result: Vec<TransactionInfo>,
}

pub struct Checker<P: Provider> {
    target_address: String,
    etherscan_api_key: String,
    http_client: Client,
    pg_client: DbClient,
    provider: P,
    start_block: u64,
    chain_id: u64,
    filter: Filter,
}

sol! {
    interface IDonate {
        event Donation(address indexed donor, uint256 amount);
    }
}

impl<P: Provider> Checker<P> {
    pub fn new(
        target_transfer_address: String,
        target_donation_address: String,
        etherscan_api_key: String,
        provider: P,
        http_client: Client,
        pg_client: DbClient,
        start_block: u64,
        chain_id: u64,
    ) -> Self {
        Self {
            target_address: target_transfer_address.clone(),
            etherscan_api_key,
            provider,
            http_client,
            pg_client,
            start_block,
            filter: Filter::new()
                .address(vec![
                    Address::from_str(&target_donation_address.clone()).unwrap()
                ]),
            chain_id,
        }
    }

    pub async fn run(&self) {
        if let Err(err) = self.process_past_logs().await {
            eprintln!("Error processing past logs: {}", err);
        }

        loop {
            if let Err(e) = self.check_transfers().await {
                println!("Error checking transfers: {}", e);
            }
            if let Err(e) = self.process_new_logs().await {
                println!("Error checking donations: {}", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;
        }
    }

    pub async fn process_past_logs(&self) -> Result<()> {
        let block = self.provider.get_block_by_number(Latest).await?;
        let mut current_end_block = block.unwrap().header.number;

        while self.start_block < current_end_block {
            let current_start_block = if current_end_block >= 49999 {
                current_end_block - 49999
            } else {
                0
            };

            let filter = self
                .filter
                .clone()
                .from_block(current_start_block)
                .to_block(current_end_block);

            // Fetch logs
            let logs = self.provider.get_logs(&filter).await?;
            for log in logs {
                self.process_donation_event(log.clone()).await?;
            }

            current_end_block = current_start_block;
        }
        Ok(())
    }

    pub async fn process_new_logs(&self) -> Result<()> {
        let block = self.provider.get_block_by_number(Latest).await?;
        let current_end_block = block.unwrap().header.number;
        let current_start_block = if current_end_block >= 64 {
            current_end_block - 64
        } else {
            0
        };
        let filter = self
            .filter
            .clone()
            .from_block(current_start_block)
            .to_block(current_end_block);
        let logs = self.provider.get_logs(&filter).await?;
        for log in logs {
            self.process_donation_event(log.clone()).await?;
        }
        Ok(())
    }

    pub async fn check_transfers(&self) -> Result<()> {
        let normal_url = format!(
            "https://api.etherscan.io/v2/api?chainid={}&module=account&action=txlist&address={}&startblock=0&endblock=99999999&sort=asc&apikey={}",
            self.chain_id,
            self.target_address,
            self.etherscan_api_key
        );

        let normal_response = self
            .http_client
            .get(&normal_url)
            .send()
            .await?
            .json::<EtherscanResponse>()
            .await?;

        // Fetch internal transactions
        let internal_url = format!(
            "https://api.etherscan.io/v2/api?chainid={}&module=account&action=txlistinternal&address={}&startblock=0&endblock=99999999&sort=asc&apikey={}",
            self.chain_id,
            self.target_address,
            self.etherscan_api_key
        );

        let internal_response = self
            .http_client
            .get(&internal_url)
            .send()
            .await?
            .json::<EtherscanResponse>()
            .await?;

        if normal_response.status != "1" && internal_response.status != "1" {
            println!("Error fetching data: both responses failed.");
            return Err(eyre::eyre!("Failed to fetch data from Etherscan"));
        }

        let all_txs: Vec<TransactionInfo> = normal_response
            .result
            .into_iter()
            .chain(internal_response.result.into_iter())
            .collect();

        // Handle normal txs
        for tx in all_txs {
            if tx.to.to_lowercase() == self.target_address.to_lowercase() {
                let value_in_wei: u128 = tx.value.parse().unwrap_or(0);
                if value_in_wei > 0 {
                    // skip failed txs
                    let hash_key = generate_transfer_hash_key(&tx.value, &tx.from, &tx.hash);

                    // Query if this hash_key already exists
                    let exists = self
                        .pg_client
                        .check_transfer_exists(hash_key.clone())
                        .await?;
                    if exists {
                        continue;
                    }

                    // If it's a new transaction: resolve ENS
                    let from_address = Address::from_str(&tx.from)?;
                    let from_display = match resolve_ens_name(&self.provider, from_address).await {
                        Some(name) => name,
                        None => format!("{:?}", from_address),
                    };

                    let value_in_wei: u128 = tx.value.parse().unwrap_or(0);
                    let eth_amount = format!("{:.18}", (value_in_wei as f64) / 1e18f64);

                    println!(
                        "TRANSFER -- From: {} Amount: {} ETH",
                        from_display, eth_amount
                    );

                    // Insert new entry into Postgres
                    self.pg_client
                        .insert_transfer(tx.hash, tx.from, eth_amount, hash_key, from_display)
                        .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn process_donation_event(&self, log: Log) -> Result<()> {
        match log.log_decode::<IDonate::Donation>() {
            Ok(decoded_log) => {
                let tx_hash = log.transaction_hash.unwrap_or_default().to_string();
                let log_index = log.log_index.unwrap_or_default().to_string();
                let amount = decoded_log.inner.amount.to_string();
                let donor = decoded_log.inner.donor.to_string();
                let hash_key = generate_donation_hash_key(&amount, &donor, &tx_hash, &log_index);

                // Query if this hash_key already exists
                let exists = self
                    .pg_client
                    .check_donation_exists(hash_key.clone())
                    .await?;
                let mut from_display = "".to_string();
                if !exists {
                    // If it's a new transaction: resolve ENS
                    from_display =
                        match resolve_ens_name(&self.provider, decoded_log.inner.donor).await {
                            Some(name) => name,
                            None => donor.clone(),
                        };

                    println!("DONATION -- From: {}", from_display);
                }

                let value_in_wei: u128 = amount.parse().unwrap_or(0);
                let eth_amount = format!("{:.18}", (value_in_wei as f64) / 1e18f64);

                // Insert new entry into Postgres
                self.pg_client
                    .insert_donation(
                        decoded_log.removed,
                        tx_hash,
                        log_index,
                        donor,
                        eth_amount,
                        hash_key,
                        from_display,
                    )
                    .await?;
            }
            Err(e) => {
                println!("Failed to decode Donation event: {:?}", e);
            }
        }
        Ok(())
    }
}

/// Resolve ENS name for a given Ethereum address.
/// Returns Some(name) or None if no reverse record.
pub async fn resolve_ens_name<P: Provider>(provider: &P, address: Address) -> Option<String> {
    // ENS Registry address
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    let ens_registry = Address::from_str("0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e").unwrap();

    // Step 1: Create the reverse record name
    let reverse_name = format!("{:x}.addr.reverse", address);

    // Step 2: Hash the reverse name
    let node_string = namehash(&reverse_name)?;
    let node = FixedBytes::from_str(&node_string).unwrap();

    // Step 3: resolver(bytes32 node) function selector
    let abi = JsonAbi::parse(["function resolver(bytes32) external view returns (address)"])
        .expect("Failed to parse ABI");

    let contract = ContractInstance::new(ens_registry, provider, Interface::new(abi));

    let return_val_raw = contract
        .function("resolver", &[DynSolValue::FixedBytes(node, 32)])
        .expect("Failed to create method call")
        .call()
        .await
        .expect("Failed to call resolver");
    let resolver_addr = return_val_raw[0]
        .as_address()
        .expect("Expected address output");
    if resolver_addr == Address::ZERO {
        return None;
    }

    // Step 5: Call name(bytes32 node) on resolver
    let abi2 = JsonAbi::parse(["function name(bytes32) external view returns (string memory)"])
        .expect("Failed to parse ABI");

    let contract2 = ContractInstance::new(resolver_addr, provider, Interface::new(abi2));

    let return_val_raw2 = contract2
        .function("name", &[DynSolValue::FixedBytes(node, 32)])
        .expect("Failed to create method call")
        .call()
        .await
        .expect("Failed to call name");
    let end_name = return_val_raw2[0].as_str().expect("Expected string output");
    if end_name.is_empty() {
        return None;
    } else {
        return Some(end_name.to_string());
    }
}

pub fn namehash(name: &str) -> Option<String> {
    let mut node = vec![0u8; 32];
    if name.is_empty() {
        return None;
    }
    let mut labels: Vec<&str> = name.split(".").collect();
    labels.reverse();
    for label in labels.iter() {
        let mut labelhash = [0u8; 32];
        Keccak::keccak256(label.as_bytes(), &mut labelhash);
        node.append(&mut labelhash.to_vec());
        labelhash = [0u8; 32];
        Keccak::keccak256(node.as_slice(), &mut labelhash);
        node = labelhash.to_vec();
    }
    Some("0x".to_string() + &hex::encode(node))
}

pub fn generate_transfer_hash_key(amount_wei: &str, from_address: &str, tx_hash: &str) -> String {
    let input = format!("{}{}{}", amount_wei, from_address, tx_hash);
    let mut hasher = Sha256::new();
    hasher.update(input);
    let result = hasher.finalize();
    hex::encode(result)
}

pub fn generate_donation_hash_key(
    amount_wei: &str,
    from_address: &str,
    tx_hash: &str,
    log_index: &str,
) -> String {
    let input = format!("{}{}{}{}", amount_wei, from_address, tx_hash, log_index);
    let mut hasher = Sha256::new();
    hasher.update(input);
    let result = hasher.finalize();
    hex::encode(result)
}
