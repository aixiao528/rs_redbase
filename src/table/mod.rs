use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::storage::index::{
    IndexCatalog, IndexEntry, IndexError, IndexKeyType, IndexMeta, create_index_file,
    delete_index_file, lookup_eq, replace_index_entries,
};
use crate::storage::page::DiskPageIo;
use crate::storage::record::{
    LogicFilter, PredicateClause, Record, RecordError, RecordFile, RecordManager, Rid, ScanCompOp,
    ScanFieldRef, ScanValue,
};

const CATALOG_FILE_NAME: &str = "catalog.txt";
const TABLE_FILE_EXTENSION: &str = "tbl";
const CATALOG_HEADER: &str = "rs-redbase-catalog-v1";

pub type TableResult<T> = Result<T, TableError>;

#[derive(Clone, Debug, PartialEq)]
pub enum ColumnType {
    Int32,
    Float32,
    Char(usize),
}

impl ColumnType {
    fn fixed_len(&self) -> usize {
        match self {
            Self::Int32 => std::mem::size_of::<i32>(),
            Self::Float32 => std::mem::size_of::<f32>(),
            Self::Char(len) => *len,
        }
    }

    fn kind_name(&self) -> &'static str {
        match self {
            Self::Int32 => "Int32",
            Self::Float32 => "Float32",
            Self::Char(_) => "Char",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnSchema {
    pub name: String,
    pub column_type: ColumnType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int32(i32),
    Float32(f32),
    Char(String),
}

impl Value {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Int32(_) => "Int32",
            Self::Float32(_) => "Float32",
            Self::Char(_) => "Char",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    values: Vec<Value>,
}

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    CmpValue {
        column: String,
        op: ScanCompOp,
        value: Value,
    },
    CmpColumns {
        lhs: String,
        op: ScanCompOp,
        rhs: String,
    },
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug)]
pub enum TableError {
    Io(io::Error),
    Index(IndexError),
    Record(RecordError),
    InvalidName(String),
    InvalidSchema(&'static str),
    DuplicateTable(String),
    DuplicateColumn(String),
    TableNotFound(String),
    ColumnNotFound(String),
    ValueCountMismatch {
        expected: usize,
        actual: usize,
    },
    ValueTypeMismatch {
        column: String,
        expected: &'static str,
        actual: &'static str,
    },
    CatalogCorrupted(&'static str),
}

impl Display for TableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Index(error) => write!(f, "{error}"),
            Self::Record(error) => write!(f, "{error}"),
            Self::InvalidName(name) => write!(f, "invalid identifier: {name}"),
            Self::InvalidSchema(message) => write!(f, "invalid schema: {message}"),
            Self::DuplicateTable(name) => write!(f, "table already exists: {name}"),
            Self::DuplicateColumn(name) => write!(f, "duplicate column: {name}"),
            Self::TableNotFound(name) => write!(f, "table not found: {name}"),
            Self::ColumnNotFound(name) => write!(f, "column not found: {name}"),
            Self::ValueCountMismatch { expected, actual } => {
                write!(
                    f,
                    "row value count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::ValueTypeMismatch {
                column,
                expected,
                actual,
            } => write!(
                f,
                "value type mismatch for column {column}: expected {expected}, got {actual}"
            ),
            Self::CatalogCorrupted(message) => write!(f, "catalog corrupted: {message}"),
        }
    }
}

impl Error for TableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Index(error) => Some(error),
            Self::Record(error) => Some(error),
            Self::InvalidName(_)
            | Self::InvalidSchema(_)
            | Self::DuplicateTable(_)
            | Self::DuplicateColumn(_)
            | Self::TableNotFound(_)
            | Self::ColumnNotFound(_)
            | Self::ValueCountMismatch { .. }
            | Self::ValueTypeMismatch { .. }
            | Self::CatalogCorrupted(_) => None,
        }
    }
}

impl From<io::Error> for TableError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<IndexError> for TableError {
    fn from(value: IndexError) -> Self {
        Self::Index(value)
    }
}

impl From<RecordError> for TableError {
    fn from(value: RecordError) -> Self {
        Self::Record(value)
    }
}

pub struct Database {
    root: PathBuf,
    catalog: Catalog,
    indexes: IndexCatalog,
}

#[derive(Clone)]
pub struct CatalogManager {
    catalog: Catalog,
}

