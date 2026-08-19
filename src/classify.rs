use pg_query::NodeEnum;
use pg_query::parse;
use pg_query::protobuf::TransactionStmtKind;
use pg_query::protobuf::WithClause;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TxAction {
    Begin,
    Commit,
    Rollback,
    Savepoint,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryKind {
    Read,
    Write,
    Transaction(TxAction),
    Utility,
    Unknown,
}

fn with_clause_has_write(with: &WithClause) -> bool {
    for cte in &with.ctes {
        if let Some(node) = cte.node.as_ref()
            && classify_node(node) == QueryKind::Write
        {
            return true;
        }
    }
    false
}

fn classify_node(node: &NodeEnum) -> QueryKind {
    match node {
        NodeEnum::CommonTableExpr(cte) => {
            match cte.ctequery.as_ref().and_then(|s| s.node.as_ref()) {
                Some(inner_node) => classify_node(inner_node),
                None => QueryKind::Write,
            }
        }
        NodeEnum::SelectStmt(stmt) => {
            if stmt.into_clause.is_some() || !stmt.locking_clause.is_empty() {
                QueryKind::Write
            } else if let Some(with) = &stmt.with_clause {
                if with_clause_has_write(with) {
                    QueryKind::Write
                } else {
                    QueryKind::Read
                }
            } else {
                QueryKind::Read
            }
        }
        // TODO: Handle InsertStmt/UpdateStmt/DeleteStmt differently than CreateStmt/AlterTableStmt/DropStmt, as the former are DML and the latter are DDL
        #[allow(clippy::match_same_arms)]
        NodeEnum::InsertStmt(_) | NodeEnum::UpdateStmt(_) | NodeEnum::DeleteStmt(_) => {
            QueryKind::Write
        }
        NodeEnum::CreateStmt(_) | NodeEnum::AlterTableStmt(_) | NodeEnum::DropStmt(_) => {
            QueryKind::Write
        }
        NodeEnum::TransactionStmt(stmt) => match stmt.kind() {
            TransactionStmtKind::TransStmtBegin | TransactionStmtKind::TransStmtStart => {
                QueryKind::Transaction(TxAction::Begin)
            }
            TransactionStmtKind::TransStmtCommit => QueryKind::Transaction(TxAction::Commit),
            TransactionStmtKind::TransStmtRollback => QueryKind::Transaction(TxAction::Rollback),
            TransactionStmtKind::TransStmtSavepoint => QueryKind::Transaction(TxAction::Savepoint),
            _ => QueryKind::Transaction(TxAction::Other),
        },
        NodeEnum::VariableSetStmt(_) | NodeEnum::VariableShowStmt(_) => QueryKind::Utility,
        _ => QueryKind::Write, // Default to Write for unclassified statements as they may have side effects
    }
}

fn classification_priority(a: QueryKind, b: QueryKind) -> QueryKind {
    use QueryKind::{Read, Transaction, Unknown, Utility, Write};

    // Write > Transaction > Utility > Unknown > Read
    match (a, b) {
        (Write, _) | (_, Write) => Write,
        // We need to preserve the specific transaction action if either is a transaction
        (Transaction(action_a), Transaction(action_b)) => Transaction(
            if matches!(
                action_a,
                TxAction::Begin | TxAction::Commit | TxAction::Rollback
            ) {
                action_a
            } else {
                action_b
            },
        ),
        (Transaction(_), _) => a,
        (_, Transaction(_)) => b,
        (Utility, _) | (_, Utility) => Utility,
        (Unknown, _) | (_, Unknown) => Unknown,
        (Read, Read) => Read,
    }
}

pub fn classify(query: &str) -> QueryKind {
    let result = match parse(query) {
        Ok(res) => res,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse query for classification: {}", query);
            return QueryKind::Unknown;
        }
    };

    let raw_stmts = result.protobuf.stmts;

    if raw_stmts.is_empty() {
        return QueryKind::Unknown;
    }

    raw_stmts
        .iter()
        .map(|stmt| {
            stmt.stmt
                .as_ref()
                .and_then(|s| s.node.as_ref())
                .map_or(QueryKind::Write, classify_node)
        })
        .reduce(classification_priority)
        .unwrap_or(QueryKind::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(sql: &str) -> QueryKind {
        classify(sql)
    }

    // --- Reads ---
    #[test]
    fn plain_select_is_read() {
        assert_eq!(kind("SELECT 1"), QueryKind::Read);
        assert_eq!(kind("SELECT * FROM users WHERE id = 5"), QueryKind::Read);
        assert_eq!(kind("SELECT count(*) FROM orders"), QueryKind::Read);
    }

    #[test]
    fn join_is_read() {
        assert_eq!(
            kind("SELECT u.id, p.bio FROM users u JOIN profiles p ON p.uid = u.id"),
            QueryKind::Read
        );
    }

    // --- Reads that are actually writes ---
    #[test]
    fn select_for_update_is_write() {
        assert_eq!(
            kind("SELECT * FROM users WHERE id = 1 FOR UPDATE"),
            QueryKind::Write
        );
    }

    #[test]
    fn select_for_share_is_write() {
        assert_eq!(kind("SELECT * FROM users FOR SHARE"), QueryKind::Write);
    }

    #[test]
    fn select_into_is_write() {
        assert_eq!(kind("SELECT * INTO tmp FROM users"), QueryKind::Write);
    }

    // --- Writes ---
    #[test]
    fn dml_is_write() {
        assert_eq!(kind("INSERT INTO t (a) VALUES (1)"), QueryKind::Write);
        assert_eq!(kind("UPDATE t SET a = 1 WHERE id = 2"), QueryKind::Write);
        assert_eq!(kind("DELETE FROM t WHERE id = 3"), QueryKind::Write);
    }

    #[test]
    fn insert_select_is_write() {
        assert_eq!(kind("INSERT INTO t SELECT * FROM s"), QueryKind::Write);
    }

    #[test]
    fn ddl_is_write() {
        assert_eq!(kind("CREATE TABLE t (id int)"), QueryKind::Write);
        assert_eq!(kind("DROP TABLE t"), QueryKind::Write);
        assert_eq!(kind("ALTER TABLE t ADD COLUMN b text"), QueryKind::Write);
    }

    // --- Transactions ---
    #[test]
    fn begin_is_transaction_begin() {
        assert_eq!(kind("BEGIN"), QueryKind::Transaction(TxAction::Begin));
        assert_eq!(
            kind("START TRANSACTION"),
            QueryKind::Transaction(TxAction::Begin)
        );
    }

    #[test]
    fn commit_and_rollback() {
        assert_eq!(kind("COMMIT"), QueryKind::Transaction(TxAction::Commit));
        assert_eq!(kind("ROLLBACK"), QueryKind::Transaction(TxAction::Rollback));
    }

    #[test]
    fn savepoint_is_savepoint() {
        assert_eq!(
            kind("SAVEPOINT sp1"),
            QueryKind::Transaction(TxAction::Savepoint)
        );
    }

    // --- Utility ---
    #[test]
    fn set_and_show_are_utility() {
        assert_eq!(kind("SET search_path = public"), QueryKind::Utility);
        assert_eq!(kind("SHOW server_version"), QueryKind::Utility);
    }

    // --- Multi-statement (priority: Write > Transaction > Utility > Unknown > Read) ---
    #[test]
    fn multi_read_is_read() {
        assert_eq!(kind("SELECT 1; SELECT 2"), QueryKind::Read);
    }

    #[test]
    fn multi_with_write_is_write() {
        assert_eq!(kind("SELECT 1; INSERT INTO t VALUES (1)"), QueryKind::Write);
    }

    #[test]
    fn multi_read_and_utility_is_utility() {
        // a SET in the batch should pull the whole batch to primary
        assert_eq!(
            kind("SELECT 1; SET search_path = public"),
            QueryKind::Utility
        );
    }

    // --- Parse failures ---
    #[test]
    fn garbage_is_unknown() {
        assert_eq!(kind("this is not sql"), QueryKind::Unknown);
        assert_eq!(kind("SELEC * FRM"), QueryKind::Unknown);
    }

    #[test]
    fn empty_is_unknown() {
        assert_eq!(kind(""), QueryKind::Unknown);
    }

    // --- CTEs ---
    #[test]
    fn read_only_cte_is_read() {
        assert_eq!(
            kind("WITH x AS (SELECT 1) SELECT * FROM x"),
            QueryKind::Read
        );
    }

    #[test]
    fn multiple_read_ctes_is_read() {
        assert_eq!(
            kind("WITH a AS (SELECT 1), b AS (SELECT 2) SELECT * FROM a, b"),
            QueryKind::Read
        );
    }

    #[test]
    fn cte_with_insert_is_write() {
        assert_eq!(
            kind("WITH x AS (INSERT INTO t VALUES (1) RETURNING id) SELECT * FROM x"),
            QueryKind::Write
        );
    }

    #[test]
    fn cte_with_update_is_write() {
        assert_eq!(
            kind("WITH x AS (UPDATE t SET a = 1 RETURNING id) SELECT * FROM x"),
            QueryKind::Write
        );
    }

    #[test]
    fn cte_with_delete_is_write() {
        assert_eq!(
            kind("WITH x AS (DELETE FROM t WHERE id = 1 RETURNING id) SELECT * FROM x"),
            QueryKind::Write
        );
    }

    #[test]
    fn mixed_ctes_one_write_is_write() {
        assert_eq!(
            kind(
                "WITH a AS (SELECT 1), b AS (INSERT INTO t VALUES (1) RETURNING id) SELECT * FROM a, b"
            ),
            QueryKind::Write
        );
    }

    #[test]
    fn nested_cte_with_inner_write_is_write() {
        assert_eq!(
            kind(
                "WITH a AS (WITH b AS (INSERT INTO t VALUES (1) RETURNING id) SELECT * FROM b) SELECT * FROM a"
            ),
            QueryKind::Write
        );
    }
}
