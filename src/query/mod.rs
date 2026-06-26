use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

use crate::storage::index::{IndexError, IndexMeta};
use crate::storage::record::{Rid, ScanCompOp};
use crate::table::{
    AttributeMeta, ColumnType, Database, Expr, RelationMeta, Row, TableError, TableSchema, Value,
};

pub type QueryResult<T> = Result<T, QueryError>;

#[derive(Debug)]
pub enum QueryError {
    Table(TableError),
    UnknownTable(String),
    DuplicateIndex(String),
    UnknownIndex(String),
    UnsupportedIndexColumnType {
        table: String,
        column: String,
        actual: &'static str,
    },
    UnknownColumn {
        table: String,
        column: String,
    },
    DuplicateUpdateColumn {
        table: String,
        column: String,
    },
    DuplicateOrderColumn {
        table: String,
        column: String,
    },
    DuplicateRequestColumn {
        table: String,
        column: String,
    },
    MissingRequestColumn {
        table: String,
        column: String,
    },
    InvalidLimit {
        table: String,
        limit: usize,
    },
    EmptyAssignments {
        table: String,
    },
    ValueCountMismatch {
        table: String,
        expected: usize,
        actual: usize,
    },
    ValueTypeMismatch {
        table: String,
        column: String,
        expected: &'static str,
        actual: &'static str,
    },
}

impl Display for QueryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Table(error) => write!(f, "{error}"),
            Self::UnknownTable(table) => write!(f, "unknown table: {table}"),
            Self::DuplicateIndex(name) => write!(f, "duplicate index: {name}"),
            Self::UnknownIndex(name) => write!(f, "unknown index: {name}"),
            Self::UnsupportedIndexColumnType {
                table,
                column,
                actual,
            } => write!(
                f,
                "unsupported index column type for {table}.{column}: {actual}"
            ),
            Self::UnknownColumn { table, column } => {
                write!(f, "unknown column {column} on table {table}")
            }
            Self::DuplicateUpdateColumn { table, column } => {
                write!(f, "duplicate update column {column} on table {table}")
            }
            Self::DuplicateOrderColumn { table, column } => {
                write!(f, "duplicate order column {column} on table {table}")
            }
            Self::DuplicateRequestColumn { table, column } => {
                write!(f, "duplicate request column {column} on table {table}")
            }
            Self::MissingRequestColumn { table, column } => {
                write!(f, "missing request column {column} on table {table}")
            }
            Self::InvalidLimit { table, limit } => {
                write!(f, "invalid limit {limit} for table {table}")
            }
            Self::EmptyAssignments { table } => {
                write!(f, "empty assignments for table {table}")
            }
            Self::ValueCountMismatch {
                table,
                expected,
                actual,
            } => write!(
                f,
                "value count mismatch for table {table}: expected {expected}, got {actual}"
            ),
            Self::ValueTypeMismatch {
                table,
                column,
                expected,
                actual,
            } => write!(
                f,
                "value type mismatch for {table}.{column}: expected {expected}, got {actual}"
            ),
        }
    }
}

impl Error for QueryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Table(error) => Some(error),
            Self::UnknownTable(_)
            | Self::DuplicateIndex(_)
            | Self::UnknownIndex(_)
            | Self::UnsupportedIndexColumnType { .. }
            | Self::UnknownColumn { .. }
            | Self::DuplicateUpdateColumn { .. }
            | Self::DuplicateOrderColumn { .. }
            | Self::DuplicateRequestColumn { .. }
            | Self::MissingRequestColumn { .. }
            | Self::InvalidLimit { .. }
            | Self::EmptyAssignments { .. }
            | Self::ValueCountMismatch { .. }
            | Self::ValueTypeMismatch { .. } => None,
        }
    }
}

impl From<TableError> for QueryError {
    fn from(value: TableError) -> Self {
        Self::Table(value)
    }
}

