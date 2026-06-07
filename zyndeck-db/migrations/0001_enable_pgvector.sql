-- Enable pgvector so rule embeddings can be stored and queried directly in
-- Postgres, alongside the relational data. Domain tables come in later
-- migrations as the schema firms up.
CREATE EXTENSION IF NOT EXISTS vector;
