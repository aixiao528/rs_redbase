use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use rs_redbase::query::{
    Assignment, CreateIndexRequest, CreateTableRequest, DeleteRequest, DropIndexRequest,
    DropTableRequest, ExtendedSelectRequest, InsertRequest, OrderBy, Projection, QueryEngine,
    SelectRequest, SortDirection, UpdateRequest,
};
use rs_redbase::storage::record::ScanCompOp;
use rs_redbase::table::{ColumnSchema, ColumnType, Expr, TableSchema, Value};

fn main() -> Result<(), Box<dyn Error>> {
    run_demo()
}

fn run_demo() -> Result<(), Box<dyn Error>> {
    let root = demo_root();
    reset_demo_root(&root)?;

    print_step("Open Database");
    println!("database root: {}", root.display());
    let mut engine = QueryEngine::open(&root)?;

    print_step("Create Table");
    let schema = student_schema();
    engine.execute_create_table(CreateTableRequest {
        schema: schema.clone(),
    })?;
    println!("created table: {}", schema.name);

    print_step("Show Tables");
    let tables = engine.show_tables()?;
    println!("tables: {}", tables.tables.join(", "));

    print_step("Describe Table");
    let description = engine.describe_table("student")?;
    println!("relation: {}", description.relation.name);
    println!("record size: {}", description.relation.record_size);
    println!("column count: {}", description.relation.column_count);
    for attribute in description.attributes {
        println!(
            "column: {} type={} offset={} length={}",
            attribute.column_name,
            format_column_type(&attribute.column_type),
            attribute.offset,
            attribute.length
        );
    }

    print_step("Insert Rows");
    for row in demo_rows() {
        engine.insert_into(InsertRequest {
            table: "student".to_string(),
            values: row,
        })?;
    }
    println!("inserted rows: 4");

    print_step("Create Index");
    engine.execute_create_index(CreateIndexRequest {
        name: "student_id_idx".to_string(),
        table: "student".to_string(),
        column: "id".to_string(),
    })?;
    let indexes = engine.show_indexes()?;
    for index in indexes.indexes {
        println!("index: {} on {}.{}", index.name, index.table, index.column);
    }

    print_step("Index Equality Query");
    let equality = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(2),
        }),
    })?;
    print_rows(&equality.rows);

    print_step("Order By + Limit");
    let ranked = engine.execute_select(ExtendedSelectRequest {
        table: "student".to_string(),
        filter: None,
        projection: Projection::Columns(vec![
            "name".to_string(),
            "score".to_string(),
            "id".to_string(),
        ]),
        order_by: vec![OrderBy {
            column: "score".to_string(),
            direction: SortDirection::Desc,
        }],
        limit: Some(2),
    })?;
    print_projected_rows(&ranked.projected_columns, &ranked.rows);

    print_step("Update Row");
    let updated = engine.execute_update(UpdateRequest {
        table: "student".to_string(),
        assignments: vec![Assignment {
            column: "score".to_string(),
            value: Value::Float32(95.0),
        }],
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(2),
        }),
    })?;
    println!("updated rows: {}", updated.affected_rows);
    let after_update = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(2),
        }),
    })?;
    print_rows(&after_update.rows);

    print_step("Delete Row");
    let deleted = engine.execute_delete(DeleteRequest {
        table: "student".to_string(),
        filter: Some(Expr::CmpValue {
            column: "id".to_string(),
            op: ScanCompOp::Eq,
            value: Value::Int32(1),
        }),
    })?;
    println!("deleted rows: {}", deleted.affected_rows);
    let remaining = engine.select_from(SelectRequest {
        table: "student".to_string(),
        filter: None,
    })?;
    print_rows(&remaining.rows);

    print_step("Cleanup");
    engine.execute_drop_index(DropIndexRequest {
        name: "student_id_idx".to_string(),
    })?;
    engine.execute_drop_table(DropTableRequest {
        table: "student".to_string(),
    })?;
    drop(engine);
    fs::remove_dir_all(&root)?;
    println!("dropped index, dropped table, removed demo directory.");

    print_step("Demo Complete");
    println!("schema, CRUD, order/limit, and minimal index flow all completed.");
    Ok(())
}

fn demo_root() -> PathBuf {
    PathBuf::from("demo-db")
}

fn reset_demo_root(root: &Path) -> Result<(), Box<dyn Error>> {
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    Ok(())
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

fn demo_rows() -> Vec<Vec<Value>> {
    vec![
        vec![
            Value::Int32(1),
            Value::Float32(88.0),
            Value::Char("alice".to_string()),
        ],
        vec![
            Value::Int32(2),
            Value::Float32(91.5),
            Value::Char("bob".to_string()),
        ],
        vec![
            Value::Int32(3),
            Value::Float32(85.0),
            Value::Char("carol".to_string()),
        ],
        vec![
            Value::Int32(4),
            Value::Float32(91.5),
            Value::Char("david".to_string()),
        ],
    ]
}

fn format_column_type(column_type: &ColumnType) -> String {
    match column_type {
        ColumnType::Int32 => "Int32".to_string(),
        ColumnType::Float32 => "Float32".to_string(),
        ColumnType::Char(length) => format!("Char({length})"),
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::Int32(value) => value.to_string(),
        Value::Float32(value) => format!("{value:.1}"),
        Value::Char(value) => value.clone(),
    }
}

fn print_rows(rows: &[rs_redbase::table::Row]) {
    if rows.is_empty() {
        println!("rows: <empty>");
        return;
    }
    for row in rows {
        let values = row
            .values()
            .iter()
            .map(format_value)
            .collect::<Vec<_>>()
            .join(", ");
        println!("row: [{values}]");
    }
}

fn print_projected_rows(
    columns: &[rs_redbase::table::ColumnSchema],
    rows: &[rs_redbase::query::ProjectedRow],
) {
    let header = columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    println!("columns: [{header}]");
    if rows.is_empty() {
        println!("rows: <empty>");
        return;
    }
    for row in rows {
        let values = row
            .values()
            .iter()
            .map(format_value)
            .collect::<Vec<_>>()
            .join(", ");
        println!("row: [{values}]");
    }
}

fn print_step(title: &str) {
    println!();
    println!("== {title} ==");
}
