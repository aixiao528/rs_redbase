use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

use crate::storage::buffer::{BufferPool, BufferPoolError};
use crate::storage::page::{DiskPageIo, PAGE_DATA_SIZE, PageId, PageManager, PageStoreError};
pub type SlotId = u32;
pub type RecordResult<T> = Result<T, RecordError>;

const DEFAULT_BUFFER_CAPACITY: usize = 8;
const RECORD_FILE_HEADER_SIZE: usize = 16;
const RECORD_PAGE_HEADER_SIZE: usize = 12;
const RECORD_PAGE_LIST_END: i32 = -1;
const RECORD_PAGE_FULL: i32 = -2;
const HEADER_PAGE_ID: PageId = 0;

type RecordPredicate<'a> = dyn FnMut(&[u8]) -> RecordResult<bool> + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rid {
    page_id: PageId,
    slot_id: SlotId,
}

impl Rid {
    pub fn new(page_id: PageId, slot_id: SlotId) -> Self {
        Self { page_id, slot_id }
    }

    pub fn page_id(self) -> PageId {
        self.page_id
    }

    pub fn slot_id(self) -> SlotId {
        self.slot_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    rid: Rid,
    data: Vec<u8>,
}

impl Record {
    pub fn rid(&self) -> Rid {
        self.rid
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanCompOp {
    NoOp,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanFieldType {
    Int32,
    Float32,
    Bytes,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScanValue {
    Int32(i32),
    Float32(f32),
    Bytes(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanFieldRef {
    offset: usize,
    length: usize,
    field_type: ScanFieldType,
}

impl ScanFieldRef {
    pub fn int32(offset: usize) -> Self {
        Self {
            offset,
            length: std::mem::size_of::<i32>(),
            field_type: ScanFieldType::Int32,
        }
    }

    pub fn float32(offset: usize) -> Self {
        Self {
            offset,
            length: std::mem::size_of::<f32>(),
            field_type: ScanFieldType::Float32,
        }
    }

    pub fn bytes(offset: usize, length: usize) -> Self {
        Self {
            offset,
            length,
            field_type: ScanFieldType::Bytes,
        }
    }

    fn validate_for_record(&self, record_size: usize) -> RecordResult<()> {
        let end = self
            .offset
            .checked_add(self.length)
            .ok_or(RecordError::InvalidPredicate("field offset overflows"))?;
        if end > record_size {
            return Err(RecordError::PredicateOutOfBounds {
                offset: self.offset,
                length: self.length,
                record_size,
            });
        }

        match (self.field_type, self.length) {
            (ScanFieldType::Int32, 4) => Ok(()),
            (ScanFieldType::Float32, 4) => Ok(()),
            (ScanFieldType::Bytes, 0) => Err(RecordError::InvalidPredicate(
                "byte fields must span at least one byte",
            )),
            (ScanFieldType::Bytes, _) => Ok(()),
            (ScanFieldType::Int32, _) => Err(RecordError::InvalidPredicate(
                "int32 fields must be exactly 4 bytes wide",
            )),
            (ScanFieldType::Float32, _) => Err(RecordError::InvalidPredicate(
                "float32 fields must be exactly 4 bytes wide",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScanRhs {
    Value(ScanValue),
    Field(ScanFieldRef),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScanPredicate {
    offset: usize,
    length: usize,
    field_type: ScanFieldType,
    comp_op: ScanCompOp,
    value: Option<ScanValue>,
}

impl ScanPredicate {
    pub fn always_true() -> Self {
        Self {
            offset: 0,
            length: 0,
            field_type: ScanFieldType::Bytes,
            comp_op: ScanCompOp::NoOp,
            value: None,
        }
    }

    pub fn int32(offset: usize, comp_op: ScanCompOp, value: i32) -> Self {
        Self {
            offset,
            length: std::mem::size_of::<i32>(),
            field_type: ScanFieldType::Int32,
            comp_op,
            value: Some(ScanValue::Int32(value)),
        }
    }

    pub fn float32(offset: usize, comp_op: ScanCompOp, value: f32) -> Self {
        Self {
            offset,
            length: std::mem::size_of::<f32>(),
            field_type: ScanFieldType::Float32,
            comp_op,
            value: Some(ScanValue::Float32(value)),
        }
    }

    pub fn bytes(
        offset: usize,
        length: usize,
        comp_op: ScanCompOp,
        value: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            offset,
            length,
            field_type: ScanFieldType::Bytes,
            comp_op,
            value: Some(ScanValue::Bytes(value.into())),
        }
    }

    fn validate_for_record(&self, record_size: usize) -> RecordResult<()> {
        if self.comp_op == ScanCompOp::NoOp {
            return Ok(());
        }

        let end = self
            .offset
            .checked_add(self.length)
            .ok_or(RecordError::InvalidPredicate("predicate offset overflows"))?;
        if end > record_size {
            return Err(RecordError::PredicateOutOfBounds {
                offset: self.offset,
                length: self.length,
                record_size,
            });
        }

        match (self.field_type, self.length, self.value.as_ref()) {
            (ScanFieldType::Int32, 4, Some(ScanValue::Int32(_))) => Ok(()),
            (ScanFieldType::Float32, 4, Some(ScanValue::Float32(_))) => Ok(()),
            (ScanFieldType::Bytes, 0, _) => Err(RecordError::InvalidPredicate(
                "byte predicates must compare at least one byte",
            )),
            (ScanFieldType::Bytes, len, Some(ScanValue::Bytes(bytes))) if bytes.len() == len => {
                Ok(())
            }
            (_, _, None) => Err(RecordError::InvalidPredicate(
                "non-NO_OP predicates require a right-hand value",
            )),
            (ScanFieldType::Int32, _, _) => Err(RecordError::InvalidPredicate(
                "int32 predicates must use a 4-byte Int32 value",
            )),
            (ScanFieldType::Float32, _, _) => Err(RecordError::InvalidPredicate(
                "float32 predicates must use a 4-byte Float32 value",
            )),
            (ScanFieldType::Bytes, _, _) => Err(RecordError::InvalidPredicate(
                "byte predicate value length must match the declared field length",
            )),
        }
    }

    fn matches(&self, record: &[u8]) -> RecordResult<bool> {
        self.validate_for_record(record.len())?;

        if self.comp_op == ScanCompOp::NoOp {
            return Ok(true);
        }

        let field = &record[self.offset..self.offset + self.length];
        let value = self.value.as_ref().ok_or(RecordError::InvalidPredicate(
            "non-NO_OP predicates require a right-hand value",
        ))?;

        let ordering = match (self.field_type, value) {
            (ScanFieldType::Int32, ScanValue::Int32(rhs)) => {
                let lhs = i32::from_le_bytes(field.try_into().expect("validated i32 field"));
                lhs.cmp(rhs)
            }
            (ScanFieldType::Float32, ScanValue::Float32(rhs)) => {
                let lhs = f32::from_le_bytes(field.try_into().expect("validated f32 field"));
                lhs.total_cmp(rhs)
            }
            (ScanFieldType::Bytes, ScanValue::Bytes(rhs)) => field.cmp(rhs),
            _ => {
                return Err(RecordError::InvalidPredicate(
                    "predicate field type does not match the right-hand value type",
                ));
            }
        };

        Ok(compare_ordering(ordering, self.comp_op))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompoundPredicate {
    clauses: Vec<PredicateClause>,
}

impl CompoundPredicate {
    pub fn and(clauses: impl Into<Vec<PredicateClause>>) -> RecordResult<Self> {
        let clauses = clauses.into();
        if clauses.is_empty() {
            return Err(RecordError::InvalidPredicate(
                "compound predicates must contain at least one clause",
            ));
        }

        Ok(Self { clauses })
    }

    fn validate_for_record(&self, record_size: usize) -> RecordResult<()> {
        for clause in &self.clauses {
            clause.validate_for_record(record_size)?;
        }
        Ok(())
    }

    fn matches(&self, record: &[u8]) -> RecordResult<bool> {
        self.validate_for_record(record.len())?;
        for clause in &self.clauses {
            if !clause.matches(record)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogicFilter {
    Clause(PredicateClause),
    All(CompoundPredicate),
    And(Vec<LogicFilter>),
    Or(Vec<LogicFilter>),
    Not(Box<LogicFilter>),
}

impl LogicFilter {
    pub fn clause(clause: PredicateClause) -> Self {
        Self::Clause(clause)
    }

    pub fn all(filter: CompoundPredicate) -> Self {
        Self::All(filter)
    }

    pub fn and(children: impl Into<Vec<LogicFilter>>) -> RecordResult<Self> {
        let children = children.into();
        if children.is_empty() {
            return Err(RecordError::InvalidPredicate(
                "logic AND nodes must contain at least one child",
            ));
        }
        Ok(Self::And(children))
    }

    pub fn or(children: impl Into<Vec<LogicFilter>>) -> RecordResult<Self> {
        let children = children.into();
        if children.is_empty() {
            return Err(RecordError::InvalidPredicate(
                "logic OR nodes must contain at least one child",
            ));
        }
        Ok(Self::Or(children))
    }

    pub fn negate(child: LogicFilter) -> Self {
        Self::Not(Box::new(child))
    }

    fn validate_for_record(&self, record_size: usize) -> RecordResult<()> {
        match self {
            Self::Clause(clause) => clause.validate_for_record(record_size),
            Self::All(filter) => filter.validate_for_record(record_size),
            Self::And(children) => validate_logic_children(children, record_size, "AND"),
            Self::Or(children) => validate_logic_children(children, record_size, "OR"),
            Self::Not(child) => child.validate_for_record(record_size),
        }
    }

    fn matches(&self, record: &[u8]) -> RecordResult<bool> {
        self.validate_for_record(record.len())?;
        match self {
            Self::Clause(clause) => clause.matches(record),
            Self::All(filter) => filter.matches(record),
            Self::And(children) => {
                for child in children {
                    if !child.matches(record)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Or(children) => {
                for child in children {
                    if child.matches(record)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(child) => Ok(!child.matches(record)?),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PredicateClause {
    lhs: ScanFieldRef,
    comp_op: ScanCompOp,
    rhs: ScanRhs,
}

impl PredicateClause {
    pub fn field_equals_value(lhs: ScanFieldRef, comp_op: ScanCompOp, value: ScanValue) -> Self {
        Self {
            lhs,
            comp_op,
            rhs: ScanRhs::Value(value),
        }
    }

    pub fn field_compares_field(lhs: ScanFieldRef, comp_op: ScanCompOp, rhs: ScanFieldRef) -> Self {
        Self {
            lhs,
            comp_op,
            rhs: ScanRhs::Field(rhs),
        }
    }

    fn validate_for_record(&self, record_size: usize) -> RecordResult<()> {
        if self.comp_op == ScanCompOp::NoOp {
            return Ok(());
        }

        self.lhs.validate_for_record(record_size)?;
        match &self.rhs {
            ScanRhs::Value(value) => validate_value_against_field(self.lhs, value),
            ScanRhs::Field(rhs) => {
                rhs.validate_for_record(record_size)?;
                if self.lhs.field_type != rhs.field_type || self.lhs.length != rhs.length {
                    return Err(RecordError::InvalidPredicate(
                        "field-to-field comparisons require matching types and lengths",
                    ));
                }
                Ok(())
            }
        }
    }

    fn matches(&self, record: &[u8]) -> RecordResult<bool> {
        self.validate_for_record(record.len())?;
        if self.comp_op == ScanCompOp::NoOp {
            return Ok(true);
        }

        let lhs = field_bytes(record, self.lhs)?;
        let ordering = match &self.rhs {
            ScanRhs::Value(value) => compare_field_to_value(lhs, self.lhs.field_type, value)?,
            ScanRhs::Field(rhs) => {
                let rhs_bytes = field_bytes(record, *rhs)?;
                compare_fields(lhs, self.lhs.field_type, rhs_bytes)?
            }
        };

        Ok(compare_ordering(ordering, self.comp_op))
    }
}

#[derive(Debug)]
pub enum RecordError {
    PageStore(PageStoreError),
    BufferPool(BufferPoolError),
    InvalidRecordSize(usize),
    RecordTooLarge(usize),
    InvalidPredicate(&'static str),
    PredicateOutOfBounds {
        offset: usize,
        length: usize,
        record_size: usize,
    },
    InvalidRid(Rid),
    EmptySlot(Rid),
    SizeMismatch {
        expected: usize,
        actual: usize,
    },
    Corrupted(&'static str),
}

impl Display for RecordError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PageStore(error) => write!(f, "{error}"),
            Self::BufferPool(error) => write!(f, "{error}"),
            Self::InvalidRecordSize(size) => write!(f, "invalid record size: {size}"),
            Self::RecordTooLarge(size) => write!(f, "record size {size} does not fit in one page"),
            Self::InvalidPredicate(message) => write!(f, "invalid predicate: {message}"),
            Self::PredicateOutOfBounds {
                offset,
                length,
                record_size,
            } => write!(
                f,
                "predicate range [{offset}, {}) exceeds record size {record_size}",
                offset + length
            ),
            Self::InvalidRid(rid) => {
                write!(f, "invalid rid: page {}, slot {}", rid.page_id, rid.slot_id)
            }
            Self::EmptySlot(rid) => {
                write!(
                    f,
                    "rid points to an empty slot: ({}, {})",
                    rid.page_id, rid.slot_id
                )
            }
            Self::SizeMismatch { expected, actual } => {
                write!(f, "record size mismatch: expected {expected}, got {actual}")
            }
            Self::Corrupted(message) => write!(f, "corrupted record file: {message}"),
        }
    }
}

impl Error for RecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PageStore(error) => Some(error),
            Self::BufferPool(error) => Some(error),
            Self::InvalidRecordSize(_)
            | Self::RecordTooLarge(_)
            | Self::InvalidPredicate(_)
            | Self::PredicateOutOfBounds { .. }
            | Self::InvalidRid(_)
            | Self::EmptySlot(_)
            | Self::SizeMismatch { .. }
            | Self::Corrupted(_) => None,
        }
    }
}

impl From<PageStoreError> for RecordError {
    fn from(value: PageStoreError) -> Self {
        Self::PageStore(value)
    }
}

impl From<BufferPoolError> for RecordError {
    fn from(value: BufferPoolError) -> Self {
        Self::BufferPool(value)
    }
}

#[derive(Clone, Copy, Debug)]
struct RecordFileHeader {
    record_size: u32,
    slots_per_page: u32,
    first_free_page: i32,
    num_pages: u32,
}

impl RecordFileHeader {
    fn new(record_size: usize) -> RecordResult<Self> {
        if record_size == 0 {
            return Err(RecordError::InvalidRecordSize(record_size));
        }

        let slots_per_page = compute_slots_per_page(record_size)?;
        Ok(Self {
            record_size: record_size as u32,
            slots_per_page: slots_per_page as u32,
            first_free_page: RECORD_PAGE_LIST_END,
            num_pages: 0,
        })
    }

    fn record_size(self) -> usize {
        self.record_size as usize
    }

    fn slots_per_page(self) -> usize {
        self.slots_per_page as usize
    }

    fn encode(self) -> [u8; RECORD_FILE_HEADER_SIZE] {
        let mut buf = [0_u8; RECORD_FILE_HEADER_SIZE];
        buf[..4].copy_from_slice(&self.record_size.to_le_bytes());
        buf[4..8].copy_from_slice(&self.slots_per_page.to_le_bytes());
        buf[8..12].copy_from_slice(&self.first_free_page.to_le_bytes());
        buf[12..16].copy_from_slice(&self.num_pages.to_le_bytes());
        buf
    }

    fn decode(buf: &[u8]) -> RecordResult<Self> {
        if buf.len() < RECORD_FILE_HEADER_SIZE {
            return Err(RecordError::Corrupted("header page is too short"));
        }

        let header = Self {
            record_size: u32::from_le_bytes(buf[..4].try_into().expect("slice length")),
            slots_per_page: u32::from_le_bytes(buf[4..8].try_into().expect("slice length")),
            first_free_page: i32::from_le_bytes(buf[8..12].try_into().expect("slice length")),
            num_pages: u32::from_le_bytes(buf[12..16].try_into().expect("slice length")),
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(self) -> RecordResult<()> {
        let record_size = self.record_size();
        if record_size == 0 {
            return Err(RecordError::Corrupted(
                "record size must be greater than zero",
            ));
        }

        let expected_slots = compute_slots_per_page(record_size)?;
        if self.slots_per_page() != expected_slots {
            return Err(RecordError::Corrupted(
                "stored slots_per_page does not match record size",
            ));
        }

        if self.first_free_page < RECORD_PAGE_LIST_END {
            return Err(RecordError::Corrupted(
                "first_free_page is smaller than the free-list sentinel",
            ));
        }

        if self.first_free_page != RECORD_PAGE_LIST_END
            && self.first_free_page as u32 > self.num_pages
        {
            return Err(RecordError::Corrupted(
                "first_free_page points outside the data-page range",
            ));
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct RecordPageHeader {
    next_free_page: i32,
    num_slots: u32,
    num_free_slots: u32,
}

impl RecordPageHeader {
    fn new(num_slots: usize) -> Self {
        Self {
            next_free_page: RECORD_PAGE_LIST_END,
            num_slots: num_slots as u32,
            num_free_slots: num_slots as u32,
        }
    }

    fn num_slots(self) -> usize {
        self.num_slots as usize
    }

    fn num_free_slots(self) -> usize {
        self.num_free_slots as usize
    }

    fn encode(self, buf: &mut [u8]) {
        buf[..4].copy_from_slice(&self.next_free_page.to_le_bytes());
        buf[4..8].copy_from_slice(&self.num_slots.to_le_bytes());
        buf[8..12].copy_from_slice(&self.num_free_slots.to_le_bytes());
    }

    fn decode(buf: &[u8]) -> RecordResult<Self> {
        if buf.len() < RECORD_PAGE_HEADER_SIZE {
            return Err(RecordError::Corrupted("record page header is too short"));
        }

        Ok(Self {
            next_free_page: i32::from_le_bytes(buf[..4].try_into().expect("slice length")),
            num_slots: u32::from_le_bytes(buf[4..8].try_into().expect("slice length")),
            num_free_slots: u32::from_le_bytes(buf[8..12].try_into().expect("slice length")),
        })
    }

    fn validate(self, expected_slots: usize) -> RecordResult<()> {
        if self.num_slots() != expected_slots {
            return Err(RecordError::Corrupted("record page slot count mismatch"));
        }

        if self.num_free_slots() > self.num_slots() {
            return Err(RecordError::Corrupted(
                "record page free-slot count exceeds total slots",
            ));
        }

        if self.next_free_page < RECORD_PAGE_FULL {
            return Err(RecordError::Corrupted(
                "record page next_free_page is invalid",
            ));
        }

        Ok(())
    }
}

pub struct RecordManager;

impl RecordManager {
    pub fn create_file(path: impl AsRef<Path>, record_size: usize) -> RecordResult<()> {
        PageManager::create_file(path.as_ref())?;
        let page_file = PageManager::open_file(path)?;
        let mut file = RecordFile::create(page_file, DEFAULT_BUFFER_CAPACITY, record_size)?;
        file.flush()?;
        Ok(())
    }

    pub fn open_file(path: impl AsRef<Path>) -> RecordResult<RecordFile<DiskPageIo>> {
        let page_file = PageManager::open_file(path)?;
        RecordFile::open(page_file, DEFAULT_BUFFER_CAPACITY)
    }

    pub fn destroy_file(path: impl AsRef<Path>) -> RecordResult<()> {
        PageManager::destroy_file(path)?;
        Ok(())
    }
}

pub struct RecordFile<I: crate::storage::page::PageIo + Send> {
    pool: BufferPool<I>,
    file_id: crate::storage::buffer::FileId,
    header: RecordFileHeader,
    header_dirty: bool,
}

pub struct RecordScan<'a, I>
where
    I: crate::storage::page::PageIo + Send,
{
    file: &'a mut RecordFile<I>,
    next_page_id: PageId,
    next_slot_id: SlotId,
    predicate: Box<RecordPredicate<'a>>,
}

impl<I: crate::storage::page::PageIo + Send> RecordFile<I> {
    pub fn create(
        page_file: crate::storage::page::PageFile<I>,
        buffer_capacity: usize,
        record_size: usize,
    ) -> RecordResult<Self> {
        let pool = BufferPool::new(buffer_capacity);
        let file_id = pool.register_file(page_file)?;
        {
            let header_page = pool.allocate_page(file_id)?;
            if header_page.page_id() != HEADER_PAGE_ID {
                return Err(RecordError::Corrupted(
                    "record header must be stored in page 0",
                ));
            }
        }

        let mut file = Self {
            pool,
            file_id,
            header: RecordFileHeader::new(record_size)?,
            header_dirty: true,
        };
        file.persist_header()?;
        Ok(file)
    }

    pub fn open(
        page_file: crate::storage::page::PageFile<I>,
        buffer_capacity: usize,
    ) -> RecordResult<Self> {
        let total_pages = page_file.num_pages();
        if total_pages == 0 {
            return Err(RecordError::Corrupted(
                "record file is missing the header page",
            ));
        }

        let pool = BufferPool::new(buffer_capacity);
        let file_id = pool.register_file(page_file)?;
        let header = {
            let header_page = pool.get_page(file_id, HEADER_PAGE_ID)?;
            let bytes = header_page.read()?;
            RecordFileHeader::decode(&bytes[..RECORD_FILE_HEADER_SIZE])?
        };

        let expected_total_pages = header
            .num_pages
            .checked_add(1)
            .ok_or(RecordError::Corrupted("record file page count overflow"))?;
        if expected_total_pages > total_pages {
            return Err(RecordError::Corrupted(
                "record file header declares more pages than exist on disk",
            ));
        }

        Ok(Self {
            pool,
            file_id,
            header,
            header_dirty: false,
        })
    }

    pub fn record_size(&self) -> usize {
        self.header.record_size()
    }

    pub fn slots_per_page(&self) -> usize {
        self.header.slots_per_page()
    }

    pub fn num_pages(&self) -> u32 {
        self.header.num_pages
    }

    pub fn scan<'a>(&'a mut self) -> RecordScan<'a, I> {
        RecordScan {
            file: self,
            next_page_id: HEADER_PAGE_ID + 1,
            next_slot_id: 0,
            predicate: Box::new(always_match),
        }
    }

    pub fn scan_with<'a, F>(&'a mut self, mut predicate: F) -> RecordScan<'a, I>
    where
        F: FnMut(&[u8]) -> bool + 'a,
    {
        RecordScan {
            file: self,
            next_page_id: HEADER_PAGE_ID + 1,
            next_slot_id: 0,
            predicate: Box::new(move |bytes| Ok(predicate(bytes))),
        }
    }

    pub fn scan_with_predicate<'a>(
        &'a mut self,
        predicate: ScanPredicate,
    ) -> RecordResult<RecordScan<'a, I>> {
        predicate.validate_for_record(self.record_size())?;

        Ok(RecordScan {
            file: self,
            next_page_id: HEADER_PAGE_ID + 1,
            next_slot_id: 0,
            predicate: Box::new(move |bytes| predicate.matches(bytes)),
        })
    }

    pub fn scan_with_filter<'a>(
        &'a mut self,
        filter: CompoundPredicate,
    ) -> RecordResult<RecordScan<'a, I>> {
        filter.validate_for_record(self.record_size())?;

        Ok(RecordScan {
            file: self,
            next_page_id: HEADER_PAGE_ID + 1,
            next_slot_id: 0,
            predicate: Box::new(move |bytes| filter.matches(bytes)),
        })
    }

    pub fn scan_with_logic<'a>(
        &'a mut self,
        filter: LogicFilter,
    ) -> RecordResult<RecordScan<'a, I>> {
        filter.validate_for_record(self.record_size())?;

        Ok(RecordScan {
            file: self,
            next_page_id: HEADER_PAGE_ID + 1,
            next_slot_id: 0,
            predicate: Box::new(move |bytes| filter.matches(bytes)),
        })
    }

    pub fn insert(&mut self, data: &[u8]) -> RecordResult<Rid> {
        self.ensure_record_length(data.len())?;

        if self.header.first_free_page == RECORD_PAGE_LIST_END {
            self.allocate_data_page()?;
        }

        let page_id = self.header.first_free_page as PageId;
        let page = self.pool.get_page_mut(self.file_id, page_id)?;
        let mut bytes = page.write()?;
        let mut page_header = self.read_page_header(&*bytes)?;
        page_header.validate(self.header.slots_per_page())?;

        let slot_id = find_first_free_slot(
            &bytes[bitmap_range(self.header.slots_per_page())],
            self.header.slots_per_page(),
        )
        .ok_or(RecordError::Corrupted(
            "free-page chain points to a full page",
        ))?;

        set_slot_free(
            &mut bytes[bitmap_range(self.header.slots_per_page())],
            slot_id,
            false,
        );
        page_header.num_free_slots = page_header
            .num_free_slots
            .checked_sub(1)
            .ok_or(RecordError::Corrupted("page free-slot counter underflow"))?;

        let record_range = self.record_range(slot_id)?;
        bytes[record_range].copy_from_slice(data);

        if page_header.num_free_slots() == 0 {
            self.header.first_free_page = page_header.next_free_page;
            self.header_dirty = true;
            page_header.next_free_page = RECORD_PAGE_FULL;
        }

        page_header.encode(&mut bytes[..RECORD_PAGE_HEADER_SIZE]);
        Ok(Rid::new(page_id, slot_id as SlotId))
    }

    pub fn get(&mut self, rid: Rid) -> RecordResult<Record> {
        self.validate_rid(rid)?;

        let page = self.pool.get_page(self.file_id, rid.page_id)?;
        let bytes = page.read()?;
        let page_header = self.read_page_header(&*bytes)?;
        self.ensure_slot_used(rid, page_header, &*bytes)?;
        let record_range = self.record_range(rid.slot_id as usize)?;

        Ok(Record {
            rid,
            data: bytes[record_range].to_vec(),
        })
    }

    pub fn update(&mut self, rid: Rid, data: &[u8]) -> RecordResult<()> {
        self.ensure_record_length(data.len())?;
        self.validate_rid(rid)?;

        let page = self.pool.get_page_mut(self.file_id, rid.page_id)?;
        let mut bytes = page.write()?;
        let page_header = self.read_page_header(&*bytes)?;
        self.ensure_slot_used(rid, page_header, &*bytes)?;
        let record_range = self.record_range(rid.slot_id as usize)?;
        bytes[record_range].copy_from_slice(data);
        Ok(())
    }

    pub fn delete(&mut self, rid: Rid) -> RecordResult<()> {
        self.validate_rid(rid)?;

        let page = self.pool.get_page_mut(self.file_id, rid.page_id)?;
        let mut bytes = page.write()?;
        let mut page_header = self.read_page_header(&*bytes)?;
        self.ensure_slot_used(rid, page_header, &*bytes)?;
        let was_full = page_header.num_free_slots() == 0;

        set_slot_free(
            &mut bytes[bitmap_range(self.header.slots_per_page())],
            rid.slot_id as usize,
            true,
        );
        page_header.num_free_slots = page_header
            .num_free_slots
            .checked_add(1)
            .ok_or(RecordError::Corrupted("page free-slot counter overflow"))?;

        let record_range = self.record_range(rid.slot_id as usize)?;
        bytes[record_range].fill(0);

        if was_full {
            page_header.next_free_page = self.header.first_free_page;
            self.header.first_free_page = rid.page_id as i32;
            self.header_dirty = true;
        }

        page_header.encode(&mut bytes[..RECORD_PAGE_HEADER_SIZE]);
        Ok(())
    }

    pub fn flush(&mut self) -> RecordResult<()> {
        self.persist_header()?;
        self.pool.force_file(self.file_id, None)?;
        Ok(())
    }

    fn allocate_data_page(&mut self) -> RecordResult<PageId> {
        let data_page = self.pool.allocate_page(self.file_id)?;
        let page_id = data_page.page_id();
        let mut bytes = data_page.write()?;

        let page_header = RecordPageHeader::new(self.header.slots_per_page());
        initialize_page(
            &mut *bytes,
            page_header,
            self.header.slots_per_page(),
            self.header.record_size(),
        )?;

        self.header.num_pages = self
            .header
            .num_pages
            .checked_add(1)
            .ok_or(RecordError::Corrupted("record page count overflow"))?;
        self.header.first_free_page = page_id as i32;
        self.header_dirty = true;
        Ok(page_id)
    }

    fn persist_header(&mut self) -> RecordResult<()> {
        if !self.header_dirty {
            return Ok(());
        }

        let header_page = self.pool.get_page_mut(self.file_id, HEADER_PAGE_ID)?;
        let mut bytes = header_page.write()?;
        bytes[..RECORD_FILE_HEADER_SIZE].copy_from_slice(&self.header.encode());
        self.header_dirty = false;
        Ok(())
    }

    fn read_page_header(&self, bytes: &[u8]) -> RecordResult<RecordPageHeader> {
        let header = RecordPageHeader::decode(&bytes[..RECORD_PAGE_HEADER_SIZE])?;
        header.validate(self.header.slots_per_page())?;
        Ok(header)
    }

    fn validate_rid(&self, rid: Rid) -> RecordResult<()> {
        if rid.page_id == HEADER_PAGE_ID || rid.page_id > self.header.num_pages {
            return Err(RecordError::InvalidRid(rid));
        }

        if rid.slot_id as usize >= self.header.slots_per_page() {
            return Err(RecordError::InvalidRid(rid));
        }

        Ok(())
    }

    fn ensure_record_length(&self, actual: usize) -> RecordResult<()> {
        let expected = self.header.record_size();
        if actual != expected {
            return Err(RecordError::SizeMismatch { expected, actual });
        }
        Ok(())
    }

    fn ensure_slot_used(
        &self,
        rid: Rid,
        page_header: RecordPageHeader,
        bytes: &[u8],
    ) -> RecordResult<()> {
        if rid.slot_id as usize >= page_header.num_slots() {
            return Err(RecordError::InvalidRid(rid));
        }

        if slot_is_free(
            &bytes[bitmap_range(self.header.slots_per_page())],
            rid.slot_id as usize,
        ) {
            return Err(RecordError::EmptySlot(rid));
        }

        Ok(())
    }

    fn record_range(&self, slot_index: usize) -> RecordResult<std::ops::Range<usize>> {
        if slot_index >= self.header.slots_per_page() {
            return Err(RecordError::Corrupted(
                "slot index is outside the page layout",
            ));
        }

        let record_size = self.header.record_size();
        let data_start = RECORD_PAGE_HEADER_SIZE + bitmap_bytes(self.header.slots_per_page());
        let start = data_start + slot_index * record_size;
        let end = start + record_size;
        if end > PAGE_DATA_SIZE {
            return Err(RecordError::Corrupted("record range exceeds page size"));
        }
        Ok(start..end)
    }

    fn read_record_if_used(
        &mut self,
        page_id: PageId,
        slot_index: usize,
    ) -> RecordResult<Option<Record>> {
        if page_id == HEADER_PAGE_ID || page_id > self.header.num_pages {
            return Err(RecordError::Corrupted(
                "scan cursor points outside the data-page range",
            ));
        }

        let page = self.pool.get_page(self.file_id, page_id)?;
        let bytes = page.read()?;
        let page_header = self.read_page_header(&*bytes)?;
        if slot_index >= page_header.num_slots() {
            return Err(RecordError::Corrupted(
                "scan cursor points outside the slot range",
            ));
        }

        if slot_is_free(
            &bytes[bitmap_range(self.header.slots_per_page())],
            slot_index,
        ) {
            return Ok(None);
        }

        let rid = Rid::new(page_id, slot_index as SlotId);
        let record_range = self.record_range(slot_index)?;
        Ok(Some(Record {
            rid,
            data: bytes[record_range].to_vec(),
        }))
    }
}

impl<I: crate::storage::page::PageIo + Send> Drop for RecordFile<I> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl<'a, I> RecordScan<'a, I>
where
    I: crate::storage::page::PageIo + Send,
{
    pub fn next_record(&mut self) -> RecordResult<Option<Record>> {
        while self.next_page_id <= self.file.header.num_pages {
            let page_id = self.next_page_id;
            let slot_id = self.next_slot_id as usize;
            self.advance_cursor();

            let Some(record) = self.file.read_record_if_used(page_id, slot_id)? else {
                continue;
            };

            if (self.predicate)(record.data())? {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    fn advance_cursor(&mut self) {
        if self.next_slot_id + 1 < self.file.header.slots_per_page as SlotId {
            self.next_slot_id += 1;
        } else {
            self.next_page_id += 1;
            self.next_slot_id = 0;
        }
    }
}

fn compute_slots_per_page(record_size: usize) -> RecordResult<usize> {
    if record_size == 0 {
        return Err(RecordError::InvalidRecordSize(record_size));
    }

    let mut slots = PAGE_DATA_SIZE / record_size;
    while slots > 0 {
        let bitmap_size = bitmap_bytes(slots);
        let used_bytes = RECORD_PAGE_HEADER_SIZE + bitmap_size + slots * record_size;
        if used_bytes <= PAGE_DATA_SIZE {
            return Ok(slots);
        }
        slots -= 1;
    }

    Err(RecordError::RecordTooLarge(record_size))
}

fn initialize_page(
    bytes: &mut [u8],
    header: RecordPageHeader,
    slots_per_page: usize,
    record_size: usize,
) -> RecordResult<()> {
    bytes.fill(0);
    header.encode(&mut bytes[..RECORD_PAGE_HEADER_SIZE]);

    let bitmap_range = bitmap_range(slots_per_page);
    bytes[bitmap_range].fill(0xFF);

    let record_data_start = RECORD_PAGE_HEADER_SIZE + bitmap_bytes(slots_per_page);
    let used_bytes = record_data_start + slots_per_page * record_size;
    if used_bytes > PAGE_DATA_SIZE {
        return Err(RecordError::Corrupted(
            "record layout exceeds page capacity",
        ));
    }

    Ok(())
}

fn compare_ordering(ordering: std::cmp::Ordering, comp_op: ScanCompOp) -> bool {
    match comp_op {
        ScanCompOp::NoOp => true,
        ScanCompOp::Eq => ordering == std::cmp::Ordering::Equal,
        ScanCompOp::Ne => ordering != std::cmp::Ordering::Equal,
        ScanCompOp::Lt => ordering == std::cmp::Ordering::Less,
        ScanCompOp::Gt => ordering == std::cmp::Ordering::Greater,
        ScanCompOp::Le => ordering != std::cmp::Ordering::Greater,
        ScanCompOp::Ge => ordering != std::cmp::Ordering::Less,
    }
}

fn always_match(_: &[u8]) -> RecordResult<bool> {
    Ok(true)
}

fn field_bytes(record: &[u8], field: ScanFieldRef) -> RecordResult<&[u8]> {
    field.validate_for_record(record.len())?;
    Ok(&record[field.offset..field.offset + field.length])
}

fn validate_logic_children(
    children: &[LogicFilter],
    record_size: usize,
    node_name: &'static str,
) -> RecordResult<()> {
    if children.is_empty() {
        return Err(RecordError::InvalidPredicate(match node_name {
            "AND" => "logic AND nodes must contain at least one child",
            "OR" => "logic OR nodes must contain at least one child",
            _ => "logic nodes must contain at least one child",
        }));
    }

    for child in children {
        child.validate_for_record(record_size)?;
    }
    Ok(())
}

fn validate_value_against_field(field: ScanFieldRef, value: &ScanValue) -> RecordResult<()> {
    match (field.field_type, field.length, value) {
        (ScanFieldType::Int32, 4, ScanValue::Int32(_)) => Ok(()),
        (ScanFieldType::Float32, 4, ScanValue::Float32(_)) => Ok(()),
        (ScanFieldType::Bytes, len, ScanValue::Bytes(bytes)) if bytes.len() == len => Ok(()),
        (ScanFieldType::Int32, _, _) => Err(RecordError::InvalidPredicate(
            "int32 clauses must compare against a 4-byte Int32 value",
        )),
        (ScanFieldType::Float32, _, _) => Err(RecordError::InvalidPredicate(
            "float32 clauses must compare against a 4-byte Float32 value",
        )),
        (ScanFieldType::Bytes, _, _) => Err(RecordError::InvalidPredicate(
            "byte clause value length must match the declared field length",
        )),
    }
}

fn compare_field_to_value(
    field: &[u8],
    field_type: ScanFieldType,
    value: &ScanValue,
) -> RecordResult<std::cmp::Ordering> {
    match (field_type, value) {
        (ScanFieldType::Int32, ScanValue::Int32(rhs)) => {
            let lhs = i32::from_le_bytes(field.try_into().expect("validated i32 field"));
            Ok(lhs.cmp(rhs))
        }
        (ScanFieldType::Float32, ScanValue::Float32(rhs)) => {
            let lhs = f32::from_le_bytes(field.try_into().expect("validated f32 field"));
            Ok(lhs.total_cmp(rhs))
        }
        (ScanFieldType::Bytes, ScanValue::Bytes(rhs)) => Ok(field.cmp(rhs)),
        _ => Err(RecordError::InvalidPredicate(
            "predicate field type does not match the right-hand value type",
        )),
    }
}

fn compare_fields(
    lhs: &[u8],
    field_type: ScanFieldType,
    rhs: &[u8],
) -> RecordResult<std::cmp::Ordering> {
    match field_type {
        ScanFieldType::Int32 => {
            let lhs = i32::from_le_bytes(lhs.try_into().expect("validated i32 lhs field"));
            let rhs = i32::from_le_bytes(rhs.try_into().expect("validated i32 rhs field"));
            Ok(lhs.cmp(&rhs))
        }
        ScanFieldType::Float32 => {
            let lhs = f32::from_le_bytes(lhs.try_into().expect("validated f32 lhs field"));
            let rhs = f32::from_le_bytes(rhs.try_into().expect("validated f32 rhs field"));
            Ok(lhs.total_cmp(&rhs))
        }
        ScanFieldType::Bytes => Ok(lhs.cmp(rhs)),
    }
}

fn bitmap_bytes(num_slots: usize) -> usize {
    num_slots.div_ceil(8)
}

fn bitmap_range(num_slots: usize) -> std::ops::Range<usize> {
    let start = RECORD_PAGE_HEADER_SIZE;
    let end = start + bitmap_bytes(num_slots);
    start..end
}

fn slot_is_free(bitmap: &[u8], slot_index: usize) -> bool {
    let byte_index = slot_index / 8;
    let bit_mask = 1_u8 << (slot_index % 8);
    bitmap[byte_index] & bit_mask != 0
}

fn set_slot_free(bitmap: &mut [u8], slot_index: usize, is_free: bool) {
    let byte_index = slot_index / 8;
    let bit_mask = 1_u8 << (slot_index % 8);
    if is_free {
        bitmap[byte_index] |= bit_mask;
    } else {
        bitmap[byte_index] &= !bit_mask;
    }
}

fn find_first_free_slot(bitmap: &[u8], num_slots: usize) -> Option<usize> {
    (0..num_slots).find(|&slot_index| slot_is_free(bitmap, slot_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_page_layout_for_fixed_records() -> RecordResult<()> {
        let slots = compute_slots_per_page(16)?;
        assert!(slots > 0);
        let used_bytes = RECORD_PAGE_HEADER_SIZE + bitmap_bytes(slots) + slots * 16;
        assert!(used_bytes <= PAGE_DATA_SIZE);
        Ok(())
    }

    #[test]
    fn rejects_records_that_do_not_fit_in_a_page() {
        let error = compute_slots_per_page(PAGE_DATA_SIZE).expect_err("record should not fit");
        assert!(matches!(error, RecordError::RecordTooLarge(_)));
    }
}