pub struct QueryEngine {
    database: Database,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InsertRequest {
    pub table: String,
    pub values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InsertInput {
    FullRow(Vec<Value>),
    Named(Vec<NamedValue>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedValue {
    pub column: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtendedInsertRequest {
    pub table: String,
    pub input: InsertInput,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub column: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectRequest {
    pub table: String,
    pub filter: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    All,
    Columns(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderBy {
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtendedSelectRequest {
    pub table: String,
    pub filter: Option<Expr>,
    pub projection: Projection,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateRequest {
    pub table: String,
    pub assignments: Vec<Assignment>,
    pub filter: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeleteRequest {
    pub table: String,
    pub filter: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateTableRequest {
    pub schema: TableSchema,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropTableRequest {
    pub table: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateIndexRequest {
    pub name: String,
    pub table: String,
    pub column: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropIndexRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectResult {
    pub schema: TableSchema,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateResult {
    pub affected_rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteResult {
    pub affected_rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowTablesResult {
    pub tables: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DescribeTableResult {
    pub relation: RelationMeta,
    pub attributes: Vec<AttributeMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowIndexesResult {
    pub indexes: Vec<IndexMeta>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedRow {
    values: Vec<Value>,
}

impl ProjectedRow {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedSelectResult {
    pub schema: TableSchema,
    pub projected_columns: Vec<crate::table::ColumnSchema>,
    pub rows: Vec<ProjectedRow>,
}

#[derive(Clone, Debug, PartialEq)]
struct ProjectionPlan {
    columns: Vec<crate::table::ColumnSchema>,
    indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct OrderKeyPlan {
    index: usize,
    direction: SortDirection,
}

#[derive(Clone, Debug, PartialEq)]
struct SelectPlan {
    projection: ProjectionPlan,
    order_by: Vec<OrderKeyPlan>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct NormalizedAssignment {
    index: usize,
    column: String,
    value: Value,
}

#[derive(Clone, Debug, PartialEq)]
struct UpdatePlan {
    assignments: Vec<NormalizedAssignment>,
}

#[derive(Clone, Debug, PartialEq)]
struct DeletePlan;

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexLookupPlan {
    index_name: String,
    key: i32,
}

impl QueryEngine {
    pub fn open(root: impl AsRef<Path>) -> QueryResult<Self> {
        Ok(Self {
            database: Database::open(root)?,
        })
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn insert_into(&self, request: InsertRequest) -> QueryResult<Rid> {
        self.execute_insert(ExtendedInsertRequest {
            table: request.table,
            input: InsertInput::FullRow(request.values),
        })
    }

    pub fn select_from(&self, request: SelectRequest) -> QueryResult<SelectResult> {
        let result = self.execute_select(ExtendedSelectRequest {
            table: request.table,
            filter: request.filter,
            projection: Projection::All,
            order_by: Vec::new(),
            limit: None,
        })?;
        Ok(SelectResult {
            schema: result.schema,
            rows: result
                .rows
                .into_iter()
                .map(|row| Row::new(row.values().to_vec()))
                .collect(),
        })
    }

    pub fn execute_insert(&self, request: ExtendedInsertRequest) -> QueryResult<Rid> {
        let schema = self.load_schema(&request.table)?;
        let values = normalize_insert_input(&request.table, &schema, request.input)?;
        let table_name = request.table;

        let mut table = self.database.open_table(&table_name)?;
        let rid = table.insert(values)?;
        drop(table);
        self.database.rebuild_indexes_for_table(&table_name)?;
        Ok(rid)
    }

    pub fn execute_create_table(&mut self, request: CreateTableRequest) -> QueryResult<()> {
        self.database.create_table(request.schema)?;
        Ok(())
    }

    pub fn execute_create_index(&mut self, request: CreateIndexRequest) -> QueryResult<()> {
        let schema = self.load_schema(&request.table)?;
        let column = schema
            .columns
            .iter()
            .find(|item| item.name == request.column)
            .ok_or_else(|| QueryError::UnknownColumn {
                table: request.table.clone(),
                column: request.column.clone(),
            })?;
        if !matches!(column.column_type, ColumnType::Int32) {
            return Err(QueryError::UnsupportedIndexColumnType {
                table: request.table,
                column: request.column,
                actual: column_type_name(&column.column_type),
            });
        }
        match self
            .database
            .create_index(&request.name, &request.table, &request.column)
        {
            Ok(_) => Ok(()),
            Err(TableError::Index(IndexError::DuplicateIndex(name))) => {
                Err(QueryError::DuplicateIndex(name))
            }
            Err(TableError::Index(IndexError::UnsupportedKeyType(_))) => {
                Err(QueryError::UnsupportedIndexColumnType {
                    table: request.table,
                    column: request.column,
                    actual: column_type_name(&column.column_type),
                })
            }
            Err(TableError::TableNotFound(_)) => Err(QueryError::UnknownTable(request.table)),
            Err(TableError::ColumnNotFound(_)) => Err(QueryError::UnknownColumn {
                table: request.table,
                column: request.column,
            }),
            Err(other) => Err(other.into()),
        }
    }

    pub fn execute_select(
        &self,
        request: ExtendedSelectRequest,
    ) -> QueryResult<ProjectedSelectResult> {
        let schema = self.load_schema(&request.table)?;
        if let Some(expr) = &request.filter {
            validate_expr_columns(&request.table, &schema, expr)?;
        }
        let SelectPlan {
            projection,
            order_by,
            limit,
        } = resolve_select_plan(&request.table, &schema, &request)?;
        let index_lookup = resolve_index_lookup(&request.table, &request.filter, &self.database);

        let mut table = self.database.open_table(&request.table)?;
        let mut rows = match index_lookup {
            Some(plan) => {
                let rids = self.database.lookup_index_eq(&plan.index_name, plan.key)?;
                table.rows_by_rids(&rids)?
            }
            None => table.scan(request.filter)?,
        };
        apply_order_by(&mut rows, &order_by);
        apply_limit(&mut rows, limit);
        let rows = rows
            .into_iter()
            .map(|row| project_row(&row, &projection))
            .collect();

        Ok(ProjectedSelectResult {
            schema,
            projected_columns: projection.columns,
            rows,
        })
    }

    pub fn execute_update(&self, request: UpdateRequest) -> QueryResult<UpdateResult> {
        let schema = self.load_schema(&request.table)?;
        let plan = resolve_update_plan(&request.table, &schema, &request)?;
        let table_name = request.table;
        let assignments = plan
            .assignments
            .iter()
            .map(|assignment| (assignment.column.clone(), assignment.value.clone()))
            .collect::<Vec<_>>();

        let mut table = self.database.open_table(&table_name)?;
        let affected_rows = table.update_where(request.filter, &assignments)?;
        drop(table);
        self.database.rebuild_indexes_for_table(&table_name)?;
        Ok(UpdateResult { affected_rows })
    }

    pub fn execute_delete(&self, request: DeleteRequest) -> QueryResult<DeleteResult> {
        let schema = self.load_schema(&request.table)?;
        let _plan = resolve_delete_plan(&request.table, &schema, &request)?;
        let table_name = request.table;

        let mut table = self.database.open_table(&table_name)?;
        let affected_rows = table.delete_where(request.filter)?;
        drop(table);
        self.database.rebuild_indexes_for_table(&table_name)?;
        Ok(DeleteResult { affected_rows })
    }

    pub fn execute_drop_table(&mut self, request: DropTableRequest) -> QueryResult<()> {
        match self.database.drop_table(&request.table) {
            Ok(()) => Ok(()),
            Err(TableError::TableNotFound(_)) => Err(QueryError::UnknownTable(request.table)),
            Err(other) => Err(other.into()),
        }
    }

    pub fn execute_drop_index(&mut self, request: DropIndexRequest) -> QueryResult<()> {
        match self.database.drop_index(&request.name) {
            Ok(()) => Ok(()),
            Err(TableError::Index(IndexError::IndexNotFound(name))) => {
                Err(QueryError::UnknownIndex(name))
            }
            Err(other) => Err(other.into()),
        }
    }

    pub fn show_tables(&self) -> QueryResult<ShowTablesResult> {
        Ok(ShowTablesResult {
            tables: self.database.list_tables(),
        })
    }

    pub fn show_indexes(&self) -> QueryResult<ShowIndexesResult> {
        Ok(ShowIndexesResult {
            indexes: self.database.list_indexes(),
        })
    }

    pub fn describe_table(&self, table: &str) -> QueryResult<DescribeTableResult> {
        let relation = match self.database.get_relation(table) {
            Ok(relation) => relation,
            Err(TableError::TableNotFound(_)) => {
                return Err(QueryError::UnknownTable(table.to_string()));
            }
            Err(other) => return Err(other.into()),
        };
        let attributes = match self.database.get_attributes(table) {
            Ok(attributes) => attributes,
            Err(TableError::TableNotFound(_)) => {
                return Err(QueryError::UnknownTable(table.to_string()));
            }
            Err(other) => return Err(other.into()),
        };
        Ok(DescribeTableResult {
            relation,
            attributes,
        })
    }

    fn load_schema(&self, table: &str) -> QueryResult<TableSchema> {
        match self.database.get_table_schema(table) {
            Ok(schema) => Ok(schema),
            Err(TableError::TableNotFound(_)) => Err(QueryError::UnknownTable(table.to_string())),
            Err(other) => Err(other.into()),
        }
    }
}

fn normalize_insert_input(
    table: &str,
    schema: &TableSchema,
    input: InsertInput,
) -> QueryResult<Vec<Value>> {
    match input {
        InsertInput::FullRow(values) => {
            validate_row_types(table, schema, &values)?;
            Ok(values)
        }
        InsertInput::Named(entries) => normalize_named_insert(table, schema, entries),
    }
}

fn normalize_named_insert(
    table: &str,
    schema: &TableSchema,
    entries: Vec<NamedValue>,
) -> QueryResult<Vec<Value>> {
    let mut seen = BTreeSet::new();
    let mut values_by_column = std::collections::BTreeMap::new();
    for entry in entries {
        require_column(schema, table, &entry.column)?;
        if !seen.insert(entry.column.clone()) {
            return Err(QueryError::DuplicateRequestColumn {
                table: table.to_string(),
                column: entry.column,
            });
        }
        values_by_column.insert(entry.column, entry.value);
    }

    let mut ordered = Vec::with_capacity(schema.columns.len());
    for column in &schema.columns {
        let value = values_by_column.remove(&column.name).ok_or_else(|| {
            QueryError::MissingRequestColumn {
                table: table.to_string(),
                column: column.name.clone(),
            }
        })?;
        if !value_matches_type(&column.column_type, &value) {
            return Err(QueryError::ValueTypeMismatch {
                table: table.to_string(),
                column: column.name.clone(),
                expected: column_type_name(&column.column_type),
                actual: value_type_name(&value),
            });
        }
        ordered.push(value);
    }

    Ok(ordered)
}

fn resolve_select_plan(
    table: &str,
    schema: &TableSchema,
    request: &ExtendedSelectRequest,
) -> QueryResult<SelectPlan> {
    Ok(SelectPlan {
        projection: resolve_projection(table, schema, &request.projection)?,
        order_by: resolve_order_by(table, schema, &request.order_by)?,
        limit: normalize_limit(table, request.limit)?,
    })
}

fn resolve_update_plan(
    table: &str,
    schema: &TableSchema,
    request: &UpdateRequest,
) -> QueryResult<UpdatePlan> {
    if let Some(expr) = &request.filter {
        validate_expr_columns(table, schema, expr)?;
    }
    Ok(UpdatePlan {
        assignments: normalize_assignments(table, schema, &request.assignments)?,
    })
}

fn resolve_delete_plan(
    table: &str,
    schema: &TableSchema,
    request: &DeleteRequest,
) -> QueryResult<DeletePlan> {
    if let Some(expr) = &request.filter {
        validate_expr_columns(table, schema, expr)?;
    }
    Ok(DeletePlan)
}

fn resolve_index_lookup(
    table: &str,
    filter: &Option<Expr>,
    database: &Database,
) -> Option<IndexLookupPlan> {
    match filter.as_ref()? {
        Expr::CmpValue {
            column,
            op: ScanCompOp::Eq,
            value: Value::Int32(key),
        } => database
            .find_index(table, column)
            .map(|index| IndexLookupPlan {
                index_name: index.name,
                key: *key,
            }),
        _ => None,
    }
}

fn normalize_assignments(
    table: &str,
    schema: &TableSchema,
    assignments: &[Assignment],
) -> QueryResult<Vec<NormalizedAssignment>> {
    if assignments.is_empty() {
        return Err(QueryError::EmptyAssignments {
            table: table.to_string(),
        });
    }

    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(assignments.len());
    for assignment in assignments {
        if !seen.insert(assignment.column.clone()) {
            return Err(QueryError::DuplicateUpdateColumn {
                table: table.to_string(),
                column: assignment.column.clone(),
            });
        }
        let (index, schema_column) = resolve_schema_column(schema, table, &assignment.column)?;
        if !value_matches_type(&schema_column.column_type, &assignment.value) {
            return Err(QueryError::ValueTypeMismatch {
                table: table.to_string(),
                column: assignment.column.clone(),
                expected: column_type_name(&schema_column.column_type),
                actual: value_type_name(&assignment.value),
            });
        }
        normalized.push(NormalizedAssignment {
            index,
            column: assignment.column.clone(),
            value: assignment.value.clone(),
        });
    }

    Ok(normalized)
}

fn resolve_projection(
    table: &str,
    schema: &TableSchema,
    projection: &Projection,
) -> QueryResult<ProjectionPlan> {
    match projection {
        Projection::All => Ok(ProjectionPlan {
            columns: schema.columns.clone(),
            indices: (0..schema.columns.len()).collect(),
        }),
        Projection::Columns(columns) => {
            let mut seen = BTreeSet::new();
            let mut resolved_columns = Vec::with_capacity(columns.len());
            let mut resolved_indices = Vec::with_capacity(columns.len());
            for column in columns {
                if !seen.insert(column.clone()) {
                    return Err(QueryError::DuplicateRequestColumn {
                        table: table.to_string(),
                        column: column.clone(),
                    });
                }
                let (index, schema_column) = resolve_schema_column(schema, table, column)?;
                resolved_indices.push(index);
                resolved_columns.push(schema_column.clone());
            }
            Ok(ProjectionPlan {
                columns: resolved_columns,
                indices: resolved_indices,
            })
        }
    }
}

fn resolve_order_by(
    table: &str,
    schema: &TableSchema,
    order_by: &[OrderBy],
) -> QueryResult<Vec<OrderKeyPlan>> {
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(order_by.len());
    for item in order_by {
        if !seen.insert(item.column.clone()) {
            return Err(QueryError::DuplicateOrderColumn {
                table: table.to_string(),
                column: item.column.clone(),
            });
        }
        let (index, _) = resolve_schema_column(schema, table, &item.column)?;
        resolved.push(OrderKeyPlan {
            index,
            direction: item.direction,
        });
    }
    Ok(resolved)
}

fn normalize_limit(table: &str, limit: Option<usize>) -> QueryResult<Option<usize>> {
    match limit {
        Some(0) => Err(QueryError::InvalidLimit {
            table: table.to_string(),
            limit: 0,
        }),
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn apply_order_by(rows: &mut [Row], order_by: &[OrderKeyPlan]) {
    if order_by.is_empty() {
        return;
    }

    // Sort on source rows before projection so hidden sort keys still work.
    rows.sort_by(|lhs, rhs| compare_rows(lhs, rhs, order_by));
}

fn compare_rows(lhs: &Row, rhs: &Row, order_by: &[OrderKeyPlan]) -> Ordering {
    for key in order_by {
        let ordering = compare_row_value_at(lhs, rhs, key.index);
        let ordering = match key.direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}

fn compare_row_value_at(lhs: &Row, rhs: &Row, index: usize) -> Ordering {
    let lhs_value = lhs
        .values()
        .get(index)
        .expect("order index derived from schema must exist in row");
    let rhs_value = rhs
        .values()
        .get(index)
        .expect("order index derived from schema must exist in row");
    compare_values(lhs_value, rhs_value)
}

fn compare_values(lhs: &Value, rhs: &Value) -> Ordering {
    match (lhs, rhs) {
        (Value::Int32(left), Value::Int32(right)) => left.cmp(right),
        (Value::Float32(left), Value::Float32(right)) => left.total_cmp(right),
        (Value::Char(left), Value::Char(right)) => left.cmp(right),
        _ => Ordering::Equal,
    }
}

fn apply_limit(rows: &mut Vec<Row>, limit: Option<usize>) {
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
}

fn project_row(row: &Row, projection: &ProjectionPlan) -> ProjectedRow {
    let values = projection
        .indices
        .iter()
        .map(|index| value_from_row(row, *index))
        .collect();
    ProjectedRow::new(values)
}

fn value_from_row(row: &Row, index: usize) -> Value {
    row.values()
        .get(index)
        .cloned()
        .expect("projection index derived from schema must exist in row")
}

fn resolve_schema_column<'a>(
    schema: &'a TableSchema,
    table: &str,
    column: &str,
) -> QueryResult<(usize, &'a crate::table::ColumnSchema)> {
    schema
        .columns
        .iter()
        .enumerate()
        .find(|(_, item)| item.name == column)
        .ok_or_else(|| QueryError::UnknownColumn {
            table: table.to_string(),
            column: column.to_string(),
        })
}

fn validate_row_types(table: &str, schema: &TableSchema, values: &[Value]) -> QueryResult<()> {
    if values.len() != schema.columns.len() {
        return Err(QueryError::ValueCountMismatch {
            table: table.to_string(),
            expected: schema.columns.len(),
            actual: values.len(),
        });
    }

    for (column, value) in schema.columns.iter().zip(values) {
        if value_matches_type(&column.column_type, value) {
            continue;
        }

        return Err(QueryError::ValueTypeMismatch {
            table: table.to_string(),
            column: column.name.clone(),
            expected: column_type_name(&column.column_type),
            actual: value_type_name(value),
        });
    }

    Ok(())
}

fn validate_expr_columns(table: &str, schema: &TableSchema, expr: &Expr) -> QueryResult<()> {
    match expr {
        Expr::CmpValue { column, .. } => {
            require_column(schema, table, column)?;
        }
        Expr::CmpColumns { lhs, rhs, .. } => {
            require_column(schema, table, lhs)?;
            require_column(schema, table, rhs)?;
        }
        Expr::And(children) | Expr::Or(children) => {
            for child in children {
                validate_expr_columns(table, schema, child)?;
            }
        }
        Expr::Not(child) => validate_expr_columns(table, schema, child)?,
    }

    Ok(())
}

fn require_column(schema: &TableSchema, table: &str, column: &str) -> QueryResult<()> {
    if schema.columns.iter().any(|item| item.name == column) {
        Ok(())
    } else {
        Err(QueryError::UnknownColumn {
            table: table.to_string(),
            column: column.to_string(),
        })
    }
}

fn value_matches_type(column_type: &ColumnType, value: &Value) -> bool {
    matches!(
        (column_type, value),
        (ColumnType::Int32, Value::Int32(_))
            | (ColumnType::Float32, Value::Float32(_))
            | (ColumnType::Char(_), Value::Char(_))
    )
}

fn column_type_name(column_type: &ColumnType) -> &'static str {
    match column_type {
        ColumnType::Int32 => "Int32",
        ColumnType::Float32 => "Float32",
        ColumnType::Char(_) => "Char",
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Int32(_) => "Int32",
        Value::Float32(_) => "Float32",
        Value::Char(_) => "Char",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::record::ScanCompOp;
    use crate::table::{ColumnSchema, ColumnType};

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
    fn update_plan_rejects_empty_assignments() {
        let schema = student_schema();
        let error = resolve_update_plan(
            "student",
            &schema,
            &UpdateRequest {
                table: "student".to_string(),
                assignments: Vec::new(),
                filter: None,
            },
        )
        .expect_err("empty assignments should fail");
        assert!(matches!(
            error,
            QueryError::EmptyAssignments { table } if table == "student"
        ));
    }

    #[test]
    fn update_plan_rejects_duplicate_columns() {
        let schema = student_schema();
        let error = resolve_update_plan(
            "student",
            &schema,
            &UpdateRequest {
                table: "student".to_string(),
                assignments: vec![
                    Assignment {
                        column: "score".to_string(),
                        value: Value::Float32(95.0),
                    },
                    Assignment {
                        column: "score".to_string(),
                        value: Value::Float32(96.0),
                    },
                ],
                filter: None,
            },
        )
        .expect_err("duplicate assignment should fail");
        assert!(matches!(
            error,
            QueryError::DuplicateUpdateColumn { table, column }
                if table == "student" && column == "score"
        ));
    }

    #[test]
    fn update_plan_rejects_type_mismatches() {
        let schema = student_schema();
        let error = resolve_update_plan(
            "student",
            &schema,
            &UpdateRequest {
                table: "student".to_string(),
                assignments: vec![Assignment {
                    column: "score".to_string(),
                    value: Value::Char("oops".to_string()),
                }],
                filter: None,
            },
        )
        .expect_err("type mismatch should fail");
        assert!(matches!(
            error,
            QueryError::ValueTypeMismatch { table, column, .. }
                if table == "student" && column == "score"
        ));
    }

    #[test]
    fn update_plan_rejects_unknown_filter_columns() {
        let schema = student_schema();
        let error = resolve_update_plan(
            "student",
            &schema,
            &UpdateRequest {
                table: "student".to_string(),
                assignments: vec![Assignment {
                    column: "score".to_string(),
                    value: Value::Float32(95.0),
                }],
                filter: Some(Expr::CmpValue {
                    column: "missing".to_string(),
                    op: ScanCompOp::Eq,
                    value: Value::Int32(1),
                }),
            },
        )
        .expect_err("unknown filter column should fail");
        assert!(matches!(
            error,
            QueryError::UnknownColumn { table, column }
                if table == "student" && column == "missing"
        ));
    }

    #[test]
    fn delete_plan_rejects_unknown_filter_columns() {
        let schema = student_schema();
        let error = resolve_delete_plan(
            "student",
            &schema,
            &DeleteRequest {
                table: "student".to_string(),
                filter: Some(Expr::CmpValue {
                    column: "missing".to_string(),
                    op: ScanCompOp::Eq,
                    value: Value::Int32(1),
                }),
            },
        )
        .expect_err("unknown delete filter column should fail");
        assert!(matches!(
            error,
            QueryError::UnknownColumn { table, column }
                if table == "student" && column == "missing"
        ));
    }

    #[test]
    fn update_plan_normalizes_assignments_to_schema_order() -> QueryResult<()> {
        let schema = student_schema();
        let plan = resolve_update_plan(
            "student",
            &schema,
            &UpdateRequest {
                table: "student".to_string(),
                assignments: vec![
                    Assignment {
                        column: "name".to_string(),
                        value: Value::Char("alice".to_string()),
                    },
                    Assignment {
                        column: "score".to_string(),
                        value: Value::Float32(95.0),
                    },
                ],
                filter: None,
            },
        )?;

        assert_eq!(
            plan.assignments,
            vec![
                NormalizedAssignment {
                    index: 2,
                    column: "name".to_string(),
                    value: Value::Char("alice".to_string()),
                },
                NormalizedAssignment {
                    index: 1,
                    column: "score".to_string(),
                    value: Value::Float32(95.0),
                },
            ]
        );
        Ok(())
    }
}
