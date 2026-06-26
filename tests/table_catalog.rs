use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rs_redbase::storage::record::ScanCompOp;
use rs_redbase::table::{
    AttributeMeta, CatalogManager, ColumnSchema, ColumnType, Database, Expr, RelationMeta, Row,
    TableError, TableSchema, Value,
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
fn creates_inserts_scans_and_reopens_tables() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("table_roundtrip");
    let mut db = Database::open(&root)?;
    db.create_table(student_schema())?;

    {
        let mut table = db.open_table("student")?;
        table.insert(vec![
            Value::Int32(1),
            Value::Float32(91.5),
            Value::Char("alice".to_string()),
        ])?;
        table.insert(vec![
            Value::Int32(2),
            Value::Float32(85.0),
            Value::Char("bob".to_string()),
        ])?;
        table.insert(vec![
            Value::Int32(3),
            Value::Float32(77.5),
            Value::Char("cindy".to_string()),
        ])?;

        let rows = table.scan(Some(Expr::And(vec![
            Expr::CmpValue {
                column: "id".to_string(),
                op: ScanCompOp::Ge,
                value: Value::Int32(2),
            },
            Expr::CmpValue {
                column: "name".to_string(),
                op: ScanCompOp::Ne,
                value: Value::Char("bob".to_string()),
            },
        ])))?;
        assert_eq!(
            rows,
            vec![Row::new(vec![
                Value::Int32(3),
                Value::Float32(77.5),
                Value::Char("cindy".to_string()),
            ])]
        );
    }

    {
        let reopened = Database::open(&root)?;
        let mut table = reopened.open_table("student")?;
        let rows = table.scan(None)?;
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[1],
            Row::new(vec![
                Value::Int32(2),
                Value::Float32(85.0),
                Value::Char("bob".to_string()),
            ])
        );
    }

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn rejects_invalid_rows_and_unknown_columns() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("table_errors");
    let mut db = Database::open(&root)?;
    db.create_table(student_schema())?;

    {
        let mut table = db.open_table("student")?;
        let error = table
            .insert(vec![
                Value::Int32(1),
                Value::Char("oops".to_string()),
                Value::Char("alice".to_string()),
            ])
            .expect_err("row should fail type checking");
        assert!(matches!(
            error,
            TableError::ValueTypeMismatch { column, .. } if column == "score"
        ));

        let error = table
            .scan(Some(Expr::CmpValue {
                column: "missing".to_string(),
                op: ScanCompOp::Eq,
                value: Value::Int32(1),
            }))
            .expect_err("missing column should fail");
        assert!(matches!(error, TableError::ColumnNotFound(name) if name == "missing"));
    }

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn exposes_sm_style_relation_and_attribute_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("table_metadata");
    let mut db = Database::open(&root)?;
    db.create_table(student_schema())?;

    assert_eq!(db.list_tables(), vec!["student".to_string()]);
    assert_eq!(
        db.get_relation("student")?,
        RelationMeta {
            name: "student".to_string(),
            record_size: 16,
            column_count: 3,
        }
    );
    assert_eq!(
        db.get_attributes("student")?,
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
    assert_eq!(db.get_table_schema("student")?, student_schema());

    let catalog: CatalogManager = db.catalog();
    assert_eq!(catalog.list_tables(), vec!["student".to_string()]);
    assert!(matches!(
        catalog.get_relation("missing"),
        Err(TableError::TableNotFound(name)) if name == "missing"
    ));

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn rejects_corrupted_catalog_files() -> Result<(), Box<dyn std::error::Error>> {
    let root = unique_db_dir("table_corrupted_catalog");
    fs::create_dir_all(&root)?;
    fs::write(root.join("catalog.txt"), "broken-header\n")?;

    match Database::open(&root) {
        Err(TableError::CatalogCorrupted(_)) => {}
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("corrupted catalog should fail to load"),
    }

    fs::remove_dir_all(&root)?;
    Ok(())
}
