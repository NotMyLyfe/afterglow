#!/bin/bash
set -e

# Create a replication role for the replica to connect with
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE ROLE replicator WITH REPLICATION LOGIN PASSWORD 'afterglowReplPw';
EOSQL

# Allow replication connections from any IP address (for testing purposes)
cat >> "$PGDATA/pg_hba.conf" <<EOL
host replication replicator 0.0.0.0/0 md5
host all all 0.0.0.0/0 md5
EOL

pg_ctl reload

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE TABLE IF NOT EXISTS test_table (
        id SERIAL PRIMARY KEY,
        data TEXT NOT NULL
    );

    INSERT INTO test_table (data) VALUES ('Hello, Afterglow!');
EOSQL
