use eyre::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct DbClient {
    pub pool: PgPool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Transfer {
    tx_hash: String,
    from_address: String,
    eth_amount: String,
    from_name: String,
}

impl DbClient {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn check_transfer_exists(&self, hash_key: String) -> Result<bool> {
        sqlx::query("SELECT COUNT(*) FROM eth_transfers WHERE hash_key = $1")
            .bind(hash_key)
            .fetch_one(&self.pool)
            .await
            .map(|row| row.get::<Option<i64>, _>(0).unwrap_or(0) > 0)
            .map_err(|e| eyre::eyre!("Failed to check transfers: {}", e))
    }

    pub async fn check_donation_exists(&self, hash_key: String) -> Result<bool> {
        sqlx::query("SELECT COUNT(*) FROM donations WHERE hash_key = $1")
            .bind(hash_key)
            .fetch_one(&self.pool)
            .await
            .map(|row| row.get::<Option<i64>, _>(0).unwrap_or(0) > 0)
            .map_err(|e| eyre::eyre!("Failed to check transfers: {}", e))
    }

    pub async fn insert_transfer(
        &self,
        tx_hash: String,
        from_address: String,
        eth_amount: String,
        hash_key: String,
        from_name: String,
    ) -> Result<()> {
        sqlx::query("INSERT INTO eth_transfers (tx_hash, from_address, eth_amount, hash_key, from_name) VALUES ($1, $2, $3, $4, $5)")
        .bind(tx_hash)
        .bind(from_address)
        .bind(eth_amount)
        .bind(hash_key)
        .bind(from_name)
        .execute(&self.pool)
        .await
        .map_err(|e| eyre::eyre!("Failed to insert transfer: {}", e))?;

        Ok(())
    }

    pub async fn get_transfers(&self) -> Result<Vec<Transfer>> {
        sqlx::query("SELECT tx_hash, from_address, eth_amount, from_name FROM eth_transfers")
            .fetch_all(&self.pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| Transfer {
                        tx_hash: row.get("tx_hash"),
                        from_address: row.get("from_address"),
                        eth_amount: row.get("eth_amount"),
                        from_name: row.get("from_name"),
                    })
                    .collect()
            })
            .map_err(|e| eyre::eyre!("Failed to fetch transfers: {}", e))
    }

    pub async fn insert_donation(
        &self,
        removed: bool,
        tx_hash: String,
        log_index: String,
        from_address: String,
        eth_amount: String,
        hash_key: String,
        from_name: String,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO donations (
                removed, tx_hash, log_index, from_address, eth_amount, hash_key, from_name
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (hash_key) 
            DO UPDATE SET
            removed = EXCLUDED.removed,
            "#,
        )
        .bind(removed)
        .bind(tx_hash)
        .bind(log_index)
        .bind(from_address)
        .bind(eth_amount)
        .bind(hash_key)
        .bind(from_name)
        .execute(&self.pool)
        .await
        .map_err(|e| eyre::eyre!("Failed to insert donation: {}", e))?;

        Ok(())
    }

    pub async fn get_donations(&self) -> Result<Vec<Transfer>> {
        sqlx::query("SELECT tx_hash, from_address, eth_amount, from_name FROM donations WHERE removed = false")
            .fetch_all(&self.pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| Transfer {
                        tx_hash: row.get("tx_hash"),
                        from_address: row.get("from_address"),
                        eth_amount: row.get("eth_amount"),
                        from_name: row.get("from_name"),
                    })
                    .collect()
            })
            .map_err(|e| eyre::eyre!("Failed to fetch donations: {}", e))
    }
}
