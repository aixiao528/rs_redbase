use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::storage::record::Rid;

const INDEX_CATALOG_FILE_NAME: &str = "indexes.txt";
const INDEX_FILE_EXTENSION: &str = "idx";
const INDEX_FILE_HEADER: &str = "rs-redbase-index-v1";

pub type IndexResult<T> = Result<T, IndexError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexKeyType {
    Int32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexMeta {
    pub name: String,
    pub table: String,
    pub column: String,
    pub key_type: IndexKeyType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    pub key: i32,
    pub rid: Rid,
}

#[derive(Clone, Default)]
pub struct IndexCatalog {
    indexes: BTreeMap<String, IndexMeta>,
}

#[derive(Debug)]
pub enum IndexError {
    Io(io::Error),
    DuplicateIndex(String),
    IndexNotFound(String),
    UnsupportedKeyType(&'static str),
    Corrupted(&'static str),
}

impl Display for IndexError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::DuplicateIndex(name) => write!(f, "index already exists: {name}"),
            Self::IndexNotFound(name) => write!(f, "index not found: {name}"),
            Self::UnsupportedKeyType(kind) => {
                write!(f, "unsupported index key type: {kind}")
            }
            Self::Corrupted(message) => write!(f, "index corrupted: {message}"),
        }
    }
}

impl Error for IndexError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::DuplicateIndex(_)
            | Self::IndexNotFound(_)
            | Self::UnsupportedKeyType(_)
            | Self::Corrupted(_) => None,
        }
    }
}

impl From<io::Error> for IndexError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl IndexCatalog {
    pub fn load(root: &Path) -> IndexResult<Self> {
        let path = index_catalog_path(root);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let mut lines = content.lines();
        match lines.next() {
            Some(INDEX_FILE_HEADER) => {}
            Some(_) => return Err(IndexError::Corrupted("unexpected index catalog header")),
            None => return Ok(Self::default()),
        }

        let mut indexes = BTreeMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            let meta = match parts.as_slice() {
                ["index", name, table, column, "int32"] => IndexMeta {
                    name: (*name).to_string(),
                    table: (*table).to_string(),
                    column: (*column).to_string(),
                    key_type: IndexKeyType::Int32,
                },
                _ => return Err(IndexError::Corrupted("unrecognized index catalog line")),
            };
            if indexes.insert(meta.name.clone(), meta).is_some() {
                return Err(IndexError::Corrupted("duplicate index entry"));
            }
        }

        Ok(Self { indexes })
    }

    pub fn save(&self, root: &Path) -> IndexResult<()> {
        let mut content = String::from(INDEX_FILE_HEADER);
        content.push('\n');
        for meta in self.indexes.values() {
            content.push_str("index\t");
            content.push_str(&meta.name);
            content.push('\t');
            content.push_str(&meta.table);
            content.push('\t');
            content.push_str(&meta.column);
            content.push('\t');
            match meta.key_type {
                IndexKeyType::Int32 => content.push_str("int32\n"),
            }
        }
        fs::write(index_catalog_path(root), content)?;
        Ok(())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.indexes.contains_key(name)
    }

    pub fn insert(&mut self, meta: IndexMeta) -> IndexResult<()> {
        if self.indexes.contains_key(&meta.name) {
            return Err(IndexError::DuplicateIndex(meta.name));
        }
        self.indexes.insert(meta.name.clone(), meta);
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> IndexResult<IndexMeta> {
        self.indexes
            .remove(name)
            .ok_or_else(|| IndexError::IndexNotFound(name.to_string()))
    }

    pub fn list(&self) -> Vec<IndexMeta> {
        self.indexes.values().cloned().collect()
    }

    pub fn indexes_for_table(&self, table: &str) -> Vec<IndexMeta> {
        self.indexes
            .values()
            .filter(|meta| meta.table == table)
            .cloned()
            .collect()
    }
}

pub fn create_index_file(root: &Path, meta: &IndexMeta) -> IndexResult<()> {
    write_index_file(root, meta, &[])
}