pub struct Table {
    schema: TableSchema,
    layout: SchemaLayout,
    file: RecordFile<DiskPageIo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationMeta {
    pub name: String,
    pub record_size: usize,
    pub column_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributeMeta {
    pub relation_name: String,
    pub column_name: String,
    pub ordinal: usize,
    pub offset: usize,
    pub length: usize,
    pub column_type: ColumnType,
}

impl Database {
    pub fn open(root: impl AsRef<Path>) -> TableResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let catalog = Catalog::load(&catalog_path(&root))?;
        let indexes = IndexCatalog::load(&root)?;
        Ok(Self {
            root,
            catalog,
            indexes,
        })
    }

    pub fn create_table(&mut self, schema: TableSchema) -> TableResult<()> {
        let layout = compile_schema(&schema)?;
        if self.catalog.tables.contains_key(&schema.name) {
            return Err(TableError::DuplicateTable(schema.name));
        }

        let path = table_path(&self.root, &schema.name);
        RecordManager::create_file(&path, layout.record_size)?;
        self.catalog.tables.insert(schema.name.clone(), schema);
        self.catalog.save(&catalog_path(&self.root))?;
        Ok(())
    }

    pub fn drop_table(&mut self, name: &str) -> TableResult<()> {
        if !self.catalog.tables.contains_key(name) {
            return Err(TableError::TableNotFound(name.to_string()));
        }

        for index in self.indexes.indexes_for_table(name) {
            delete_index_file(&self.root, &index.name)?;
            self.indexes.remove(&index.name)?;
        }
        let path = table_path(&self.root, name);
        fs::remove_file(&path)?;
        self.catalog.tables.remove(name);
        self.catalog.save(&catalog_path(&self.root))?;
        self.indexes.save(&self.root)?;
        Ok(())
    }

    pub fn create_index(
        &mut self,
        name: &str,
        table: &str,
        column: &str,
    ) -> TableResult<IndexMeta> {
        validate_identifier(name)?;
        let schema = self
            .catalog
            .tables
            .get(table)
            .ok_or_else(|| TableError::TableNotFound(table.to_string()))?;
        let layout = compile_schema(schema)?;
        let layout_column = layout.column(column)?;
        let key_type = match layout_column.column_type {
            ColumnType::Int32 => IndexKeyType::Int32,
            _ => {
                return Err(
                    IndexError::UnsupportedKeyType(layout_column.column_type.kind_name()).into(),
                );
            }
        };
        let meta = IndexMeta {
            name: name.to_string(),
            table: table.to_string(),
            column: column.to_string(),
            key_type,
        };
        create_index_file(&self.root, &meta)?;
        self.indexes.insert(meta.clone())?;
        self.indexes.save(&self.root)?;
        self.rebuild_index(&meta.name)?;
        Ok(meta)
    }

    pub fn drop_index(&mut self, name: &str) -> TableResult<()> {
        let meta = self.indexes.remove(name)?;
        delete_index_file(&self.root, &meta.name)?;
        self.indexes.save(&self.root)?;
        Ok(())
    }

    pub fn list_indexes(&self) -> Vec<IndexMeta> {
        self.indexes.list()
    }

    pub fn find_index(&self, table: &str, column: &str) -> Option<IndexMeta> {
        self.indexes
            .indexes_for_table(table)
            .into_iter()
            .find(|meta| meta.column == column)
    }

    pub fn lookup_index_eq(&self, name: &str, key: i32) -> TableResult<Vec<Rid>> {
        Ok(lookup_eq(&self.root, name, key)?)
    }

    pub fn rebuild_indexes_for_table(&self, table: &str) -> TableResult<()> {
        for index in self.indexes.indexes_for_table(table) {
            self.rebuild_index(&index.name)?;
        }
        Ok(())
    }

    fn rebuild_index(&self, name: &str) -> TableResult<()> {
        let index = self
            .indexes
            .list()
            .into_iter()
            .find(|meta| meta.name == name)
            .ok_or_else(|| TableError::Index(IndexError::IndexNotFound(name.to_string())))?;
        let mut table = self.open_table(&index.table)?;
        let entries = match index.key_type {
            IndexKeyType::Int32 => table.index_entries_for_int32_column(&index.column)?,
        };
        replace_index_entries(&self.root, &index.name, &entries)?;
        Ok(())
    }

    pub fn open_table(&self, name: &str) -> TableResult<Table> {
        let schema = self
            .catalog
            .tables
            .get(name)
            .cloned()
            .ok_or_else(|| TableError::TableNotFound(name.to_string()))?;
        let layout = compile_schema(&schema)?;
        let file = RecordManager::open_file(table_path(&self.root, name))?;
        Ok(Table {
            schema,
            layout,
            file,
        })
    }

    pub fn catalog(&self) -> CatalogManager {
        CatalogManager {
            catalog: self.catalog.clone(),
        }
    }

    pub fn list_tables(&self) -> Vec<String> {
        self.catalog.list_tables()
    }

    pub fn get_relation(&self, name: &str) -> TableResult<RelationMeta> {
        self.catalog.get_relation(name)
    }

    pub fn get_attributes(&self, name: &str) -> TableResult<Vec<AttributeMeta>> {
        self.catalog.get_attributes(name)
    }

    pub fn get_table_schema(&self, name: &str) -> TableResult<TableSchema> {
        self.catalog.get_table_schema(name)
    }
}

impl CatalogManager {
    pub fn list_tables(&self) -> Vec<String> {
        self.catalog.list_tables()
    }

