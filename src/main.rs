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

#[get("/health")]
async fn health() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenv().ok();

    let etherscan_api_key = env::var("ETHERSCAN_API_KEY")?;
    let rpc_url_string = env::var("RPC_URL")?;
    let rpc_url = Url::parse(&rpc_url_string)?;

    let provider = ProviderBuilder::new().connect_http(rpc_url);

    let database_url = env::var("DATABASE_URL")?;
    println!("connecting to db...");
    let pg_pool = PgPool::connect(&database_url).await?;
    println!("running migrations...");
    sqlx::migrate!().run(&pg_pool).await?;
    let pg_client = DbClient::new(pg_pool.clone());

    let target_address = env::var("TARGET_ADDRESS")?;
    let client = Client::new();

    println!("initializing checker...");
    let checker = Checker::new(
        target_address,
        etherscan_api_key,
        provider,
        client,
        pg_client.clone(),
    );

    // Spawn the background checker
    tokio::spawn(async move {
        checker.run().await;
    });

    println!("launching rocket server...");
    // Launch Rocket server
    let result = rocket::build()
    .manage(pg_client)
    .mount("/", routes![get_transfers, health])
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
