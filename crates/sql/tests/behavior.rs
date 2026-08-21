use microsystem_sql::{ColumnInfo, Database, Error, Execution, SqlType, Value};

fn command(db: &mut Database, sql: &str, affected_rows: u32) {
    assert_eq!(db.execute(sql), Ok(Execution::Command { affected_rows }));
}

#[test]
fn crud_lifecycle_supports_named_inserts_and_drop() {
    let mut db = Database::new();

    command(
        &mut db,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active BOOL)",
        0,
    );
    command(
        &mut db,
        "INSERT INTO users (name, id, active) VALUES ('Alice', 1, TRUE)",
        1,
    );
    command(&mut db, "INSERT INTO users VALUES (2, 'Bob', FALSE);", 1);

    assert_eq!(
        db.execute("SELECT id, name, active FROM users WHERE id >= 1 AND active IS NOT NULL"),
        Ok(Execution::Rows {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    ty: SqlType::Integer,
                },
                ColumnInfo {
                    name: "name".into(),
                    ty: SqlType::Text,
                },
                ColumnInfo {
                    name: "active".into(),
                    ty: SqlType::Bool,
                },
            ],
            rows: vec![
                vec![
                    Value::Integer(1),
                    Value::Text("Alice".into()),
                    Value::Bool(true)
                ],
                vec![
                    Value::Integer(2),
                    Value::Text("Bob".into()),
                    Value::Bool(false)
                ],
            ],
        })
    );

    command(
        &mut db,
        "UPDATE users SET name = 'Alice Cooper', active = FALSE WHERE id = 1",
        1,
    );
    assert_eq!(
        db.execute("SELECT * FROM users WHERE id = 1"),
        Ok(Execution::Rows {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    ty: SqlType::Integer,
                },
                ColumnInfo {
                    name: "name".into(),
                    ty: SqlType::Text,
                },
                ColumnInfo {
                    name: "active".into(),
                    ty: SqlType::Bool,
                },
            ],
            rows: vec![vec![
                Value::Integer(1),
                Value::Text("Alice Cooper".into()),
                Value::Bool(false),
            ]],
        })
    );

    command(&mut db, "DELETE FROM users WHERE active = FALSE", 2);
    assert_eq!(
        db.execute("SELECT * FROM users"),
        Ok(Execution::Rows {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    ty: SqlType::Integer,
                },
                ColumnInfo {
                    name: "name".into(),
                    ty: SqlType::Text,
                },
                ColumnInfo {
                    name: "active".into(),
                    ty: SqlType::Bool,
                },
            ],
            rows: Vec::new(),
        })
    );

    command(&mut db, "DROP TABLE users", 0);
    assert_eq!(db.execute("SELECT * FROM users"), Err(Error::TableNotFound));
}

#[test]
fn types_and_key_constraints_reject_invalid_rows_and_updates() {
    let mut db = Database::new();
    assert_eq!(
        db.execute(
            "CREATE TABLE invalid_keys (first INTEGER PRIMARY KEY, second INTEGER PRIMARY KEY)"
        ),
        Err(Error::Constraint)
    );
    command(
        &mut db,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, title TEXT NOT NULL, enabled BOOL)",
        0,
    );
    command(&mut db, "INSERT INTO items VALUES (1, 'first', TRUE)", 1);

    assert_eq!(
        db.execute("INSERT INTO items VALUES ('wrong', 'text', TRUE)"),
        Err(Error::TypeMismatch)
    );
    assert_eq!(
        db.execute("INSERT INTO items (id, enabled) VALUES (2, FALSE)"),
        Err(Error::Constraint)
    );
    assert_eq!(
        db.execute("INSERT INTO items VALUES (1, 'duplicate', FALSE)"),
        Err(Error::Constraint)
    );
    assert_eq!(
        db.execute("INSERT INTO items VALUES (NULL, 'null id', FALSE)"),
        Err(Error::Constraint)
    );
    assert_eq!(
        db.execute("INSERT INTO items VALUES (2, 'wrong bool', 1)"),
        Err(Error::TypeMismatch)
    );
    command(&mut db, "INSERT INTO items VALUES (2, 'second', FALSE)", 1);
    assert_eq!(
        db.execute("UPDATE items SET title = NULL WHERE id = 1"),
        Err(Error::Constraint)
    );
    assert_eq!(
        db.execute("UPDATE items SET enabled = 1 WHERE id = 1"),
        Err(Error::TypeMismatch)
    );
    assert_eq!(
        db.execute("UPDATE items SET id = 2 WHERE id = 1"),
        Err(Error::Constraint)
    );

    assert_eq!(
        db.execute("SELECT * FROM items"),
        Ok(Execution::Rows {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    ty: SqlType::Integer,
                },
                ColumnInfo {
                    name: "title".into(),
                    ty: SqlType::Text,
                },
                ColumnInfo {
                    name: "enabled".into(),
                    ty: SqlType::Bool,
                },
            ],
            rows: vec![
                vec![
                    Value::Integer(1),
                    Value::Text("first".into()),
                    Value::Bool(true),
                ],
                vec![
                    Value::Integer(2),
                    Value::Text("second".into()),
                    Value::Bool(false),
                ],
            ],
        })
    );
}