    pub fn get_relation(&self, name: &str) -> TableResult<RelationMeta> {
        self.catalog.get_relation(name)
    }

    pub fn get_attributes(&self, name: &str) -> TableResult<Vec<AttributeMeta>> {
        self.catalog.get_attributes(name)
    }

    pub fn get_table_schema(&self, name: &str) -> TableResult<TableSchema> {
        self.catalog.get_table_schema(name)
    }
}

impl Table {
    pub fn schema(&self) -> &TableSchema {
        &self.schema
    }

    pub fn insert(&mut self, values: Vec<Value>) -> TableResult<Rid> {
        let bytes = encode_row(&self.layout, &values)?;
        Ok(self.file.insert(&bytes)?)
    }

    pub fn scan(&mut self, expr: Option<Expr>) -> TableResult<Vec<Row>> {
        let mut rows = Vec::new();
        match expr {
            None => {
                let mut scan = self.file.scan();
                while let Some(record) = scan.next_record()? {
                    rows.push(decode_record(&self.layout, &record)?);
                }
            }
            Some(expr) => {
                let filter = compile_expr(&self.layout, &expr)?;
                let mut scan = self.file.scan_with_logic(filter)?;
                while let Some(record) = scan.next_record()? {
                    rows.push(decode_record(&self.layout, &record)?);
                }
            }
        }
        Ok(rows)
    }

    pub fn rows_by_rids(&mut self, rids: &[Rid]) -> TableResult<Vec<Row>> {
        let mut rows = Vec::with_capacity(rids.len());
        for rid in rids {
            let record = self.file.get(*rid)?;
            rows.push(decode_record(&self.layout, &record)?);
        }
        Ok(rows)
    }

    pub fn update_where(
        &mut self,
        expr: Option<Expr>,
        assignments: &[(String, Value)],
    ) -> TableResult<usize> {
        let records = self.collect_records(expr.as_ref())?;
        let mut affected_rows = 0usize;
        for record in records {
            let mut row = decode_record(&self.layout, &record)?;
            for (column_name, value) in assignments {
                let index = self.layout.column_index(column_name)?;
                row.values[index] = value.clone();
            }
            let bytes = encode_row(&self.layout, &row.values)?;
            self.file.update(record.rid(), &bytes)?;
            affected_rows += 1;
        }
        Ok(affected_rows)
    }

    pub fn delete_where(&mut self, expr: Option<Expr>) -> TableResult<usize> {
        let records = self.collect_records(expr.as_ref())?;
        let mut affected_rows = 0usize;
        for record in records {
            self.file.delete(record.rid())?;
            affected_rows += 1;
        }
        Ok(affected_rows)
    }

