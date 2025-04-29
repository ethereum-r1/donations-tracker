use crate::sql::DbClient;
use alloy::contract::{ContractInstance, Interface};
use alloy::dyn_abi::DynSolValue;
use alloy::json_abi::JsonAbi;
use alloy::primitives::{Address, FixedBytes};
use alloy::providers::Provider;
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
}

impl<P: Provider> Checker<P> {
    pub fn new(
        target_address: String,
        etherscan_api_key: String,
        provider: P,
        http_client: Client,
        pg_client: DbClient,
    ) -> Self {
        Self {
            target_address,
            etherscan_api_key,
            provider,
            http_client,
            pg_client,
        }
    }

    pub async fn run(&self) {
        loop {
            if let Err(e) = self.check_transfers().await {
                println!("Error checking transfers: {}", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }
    }

    pub async fn check_transfers(&self) -> Result<()> {
        let normal_url = format!(
            "https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address={}&startblock=0&endblock=99999999&sort=asc&apikey={}",
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
            "https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlistinternal&address={}&startblock=0&endblock=99999999&sort=asc&apikey={}",
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
                    let hash_key = generate_hash_key(&tx.value, &tx.from, &tx.hash);

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

                    println!("From: {} Amount: {} ETH", from_display, eth_amount);

                    // Insert new entry into Postgres
                    self.pg_client
                        .insert_transfer(tx.hash, tx.from, eth_amount, hash_key, from_display)
                        .await?;
                }
            }
        }

        Ok(())
    }
}

/// Resolve ENS name for a given Ethereum address.
/// Returns Some(name) or None if no reverse record.
pub async fn resolve_ens_name<P: Provider>(provider: &P, address: Address) -> Option<String> {
    // ENS Registry address
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

pub fn generate_hash_key(amount_wei: &str, from_address: &str, tx_hash: &str) -> String {
    let input = format!("{}{}{}", amount_wei, from_address, tx_hash);
    let mut hasher = Sha256::new();
    hasher.update(input);
    let result = hasher.finalize();
    hex::encode(result)
}
