use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rs_redbase::query::{
    Assignment, CreateIndexRequest, CreateTableRequest, DeleteRequest, DeleteResult,
    DropIndexRequest, DropTableRequest, ExtendedInsertRequest, ExtendedSelectRequest, InsertInput,
    InsertRequest, NamedValue, OrderBy, Projection, QueryEngine, QueryError, SelectRequest,
    SelectResult, ShowIndexesResult, ShowTablesResult, SortDirection, UpdateRequest, UpdateResult,
};
use rs_redbase::storage::index::{IndexKeyType, IndexMeta};
use rs_redbase::storage::record::ScanCompOp;
use rs_redbase::table::{
    AttributeMeta, ColumnSchema, ColumnType, Database, Expr, RelationMeta, Row, TableSchema, Value,
};

fn unique_db_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("rs_redbase_{prefix}_{nanos}"))
}

fn student_schema() -> TableSchema {
    TableSchema {
        name: "student".to_string(),
        columns: vec![
            ColumnSchema {
                name: "id".to_string(),
                column_type: ColumnType::Int32,
            },
            ColumnSchema {
                name: "score".to_string(),
                column_type: ColumnType::Float32,
            },
            ColumnSchema {
                name: "name".to_string(),
                column_type: ColumnType::Char(8),
            },
        ],
    }
}

