#[macro_use]
extern crate rocket;

mod checker;
mod sql;

use alloy::providers::ProviderBuilder;
use checker::Checker;
use dotenv::dotenv;
use reqwest::Client;
use rocket::http::Status;
use rocket::response::status::Custom;
use rocket::{serde::json::Json, State};
use sql::{DbClient, Transfer};
use sqlx::PgPool;
use std::env;
use tokio::time::Duration;
use url::Url;

#[get("/transfers")]
async fn get_transfers(db: &State<DbClient>) -> Result<Json<Vec<Transfer>>, Custom<String>> {
    match db.get_transfers().await {
        Ok(transfers) => Ok(Json(transfers)),
        Err(e) => Err(Custom(
            Status::InternalServerError,
            format!("Database error: {}", e),
        )),
    }
}

#[get("/donations")]
async fn get_donations(db: &State<DbClient>) -> Result<Json<Vec<Transfer>>, Custom<String>> {
    match db.get_donations().await {
        Ok(donations) => Ok(Json(donations)),
        Err(e) => Err(Custom(
            Status::InternalServerError,
            format!("Database error: {}", e),
        )),
    }
}

#[get("/health")]
async fn health() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    println!("loading dotenv...");
    dotenv().ok();
    println!("starting...");
    let etherscan_api_key = env::var("ETHERSCAN_API_KEY").expect("❌ Missing ETHERSCAN_API_KEY");
    let rpc_url_transfer_str = env::var("RPC_URL_TRANSFER").expect("❌ Missing RPC_URL_TRANSFER");
    let rpc_url_transfer = Url::parse(&rpc_url_transfer_str)?;
    let rpc_url_donation_str = env::var("RPC_URL_DONATION").expect("❌ Missing RPC_URL_DONATION");
    let rpc_url_donation = Url::parse(&rpc_url_donation_str)?;
    let database_url = env::var("DATABASE_URL").expect("❌ Missing DATABASE_URL");
    let target_transfer_address =
        env::var("TARGET_TRANSFER_ADDRESS").expect("❌ Missing TARGET_TRANSFER_ADDRESS");
    let target_donation_address =
        env::var("TARGET_DONATION_ADDRESS").expect("❌ Missing TARGET_DONATION_ADDRESS");
    let start_block_str = env::var("START_BLOCK").expect("❌ Missing START_BLOCK");

    let start_block = start_block_str
        .parse::<u64>()
        .expect("❌ Invalid START_BLOCK");

    let provider_transfer = ProviderBuilder::new().connect_http(rpc_url_transfer);
    let provider_donation = ProviderBuilder::new().connect_http(rpc_url_donation);

    let pg_pool = loop {
        println!("⏳ Attempting to connect to Postgres...");
        match PgPool::connect(&database_url).await {
            Ok(pool) => {
                println!("✅ Connected to Postgres!");
                break pool;
            }
            Err(e) => {
                eprintln!("⚠️ Failed to connect to Postgres: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    println!("running migrations...");
    sqlx::migrate!().run(&pg_pool).await?;
    let pg_client = DbClient::new(pg_pool.clone());

    let client = Client::new();

    println!("initializing checker...");
    let checker = Checker::new(
        target_transfer_address,
        target_donation_address,
        etherscan_api_key,
        provider_transfer,
        provider_donation,
        client,
        pg_client.clone(),
        start_block,
    );

    // Spawn the background checker
    tokio::spawn(async move {
        checker.run().await;
    });

    println!("launching rocket server...");
    // Launch Rocket server
    let result = rocket::build()
        .manage(pg_client)
        .mount("/", routes![get_transfers, get_donations, health])
        .launch()
        .await;

    match result {
        Ok(_) => println!("🚀 Rocket launched successfully."),
        Err(e) => {
            eprintln!("🔥 Rocket launch failed: {:?}", e);
        }
    }

    Ok(())
}
