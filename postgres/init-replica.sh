#!/bin/bash
set -e

DATA_DIR="/var/lib/postgresql/data"

if [ -z "$(ls -A $DATA_DIR 2>/dev/null)" ]; then
    echo "Data directory is empty. Initializing replica..."
    
    # Wait til primary ready for replication connections
    # Need to use replication endpoint, not pg_isready since
    # pg_isready doesn't check replication connection availability
    until PGPASSWORD=afterglowReplPw psql -h primary -U replicator -d postgres -c '\q' 2>/dev/null; do
        echo "Waiting for primary to be ready for replication..."
        sleep 2
    done

    PGPASSWORD=afterglowReplPw pg_basebackup -h primary -U replicator -D $DATA_DIR -Fp -Xs -P -R

    chmod 700 $DATA_DIR
else
    echo "Data directory is not empty. Skipping initialization."
fi

grep -q "^hot_standby" "$DATA_DIR/postgresql.auto.conf" 2>/dev/null || \
    echo "hot_standby = on" >> "$DATA_DIR/postgresql.auto.conf"

grep -q "^recovery_min_apply_delay" "$DATA_DIR/postgresql.auto.conf" 2>/dev/null || \
    echo "recovery_min_apply_delay = '${REPLICA_APPLY_DELAY}'" >> "$DATA_DIR/postgresql.auto.conf"

exec docker-entrypoint.sh postgres