#[test]
fn ql_engine_inserts_and_selects_rows() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_roundtrip");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;

    let engine = QueryEngine::open(&root)?;
    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(1),
            Value::Float32(93.5),
            Value::Char("alice".to_string()),
        ],
    })?;
    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(2),
            Value::Float32(81.0),
            Value::Char("bob".to_string()),
        ],
    })?;

    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Ge,
            value: Value::Int32(2),
        }),
    })?;

    assert_eq!(
        result,
        SelectResult {
            schema: student_schema(),
            rows: vec![Row::new(vec![
                Value::Int32(2),
                Value::Float32(81.0),
                Value::Char("bob".to_string()),
            ])],
        }
    );

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_unknown_tables_columns_and_types() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_errors");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    let error = engine
        .insert_into(InsertRequest {
            table: "missing".to_string(),
            values: vec![Value::Int32(1)],
        })
        .expect_err("unknown table should fail");
    assert!(matches!(error, QueryError::UnknownTable(name) if name == "missing"));

    let error = engine
        .insert_into(InsertRequest {
            table: "student".to_string(),
            values: vec![
                Value::Int32(1),
                Value::Char("oops".to_string()),
                Value::Char("alice".to_string()),
            ],
        })
        .expect_err("type mismatch should fail");
    assert!(matches!(
        error,
        QueryError::ValueTypeMismatch { table, column, .. }
            if table == "student" && column == "score"
    ));

    let error = engine
        .select_from(SelectRequest {
            table: "student".to_string(),
            filter: Some(Expr::CmpValue {
                column: "missing".to_string(),
                op: ScanCompOp::Eq,
                value: Value::Int32(1),
            }),
        })
        .expect_err("unknown column should fail");
    assert!(matches!(
        error,
        QueryError::UnknownColumn { table, column }
            if table == "student" && column == "missing"
    ));

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_supports_named_insert_and_projection() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_projection");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    engine.execute_insert(ExtendedInsertRequest {
        table: "student".to_string(),
        input: InsertInput::Named(vec![
            NamedValue {
                column: "name".to_string(),
                value: Value::Char("alice".to_string()),
            },
            NamedValue {
                column: "id".to_string(),
                value: Value::Int32(1),
            },
            NamedValue {
                column: "score".to_string(),
                value: Value::Float32(93.5),
            },
        ]),
    })?;

    let result = engine.execute_select(ExtendedSelectRequest {
        table: "student".to_string(),
        filter: None,
        projection: Projection::Columns(vec!["name".to_string(), "id".to_string()]),
        order_by: Vec::new(),
        limit: None,
    })?;

    assert_eq!(
        result.projected_columns,
        vec![
            ColumnSchema {
                name: "name".to_string(),
                column_type: ColumnType::Char(8),
            },
            ColumnSchema {
                name: "id".to_string(),
                column_type: ColumnType::Int32,
            },
        ]
    );
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].values(),
        &[Value::Char("alice".to_string()), Value::Int32(1)]
    );

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_sorts_by_hidden_column_and_applies_limit() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_order_limit_projection");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    for (id, score, name) in [(1, 88.0, "alice"), (2, 91.5, "bob"), (3, 85.0, "carol")] {
        engine.insert_into(InsertRequest {
            table: "student".to_string(),
            values: vec![
                Value::Int32(id),
                Value::Float32(score),
                Value::Char(name.to_string()),
            ],
        })?;
    }

    let result = engine.execute_select(ExtendedSelectRequest {
        table: "student".to_string(),
        filter: None,
        projection: Projection::Columns(vec!["name".to_string()]),
        order_by: vec![OrderBy {
            column: "score".to_string(),
            direction: SortDirection::Desc,
        }],
        limit: Some(2),
    })?;

    assert_eq!(
        result.projected_columns,
        vec![ColumnSchema {
            name: "name".to_string(),
            column_type: ColumnType::Char(8),
        }]
    );
    assert_eq!(result.rows.len(), 2);
    assert_eq!(result.rows[0].values(), &[Value::Char("bob".to_string())]);
    assert_eq!(result.rows[1].values(), &[Value::Char("alice".to_string())]);

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_supports_multi_column_ordering() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_multi_order");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    for (id, score, name) in [(1, 88.0, "alice"), (2, 91.5, "bob"), (3, 88.0, "carol")] {
        engine.insert_into(InsertRequest {
            table: "student".to_string(),
            values: vec![
                Value::Int32(id),
                Value::Float32(score),
                Value::Char(name.to_string()),
            ],
        })?;
    }

    let result = engine.execute_select(ExtendedSelectRequest {
        table: "student".to_string(),
        filter: None,
        projection: Projection::Columns(vec!["id".to_string(), "score".to_string()]),
        order_by: vec![
            OrderBy {
                column: "score".to_string(),
                direction: SortDirection::Asc,
            },
            OrderBy {
                column: "id".to_string(),
                direction: SortDirection::Desc,
            },
        ],
        limit: None,
    })?;

    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows[0].values(),
        &[Value::Int32(3), Value::Float32(88.0)]
    );
    assert_eq!(
        result.rows[1].values(),
        &[Value::Int32(1), Value::Float32(88.0)]
    );
    assert_eq!(
        result.rows[2].values(),
        &[Value::Int32(2), Value::Float32(91.5)]
    );

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_invalid_named_insert_requests() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_named_errors");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    let error = engine
        .execute_insert(ExtendedInsertRequest {
            table: "student".to_string(),
            input: InsertInput::Named(vec![
                NamedValue {
                    column: "id".to_string(),
                    value: Value::Int32(1),
                },
                NamedValue {
                    column: "id".to_string(),
                    value: Value::Int32(2),
                },
                NamedValue {
                    column: "score".to_string(),
                    value: Value::Float32(93.5),
                },
                NamedValue {
                    column: "name".to_string(),
                    value: Value::Char("alice".to_string()),
                },
            ]),
        })
        .expect_err("duplicate named column should fail");
    assert!(matches!(
        error,
        QueryError::DuplicateRequestColumn { table, column }
            if table == "student" && column == "id"
    ));

    let error = engine
        .execute_insert(ExtendedInsertRequest {
            table: "student".to_string(),
            input: InsertInput::Named(vec![
                NamedValue {
                    column: "id".to_string(),
                    value: Value::Int32(1),
                },
                NamedValue {
                    column: "score".to_string(),
                    value: Value::Float32(93.5),
                },
            ]),
        })
        .expect_err("missing named column should fail");
    assert!(matches!(
        error,
        QueryError::MissingRequestColumn { table, column }
            if table == "student" && column == "name"
    ));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_invalid_projection_requests() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_projection_errors");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(1),
            Value::Float32(93.5),
            Value::Char("alice".to_string()),
        ],
    })?;

    let error = engine
        .execute_select(ExtendedSelectRequest {
            table: "student".to_string(),
            filter: None,
            projection: Projection::Columns(vec!["name".to_string(), "name".to_string()]),
            order_by: Vec::new(),
            limit: None,
        })
        .expect_err("duplicate projection column should fail");
    assert!(matches!(
        error,
        QueryError::DuplicateRequestColumn { table, column }
            if table == "student" && column == "name"
    ));

    let error = engine
        .execute_select(ExtendedSelectRequest {
            table: "student".to_string(),
            filter: None,
            projection: Projection::Columns(vec!["missing".to_string()]),
            order_by: Vec::new(),
            limit: None,
        })
        .expect_err("unknown projection column should fail");
    assert!(matches!(
        error,
        QueryError::UnknownColumn { table, column }
            if table == "student" && column == "missing"
    ));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_invalid_order_and_limit_requests() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_order_limit_errors");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(1),
            Value::Float32(93.5),
            Value::Char("alice".to_string()),
        ],
    })?;

    let error = engine
        .execute_select(ExtendedSelectRequest {
            table: "student".to_string(),
            filter: None,
            projection: Projection::All,
            order_by: vec![
                OrderBy {
                    column: "id".to_string(),
                    direction: SortDirection::Asc,
                },
                OrderBy {
                    column: "id".to_string(),
                    direction: SortDirection::Desc,
                },
            ],
            limit: None,
        })
        .expect_err("duplicate order column should fail");
    assert!(matches!(
        error,
        QueryError::DuplicateOrderColumn { table, column }
            if table == "student" && column == "id"
    ));

    let error = engine
        .execute_select(ExtendedSelectRequest {
            table: "student".to_string(),
            filter: None,
            projection: Projection::All,
            order_by: vec![OrderBy {
                column: "missing".to_string(),
                direction: SortDirection::Asc,
            }],
            limit: None,
        })
        .expect_err("unknown order column should fail");
    assert!(matches!(
        error,
        QueryError::UnknownColumn { table, column }
            if table == "student" && column == "missing"
    ));

    let error = engine
        .execute_select(ExtendedSelectRequest {
            table: "student".to_string(),
            filter: None,
            projection: Projection::All,
            order_by: vec![],
            limit: Some(0),
        })
        .expect_err("zero limit should fail");
    assert!(matches!(
        error,
        QueryError::InvalidLimit { table, limit }
            if table == "student" && limit == 0
    ));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_updates_matching_rows() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_update");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    for (id, score, name) in [(1, 88.0, "alice"), (2, 91.5, "bob"), (3, 85.0, "carol")] {
        engine.insert_into(InsertRequest {
            table: "student".to_string(),
            values: vec![
                Value::Int32(id),
                Value::Float32(score),
                Value::Char(name.to_string()),
            ],
        })?;
    }

    let result = engine.execute_update(UpdateRequest {
        table: "student".to_string(),
        assignments: vec![
            Assignment {
                column: "score".to_string(),
                value: Value::Float32(95.0),
            },
            Assignment {
                column: "name".to_string(),
                value: Value::Char("top".to_string()),
            },
        ],
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(1),
        }),
    })?;
    assert_eq!(result, UpdateResult { affected_rows: 1 });

    let result = engine.execute_select(ExtendedSelectRequest {
        table: "student".to_string(),
        filter: None,
        projection: Projection::Columns(vec![
            "id".to_string(),
            "score".to_string(),
            "name".to_string(),
        ]),
        order_by: vec![OrderBy {
            column: "id".to_string(),
            direction: SortDirection::Asc,
        }],
        limit: None,
    })?;
    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows[0].values(),
        &[
            Value::Int32(1),
            Value::Float32(95.0),
            Value::Char("top".to_string()),
        ]
    );
    assert_eq!(
        result.rows[1].values(),
        &[
            Value::Int32(2),
            Value::Float32(91.5),
            Value::Char("bob".to_string()),
        ]
    );

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_deletes_matching_rows_and_supports_delete_all()
-> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_delete");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    for (id, score, name) in [(1, 88.0, "alice"), (2, 91.5, "bob"), (3, 85.0, "carol")] {
        engine.insert_into(InsertRequest {
            table: "student".to_string(),
            values: vec![
                Value::Int32(id),
                Value::Float32(score),
                Value::Char(name.to_string()),
            ],
        })?;
    }

    let result = engine.execute_delete(DeleteRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Ge,
            value: Value::Int32(2),
        }),
    })?;
    assert_eq!(result, DeleteResult { affected_rows: 2 });

    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: None,
    })?;
    assert_eq!(
        result,
        SelectResult {
            schema: student_schema(),
            rows: vec![Row::new(vec![
                Value::Int32(1),
                Value::Float32(88.0),
                Value::Char("alice".to_string()),
            ])],
        }
    );

    let result = engine.execute_delete(DeleteRequest {
        table: "student".to_string(),
        filter: None,
    })?;
    assert_eq!(result, DeleteResult { affected_rows: 1 });

    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: None,
    })?;
    assert_eq!(
        result,
        SelectResult {
            schema: student_schema(),
            rows: Vec::new(),
        }
    );

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_shows_and_describes_tables() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_schema_reads");
    let mut database = Database::open(&root)?;
    database.create_table(student_schema())?;
    let engine = QueryEngine::open(&root)?;

    let tables = engine.show_tables()?;
    assert_eq!(
        tables,
        ShowTablesResult {
            tables: vec!["student".to_string()],
        }
    );

    let description = engine.describe_table("student")?;
    assert_eq!(
        description.relation,
        RelationMeta {
            name: "student".to_string(),
            record_size: 16,
            column_count: 3,
        }
    );
    assert_eq!(
        description.attributes,
        vec![
            AttributeMeta {
                relation_name: "student".to_string(),
                column_name: "id".to_string(),
                ordinal: 0,
                offset: 0,
                length: 4,
                column_type: ColumnType::Int32,
            },
            AttributeMeta {
                relation_name: "student".to_string(),
                column_name: "score".to_string(),
                ordinal: 1,
                offset: 4,
                length: 4,
                column_type: ColumnType::Float32,
            },
            AttributeMeta {
                relation_name: "student".to_string(),
                column_name: "name".to_string(),
                ordinal: 2,
                offset: 8,
                length: 8,
                column_type: ColumnType::Char(8),
            },
        ]
    );

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_describe_unknown_table() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_schema_missing");
    let database = Database::open(&root)?;
    let engine = QueryEngine::open(&root)?;

    let error = engine
        .describe_table("missing")
        .expect_err("unknown table should fail");
    assert!(matches!(error, QueryError::UnknownTable(name) if name == "missing"));

    drop(engine);
    drop(database);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_creates_and_drops_tables() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_ql_schema_write");
    let mut engine = QueryEngine::open(&root)?;

    engine.execute_create_table(CreateTableRequest {
        schema: student_schema(),
    })?;
    assert_eq!(
        engine.show_tables()?,
        ShowTablesResult {
            tables: vec!["student".to_string()],
        }
    );

    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(1),
            Value::Float32(93.5),
            Value::Char("alice".to_string()),
        ],
    })?;
    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: None,
    })?;
    assert_eq!(result.rows.len(), 1);

    engine.execute_drop_table(DropTableRequest {
        table: "student".to_string(),
    })?;
    assert_eq!(engine.show_tables()?, ShowTablesResult { tables: vec![] });

    let error = engine
        .select_from(SelectRequest {
            table: "student".to_string(),
            filter: None,
        })
        .expect_err("dropped table should no longer be queryable");
    assert!(matches!(error, QueryError::UnknownTable(name) if name == "student"));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_invalid_schema_management_requests() -> Result<(), Box<dyn std::error::Error>>
{
    let root = unique_db_dir("minimal_ql_schema_errors");
    let mut engine = QueryEngine::open(&root)?;

    engine.execute_create_table(CreateTableRequest {
        schema: student_schema(),
    })?;

    let error = engine
        .execute_create_table(CreateTableRequest {
            schema: student_schema(),
        })
        .expect_err("duplicate table should fail");
    assert!(matches!(
        error,
        QueryError::Table(rs_redbase::table::TableError::DuplicateTable(name))
            if name == "student"
    ));

    let error = engine
        .execute_drop_table(DropTableRequest {
            table: "missing".to_string(),
        })
        .expect_err("dropping unknown table should fail");
    assert!(matches!(error, QueryError::UnknownTable(name) if name == "missing"));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_creates_lists_and_drops_indexes() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_index_roundtrip");
    let mut engine = QueryEngine::open(&root)?;
    engine.execute_create_table(CreateTableRequest {
        schema: student_schema(),
    })?;

    engine.execute_create_index(CreateIndexRequest {
        name: "student_id_idx".to_string(),
        table: "student".to_string(),
        column: "id".to_string(),
    })?;
    assert_eq!(
        engine.show_indexes()?,
        ShowIndexesResult {
            indexes: vec![IndexMeta {
                name: "student_id_idx".to_string(),
                table: "student".to_string(),
                column: "id".to_string(),
                key_type: IndexKeyType::Int32,
            }],
        }
    );

    drop(engine);
    let mut reopened = QueryEngine::open(&root)?;
    assert_eq!(
        reopened.show_indexes()?,
        ShowIndexesResult {
            indexes: vec![IndexMeta {
                name: "student_id_idx".to_string(),
                table: "student".to_string(),
                column: "id".to_string(),
                key_type: IndexKeyType::Int32,
            }],
        }
    );

    reopened.execute_drop_index(DropIndexRequest {
        name: "student_id_idx".to_string(),
    })?;
    assert_eq!(
        reopened.show_indexes()?,
        ShowIndexesResult { indexes: vec![] }
    );

    drop(reopened);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_rejects_invalid_index_requests() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_index_errors");
    let mut engine = QueryEngine::open(&root)?;
    engine.execute_create_table(CreateTableRequest {
        schema: student_schema(),
    })?;

    engine.execute_create_index(CreateIndexRequest {
        name: "student_id_idx".to_string(),
        table: "student".to_string(),
        column: "id".to_string(),
    })?;

    let error = engine
        .execute_create_index(CreateIndexRequest {
            name: "student_id_idx".to_string(),
            table: "student".to_string(),
            column: "id".to_string(),
        })
        .expect_err("duplicate index should fail");
    assert!(matches!(error, QueryError::DuplicateIndex(name) if name == "student_id_idx"));

    let error = engine
        .execute_create_index(CreateIndexRequest {
            name: "student_name_idx".to_string(),
            table: "student".to_string(),
            column: "name".to_string(),
        })
        .expect_err("unsupported index type should fail");
    assert!(matches!(
        error,
        QueryError::UnsupportedIndexColumnType { table, column, actual }
            if table == "student" && column == "name" && actual == "Char"
    ));

    let error = engine
        .execute_drop_index(DropIndexRequest {
            name: "missing_idx".to_string(),
        })
        .expect_err("dropping unknown index should fail");
    assert!(matches!(error, QueryError::UnknownIndex(name) if name == "missing_idx"));

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn ql_engine_keeps_indexed_eq_queries_correct_after_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("minimal_index_query_path");
    let mut engine = QueryEngine::open(&root)?;
    engine.execute_create_table(CreateTableRequest {
        schema: student_schema(),
    })?;

    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(1),
            Value::Float32(88.0),
            Value::Char("alice".to_string()),
        ],
    })?;
    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(2),
            Value::Float32(91.5),
            Value::Char("bob".to_string()),
        ],
    })?;

    engine.execute_create_index(CreateIndexRequest {
        name: "student_id_idx".to_string(),
        table: "student".to_string(),
        column: "id".to_string(),
    })?;

    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(2),
        }),
    })?;
    assert_eq!(
        result.rows,
        vec![Row::new(vec![
            Value::Int32(2),
            Value::Float32(91.5),
            Value::Char("bob".to_string()),
        ])]
    );

    engine.insert_into(InsertRequest {
        table: "student".to_string(),
        values: vec![
            Value::Int32(3),
            Value::Float32(85.0),
            Value::Char("carol".to_string()),
        ],
    })?;
    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(3),
        }),
    })?;
    assert_eq!(
        result.rows,
        vec![Row::new(vec![
            Value::Int32(3),
            Value::Float32(85.0),
            Value::Char("carol".to_string()),
        ])]
    );

    engine.execute_update(UpdateRequest {
        table: "student".to_string(),
        assignments: vec![Assignment {
            column: "id".to_string(),
            value: Value::Int32(5),
        }],
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(2),
        }),
    })?;
    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(5),
        }),
    })?;
    assert_eq!(
        result.rows,
        vec![Row::new(vec![
            Value::Int32(5),
            Value::Float32(91.5),
            Value::Char("bob".to_string()),
        ])]
    );

    engine.execute_delete(DeleteRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(1),
        }),
    })?;
    let result = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(1),
        }),
    })?;
    assert!(result.rows.is_empty());

    drop(engine);
    fs::remove_dir_all(&root)?;
    Ok(())
}