#[test]
fn where_predicates_distinguish_null_and_non_null_values() {
    let mut db = Database::new();
    command(
        &mut db,
        "CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT, enabled BOOL)",
        0,
    );
    command(&mut db, "INSERT INTO records VALUES (1, NULL, TRUE)", 1);
    command(&mut db, "INSERT INTO records VALUES (2, 'ready', NULL)", 1);
    command(&mut db, "INSERT INTO records VALUES (3, NULL, NULL)", 1);

    assert_eq!(
        db.execute("DELETE FROM records WHERE missing = 1"),
        Err(Error::ColumnNotFound)
    );
    assert_eq!(
        db.execute("SELECT id FROM records"),
        Ok(Execution::Rows {
            columns: vec![ColumnInfo {
                name: "id".into(),
                ty: SqlType::Integer,
            }],
            rows: vec![
                vec![Value::Integer(1)],
                vec![Value::Integer(2)],
                vec![Value::Integer(3)],
            ],
        })
    );

    assert_eq!(
        db.execute("SELECT id FROM records WHERE note IS NULL"),
        Ok(Execution::Rows {
            columns: vec![ColumnInfo {
                name: "id".into(),
                ty: SqlType::Integer,
            }],
            rows: vec![vec![Value::Integer(1)], vec![Value::Integer(3)]],
        })
    );
    assert_eq!(
        db.execute("SELECT id FROM records WHERE note IS NOT NULL"),
        Ok(Execution::Rows {
            columns: vec![ColumnInfo {
                name: "id".into(),
                ty: SqlType::Integer,
            }],
            rows: vec![vec![Value::Integer(2)]],
        })
    );

    command(
        &mut db,
        "UPDATE records SET enabled = FALSE WHERE enabled IS NULL",
        2,
    );
    command(
        &mut db,
        "DELETE FROM records WHERE note IS NULL AND enabled = FALSE",
        1,
    );
    assert_eq!(
        db.execute("SELECT id FROM records"),
        Ok(Execution::Rows {
            columns: vec![ColumnInfo {
                name: "id".into(),
                ty: SqlType::Integer,
            }],
            rows: vec![vec![Value::Integer(1)], vec![Value::Integer(2)]],
        })
    );
}

#[test]
fn snapshots_round_trip_and_reject_corruption_or_unknown_versions() {
    let mut db = Database::new();
    command(
        &mut db,
        "CREATE TABLE snapshot_data (id INTEGER PRIMARY KEY, label TEXT, ready BOOL)",
        0,
    );
    command(
        &mut db,
        "INSERT INTO snapshot_data VALUES (7, 'persisted', FALSE)",
        1,
    );

    let mut snapshot = Vec::new();
    db.encode_snapshot(&mut snapshot).unwrap();
    let restored = Database::decode_snapshot(&snapshot).unwrap();
    assert_eq!(restored, db);
    assert_eq!(
        restored.clone().execute("SELECT * FROM snapshot_data"),
        Ok(Execution::Rows {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    ty: SqlType::Integer,
                },
                ColumnInfo {
                    name: "label".into(),
                    ty: SqlType::Text,
                },
                ColumnInfo {
                    name: "ready".into(),
                    ty: SqlType::Bool,
                },
            ],
            rows: vec![vec![
                Value::Integer(7),
                Value::Text("persisted".into()),
                Value::Bool(false),
            ]],
        })
    );

    let mut corrupt = snapshot.clone();
    let last_byte = corrupt.len() - 1;
    corrupt[last_byte] ^= 0x80;
    assert_eq!(Database::decode_snapshot(&corrupt), Err(Error::Corrupt));

    let mut unsupported_version = snapshot;
    unsupported_version[8..10].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(
        Database::decode_snapshot(&unsupported_version),
        Err(Error::Version)
    );
}