    fn collect_records(&mut self, expr: Option<&Expr>) -> TableResult<Vec<Record>> {
        let mut records = Vec::new();
        match expr {
            None => {
                let mut scan = self.file.scan();
                while let Some(record) = scan.next_record()? {
                    records.push(record);
                }
            }
            Some(expr) => {
                let filter = compile_expr(&self.layout, expr)?;
                let mut scan = self.file.scan_with_logic(filter)?;
                while let Some(record) = scan.next_record()? {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }

    fn index_entries_for_int32_column(&mut self, column: &str) -> TableResult<Vec<IndexEntry>> {
        let index = self.layout.column_index(column)?;
        let records = self.collect_records(None)?;
        let mut entries = Vec::with_capacity(records.len());
        for record in records {
            let row = decode_record(&self.layout, &record)?;
            let key = match row.values[index] {
                Value::Int32(value) => value,
                _ => {
                    return Err(TableError::InvalidSchema(
                        "int32 index requested on non-int32 column",
                    ));
                }
            };
            entries.push(IndexEntry {
                key,
                rid: record.rid(),
            });
        }
        Ok(entries)
    }
}

#[derive(Clone)]
struct LayoutColumn {
    name: String,
    column_type: ColumnType,
    offset: usize,
}

#[derive(Clone)]
struct SchemaLayout {
    columns: Vec<LayoutColumn>,
    record_size: usize,
}

impl SchemaLayout {
    fn column(&self, name: &str) -> TableResult<&LayoutColumn> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| TableError::ColumnNotFound(name.to_string()))
    }

    fn column_index(&self, name: &str) -> TableResult<usize> {
        self.columns
            .iter()
            .position(|column| column.name == name)
            .ok_or_else(|| TableError::ColumnNotFound(name.to_string()))
    }
}

#[derive(Clone, Default)]
struct Catalog {
    tables: BTreeMap<String, TableSchema>,
}

impl Catalog {
    fn load(path: &Path) -> TableResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let mut lines = content.lines();
        match lines.next() {
            Some(CATALOG_HEADER) => {}
            Some(_) => return Err(TableError::CatalogCorrupted("unexpected catalog header")),
            None => return Ok(Self::default()),
        }

        let mut tables = BTreeMap::new();
        let mut current_table: Option<TableSchema> = None;
        for line in lines {
            if line.is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                ["table", table_name] => {
                    if current_table.is_some() {
                        return Err(TableError::CatalogCorrupted("nested table declaration"));
                    }
                    current_table = Some(TableSchema {
                        name: (*table_name).to_string(),
                        columns: Vec::new(),
                    });
                }
                ["column", column_name, "int32"] => {
                    let table = current_table
                        .as_mut()
                        .ok_or(TableError::CatalogCorrupted("column outside table"))?;
                    table.columns.push(ColumnSchema {
                        name: (*column_name).to_string(),
                        column_type: ColumnType::Int32,
                    });
                }
                ["column", column_name, "float32"] => {
                    let table = current_table
                        .as_mut()
                        .ok_or(TableError::CatalogCorrupted("column outside table"))?;
                    table.columns.push(ColumnSchema {
                        name: (*column_name).to_string(),
                        column_type: ColumnType::Float32,
                    });
                }
                ["column", column_name, "char", len] => {
                    let table = current_table
                        .as_mut()
                        .ok_or(TableError::CatalogCorrupted("column outside table"))?;
                    let len = len
                        .parse::<usize>()
                        .map_err(|_| TableError::CatalogCorrupted("invalid char length"))?;
                    table.columns.push(ColumnSchema {
                        name: (*column_name).to_string(),
                        column_type: ColumnType::Char(len),
                    });
                }
                ["end"] => {
                    let table = current_table
                        .take()
                        .ok_or(TableError::CatalogCorrupted("end without table"))?;
                    compile_schema(&table)?;
                    tables.insert(table.name.clone(), table);
                }
                _ => return Err(TableError::CatalogCorrupted("unrecognized catalog line")),
            }
        }

        if current_table.is_some() {
            return Err(TableError::CatalogCorrupted("unterminated table entry"));
        }

        Ok(Self { tables })
    }

    fn save(&self, path: &Path) -> TableResult<()> {
        let mut content = String::from(CATALOG_HEADER);
        content.push('\n');
        for table in self.tables.values() {
            content.push_str("table\t");
            content.push_str(&table.name);
            content.push('\n');
            for column in &table.columns {
                content.push_str("column\t");
                content.push_str(&column.name);
                match column.column_type {
                    ColumnType::Int32 => content.push_str("\tint32\n"),
                    ColumnType::Float32 => content.push_str("\tfloat32\n"),
                    ColumnType::Char(len) => {
                        content.push_str("\tchar\t");
                        content.push_str(&len.to_string());
                        content.push('\n');
                    }
                }
            }
            content.push_str("end\n");
        }
        fs::write(path, content)?;
        Ok(())
    }

