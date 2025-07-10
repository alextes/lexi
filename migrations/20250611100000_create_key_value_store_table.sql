-- migrations/20250611100000_create_key_value_store_table.sql

CREATE TABLE key_value_store (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
); 