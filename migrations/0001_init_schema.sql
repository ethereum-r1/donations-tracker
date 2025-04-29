CREATE TABLE eth_transfers (
    id SERIAL PRIMARY KEY,
    tx_hash TEXT NOT NULL,
    from_address TEXT NOT NULL,
    eth_amount TEXT NOT NULL,
    hash_key TEXT UNIQUE NOT NULL,
    from_name TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT NOW()
);