    fn list_tables(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    fn get_relation(&self, name: &str) -> TableResult<RelationMeta> {
        let schema = self
            .tables
            .get(name)
            .ok_or_else(|| TableError::TableNotFound(name.to_string()))?;
        let layout = compile_schema(schema)?;
        Ok(RelationMeta {
            name: schema.name.clone(),
            record_size: layout.record_size,
            column_count: schema.columns.len(),
        })
    }

    fn get_attributes(&self, name: &str) -> TableResult<Vec<AttributeMeta>> {
        let schema = self
            .tables
            .get(name)
            .ok_or_else(|| TableError::TableNotFound(name.to_string()))?;
        let layout = compile_schema(schema)?;
        Ok(layout
            .columns
            .iter()
            .enumerate()
            .map(|(ordinal, column)| AttributeMeta {
                relation_name: schema.name.clone(),
                column_name: column.name.clone(),
                ordinal,
                offset: column.offset,
                length: column.column_type.fixed_len(),
                column_type: column.column_type.clone(),
            })
            .collect())
    }

    fn get_table_schema(&self, name: &str) -> TableResult<TableSchema> {
        self.tables
            .get(name)
            .cloned()
            .ok_or_else(|| TableError::TableNotFound(name.to_string()))
    }
}

fn compile_schema(schema: &TableSchema) -> TableResult<SchemaLayout> {
    validate_identifier(&schema.name)?;
    if schema.columns.is_empty() {
        return Err(TableError::InvalidSchema(
            "tables must contain at least one column",
        ));
    }

    let mut offset = 0usize;
    let mut columns = Vec::with_capacity(schema.columns.len());
    let mut seen_names = BTreeMap::new();
    for column in &schema.columns {
        validate_identifier(&column.name)?;
        if seen_names.insert(column.name.clone(), ()).is_some() {
            return Err(TableError::DuplicateColumn(column.name.clone()));
        }

        let len = column.column_type.fixed_len();
        if len == 0 {
            return Err(TableError::InvalidSchema(
                "column length must be greater than zero",
            ));
        }

        columns.push(LayoutColumn {
            name: column.name.clone(),
            column_type: column.column_type.clone(),
            offset,
        });
        offset = offset
            .checked_add(len)
            .ok_or(TableError::InvalidSchema("record size overflow"))?;
    }

    Ok(SchemaLayout {
        columns,
        record_size: offset,
    })
}

fn encode_row(layout: &SchemaLayout, values: &[Value]) -> TableResult<Vec<u8>> {
    if values.len() != layout.columns.len() {
        return Err(TableError::ValueCountMismatch {
            expected: layout.columns.len(),
            actual: values.len(),
        });
    }

    let mut bytes = vec![0_u8; layout.record_size];
    for (column, value) in layout.columns.iter().zip(values) {
        encode_value(&mut bytes, column, value)?;
    }
    Ok(bytes)
}

fn encode_value(target: &mut [u8], column: &LayoutColumn, value: &Value) -> TableResult<()> {
    match (&column.column_type, value) {
        (ColumnType::Int32, Value::Int32(number)) => {
            target[column.offset..column.offset + 4].copy_from_slice(&number.to_le_bytes());
            Ok(())
        }
        (ColumnType::Float32, Value::Float32(number)) => {
            target[column.offset..column.offset + 4].copy_from_slice(&number.to_le_bytes());
            Ok(())
        }
        (ColumnType::Char(len), Value::Char(text)) => {
            let encoded = text.as_bytes();
            if encoded.len() > *len {
                return Err(TableError::InvalidSchema(
                    "char values must not exceed the declared length",
                ));
            }
            let start = column.offset;
            let end = start + len;
            target[start..end].fill(0);
            target[start..start + encoded.len()].copy_from_slice(encoded);
            Ok(())
        }
        (expected, actual) => Err(TableError::ValueTypeMismatch {
            column: column.name.clone(),
            expected: expected.kind_name(),
            actual: actual.kind_name(),
        }),
    }
}

fn decode_record(layout: &SchemaLayout, record: &Record) -> TableResult<Row> {
    let mut values = Vec::with_capacity(layout.columns.len());
    for column in &layout.columns {
        values.push(decode_value(record.data(), column)?);
    }
    Ok(Row::new(values))
}

fn decode_value(bytes: &[u8], column: &LayoutColumn) -> TableResult<Value> {
    let start = column.offset;
    match &column.column_type {
        ColumnType::Int32 => {
            let value = i32::from_le_bytes(bytes[start..start + 4].try_into().expect("i32"));
            Ok(Value::Int32(value))
        }
        ColumnType::Float32 => {
            let value = f32::from_le_bytes(bytes[start..start + 4].try_into().expect("f32"));
            Ok(Value::Float32(value))
        }
        ColumnType::Char(len) => {
            let raw = &bytes[start..start + len];
            let trimmed = raw
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&[][..], |index| &raw[..=index]);
            let value = String::from_utf8(trimmed.to_vec())
                .map_err(|_| TableError::CatalogCorrupted("stored char data is not valid UTF-8"))?;
            Ok(Value::Char(value))
        }
    }
}