pub fn write_index_file(root: &Path, meta: &IndexMeta, entries: &[IndexEntry]) -> IndexResult<()> {
    let mut content = String::from(INDEX_FILE_HEADER);
    content.push('\n');
    content.push_str("name\t");
    content.push_str(&meta.name);
    content.push('\n');
    content.push_str("table\t");
    content.push_str(&meta.table);
    content.push('\n');
    content.push_str("column\t");
    content.push_str(&meta.column);
    content.push('\n');
    content.push_str("key_type\t");
    match meta.key_type {
        IndexKeyType::Int32 => content.push_str("int32\n"),
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| (entry.key, entry.rid.page_id(), entry.rid.slot_id()));
    for entry in sorted {
        content.push_str("entry\t");
        content.push_str(&entry.key.to_string());
        content.push('\t');
        content.push_str(&entry.rid.page_id().to_string());
        content.push('\t');
        content.push_str(&entry.rid.slot_id().to_string());
        content.push('\n');
    }
    fs::write(index_file_path(root, &meta.name), content)?;
    Ok(())
}

pub fn delete_index_file(root: &Path, name: &str) -> IndexResult<()> {
    fs::remove_file(index_file_path(root, name))?;
    Ok(())
}

pub fn load_index_file(root: &Path, name: &str) -> IndexResult<(IndexMeta, Vec<IndexEntry>)> {
    let content = fs::read_to_string(index_file_path(root, name))?;
    let mut lines = content.lines();
    match lines.next() {
        Some(INDEX_FILE_HEADER) => {}
        Some(_) => return Err(IndexError::Corrupted("unexpected index file header")),
        None => return Err(IndexError::Corrupted("missing index file header")),
    }

    let mut meta_name = None;
    let mut table = None;
    let mut column = None;
    let mut key_type = None;
    let mut entries = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            ["name", value] => meta_name = Some((*value).to_string()),
            ["table", value] => table = Some((*value).to_string()),
            ["column", value] => column = Some((*value).to_string()),
            ["key_type", "int32"] => key_type = Some(IndexKeyType::Int32),
            ["entry", key, page, slot] => {
                let key = key
                    .parse::<i32>()
                    .map_err(|_| IndexError::Corrupted("invalid index key"))?;
                let page = page
                    .parse::<u32>()
                    .map_err(|_| IndexError::Corrupted("invalid index page id"))?;
                let slot = slot
                    .parse::<u32>()
                    .map_err(|_| IndexError::Corrupted("invalid index slot id"))?;
                entries.push(IndexEntry {
                    key,
                    rid: Rid::new(page, slot),
                });
            }
            _ => return Err(IndexError::Corrupted("unrecognized index file line")),
        }
    }

    let meta = IndexMeta {
        name: meta_name.ok_or(IndexError::Corrupted("missing index name"))?,
        table: table.ok_or(IndexError::Corrupted("missing index table"))?,
        column: column.ok_or(IndexError::Corrupted("missing index column"))?,
        key_type: key_type.ok_or(IndexError::Corrupted("missing index key type"))?,
    };
    Ok((meta, entries))
}

pub fn replace_index_entries(root: &Path, name: &str, entries: &[IndexEntry]) -> IndexResult<()> {
    let (meta, _) = load_index_file(root, name)?;
    write_index_file(root, &meta, entries)
}

pub fn lookup_eq(root: &Path, name: &str, key: i32) -> IndexResult<Vec<Rid>> {
    let (_, entries) = load_index_file(root, name)?;
    let start = entries.partition_point(|entry| entry.key < key);
    let end = entries.partition_point(|entry| entry.key <= key);
    Ok(entries[start..end].iter().map(|entry| entry.rid).collect())
}

pub fn index_catalog_path(root: &Path) -> PathBuf {
    root.join(INDEX_CATALOG_FILE_NAME)
}

pub fn index_file_path(root: &Path, index_name: &str) -> PathBuf {
    root.join(format!("{index_name}.{INDEX_FILE_EXTENSION}"))
}