fn compile_expr(layout: &SchemaLayout, expr: &Expr) -> TableResult<LogicFilter> {
    match expr {
        Expr::CmpValue { column, op, value } => {
            let column = layout.column(column)?;
            let field = scan_field(column)?;
            let value = scan_value(column, value)?;
            Ok(LogicFilter::clause(PredicateClause::field_equals_value(
                field, *op, value,
            )))
        }
        Expr::CmpColumns { lhs, op, rhs } => {
            let lhs = scan_field(layout.column(lhs)?)?;
            let rhs = scan_field(layout.column(rhs)?)?;
            Ok(LogicFilter::clause(PredicateClause::field_compares_field(
                lhs, *op, rhs,
            )))
        }
        Expr::And(children) => {
            let compiled = children
                .iter()
                .map(|child| compile_expr(layout, child))
                .collect::<TableResult<Vec<_>>>()?;
            LogicFilter::and(compiled).map_err(Into::into)
        }
        Expr::Or(children) => {
            let compiled = children
                .iter()
                .map(|child| compile_expr(layout, child))
                .collect::<TableResult<Vec<_>>>()?;
            LogicFilter::or(compiled).map_err(Into::into)
        }
        Expr::Not(child) => Ok(LogicFilter::negate(compile_expr(layout, child)?)),
    }
}

fn scan_field(column: &LayoutColumn) -> TableResult<ScanFieldRef> {
    Ok(match column.column_type {
        ColumnType::Int32 => ScanFieldRef::int32(column.offset),
        ColumnType::Float32 => ScanFieldRef::float32(column.offset),
        ColumnType::Char(len) => ScanFieldRef::bytes(column.offset, len),
    })
}

fn scan_value(column: &LayoutColumn, value: &Value) -> TableResult<ScanValue> {
    match (&column.column_type, value) {
        (ColumnType::Int32, Value::Int32(number)) => Ok(ScanValue::Int32(*number)),
        (ColumnType::Float32, Value::Float32(number)) => Ok(ScanValue::Float32(*number)),
        (ColumnType::Char(len), Value::Char(text)) => {
            let encoded = text.as_bytes();
            if encoded.len() > *len {
                return Err(TableError::ValueTypeMismatch {
                    column: column.name.clone(),
                    expected: "Char",
                    actual: "Char",
                });
            }

            let mut bytes = vec![0_u8; *len];
            bytes[..encoded.len()].copy_from_slice(encoded);
            Ok(ScanValue::Bytes(bytes))
        }
        (_, value) => Err(TableError::InvalidSchema(match value {
            Value::Int32(_) => "int32 expression does not match the selected column type",
            Value::Float32(_) => "float32 expression does not match the selected column type",
            Value::Char(_) => "char expression does not match the selected column type",
        })),
    }
}

fn validate_identifier(name: &str) -> TableResult<()> {
    let is_valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if is_valid {
        Ok(())
    } else {
        Err(TableError::InvalidName(name.to_string()))
    }
}

fn table_path(root: &Path, table_name: &str) -> PathBuf {
    root.join(format!("{table_name}.{TABLE_FILE_EXTENSION}"))
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join(CATALOG_FILE_NAME)
}
