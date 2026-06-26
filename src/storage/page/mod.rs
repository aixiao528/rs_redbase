use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::{Deref, DerefMut};
use std::path::Path;

pub type PageId = u32;
pub type PageStoreResult<T> = Result<T, PageStoreError>;

pub const PAGE_HEADER_SIZE: usize = std::mem::size_of::<i32>();
pub const PAGE_DATA_SIZE: usize = 4092;
pub const PAGE_SIZE: usize = PAGE_HEADER_SIZE + PAGE_DATA_SIZE;
pub const FILE_HEADER_SIZE: usize = PAGE_SIZE;
pub const FREE_LIST_END: i32 = -1;
pub const PAGE_USED: i32 = -2;

#[derive(Debug)]
pub enum PageStoreError {
    Io(io::Error),
    InvalidPageId(PageId),
    PageAlreadyFree(PageId),
    Corrupted(&'static str),
}

impl Display for PageStoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidPageId(page_id) => write!(f, "invalid page id: {page_id}"),
            Self::PageAlreadyFree(page_id) => write!(f, "page {page_id} is already free"),
            Self::Corrupted(message) => write!(f, "corrupted page store: {message}"),
        }
    }
}

impl Error for PageStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidPageId(_) | Self::PageAlreadyFree(_) | Self::Corrupted(_) => None,
        }
    }
}

impl From<io::Error> for PageStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub trait PageIo {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()>;
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    fn len(&mut self) -> io::Result<u64>;
    fn is_empty(&mut self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
    fn sync_all(&mut self) -> io::Result<()>;
}

#[derive(Debug)]
pub struct DiskPageIo {
    file: File,
}

impl DiskPageIo {
    pub fn create_new(path: impl AsRef<Path>) -> PageStoreResult<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(Self { file })
    }

    pub fn open(path: impl AsRef<Path>) -> PageStoreResult<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self { file })
    }
}

impl PageIo for DiskPageIo {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }

    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buf)
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FileHeader {
    first_free_page: i32,
    num_pages: u32,
}

impl FileHeader {
    fn new() -> Self {
        Self {
            first_free_page: FREE_LIST_END,
            num_pages: 0,
        }
    }

    fn encode(self) -> [u8; FILE_HEADER_SIZE] {
        let mut buf = [0_u8; FILE_HEADER_SIZE];
        buf[..4].copy_from_slice(&self.first_free_page.to_le_bytes());
        buf[4..8].copy_from_slice(&self.num_pages.to_le_bytes());
        buf
    }

    fn decode(buf: &[u8; FILE_HEADER_SIZE]) -> PageStoreResult<Self> {
        let first_free_page = i32::from_le_bytes(buf[..4].try_into().expect("slice length"));
        let num_pages = u32::from_le_bytes(buf[4..8].try_into().expect("slice length"));
        let header = Self {
            first_free_page,
            num_pages,
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(self) -> PageStoreResult<()> {
        if self.first_free_page < FREE_LIST_END {
            return Err(PageStoreError::Corrupted(
                "first_free_page is smaller than the free-list sentinel",
            ));
        }

        if self.first_free_page != FREE_LIST_END && self.first_free_page as u32 >= self.num_pages {
            return Err(PageStoreError::Corrupted(
                "first_free_page points outside the current file",
            ));
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct PageHeader {
    next_free: i32,
}

impl PageHeader {
    fn used() -> Self {
        Self {
            next_free: PAGE_USED,
        }
    }

    fn encode(self) -> [u8; PAGE_HEADER_SIZE] {
        self.next_free.to_le_bytes()
    }

    fn decode(buf: &[u8; PAGE_HEADER_SIZE]) -> Self {
        Self {
            next_free: i32::from_le_bytes(*buf),
        }
    }
}

pub struct PageManager;

impl PageManager {
    pub fn create_file(path: impl AsRef<Path>) -> PageStoreResult<()> {
        let io = DiskPageIo::create_new(path)?;
        let mut file = PageFile::create(io)?;
        file.flush()?;
        Ok(())
    }

    pub fn open_file(path: impl AsRef<Path>) -> PageStoreResult<PageFile<DiskPageIo>> {
        PageFile::open(DiskPageIo::open(path)?)
    }

    pub fn destroy_file(path: impl AsRef<Path>) -> PageStoreResult<()> {
        fs::remove_file(path)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct PageFile<I: PageIo> {
    io: I,
    header: FileHeader,
    header_dirty: bool,
    loaded_page_id: Option<PageId>,
    loaded_page_dirty: bool,
    loaded_page: [u8; PAGE_SIZE],
}

impl<I: PageIo> PageFile<I> {
    pub fn create(io: I) -> PageStoreResult<Self> {
        let mut file = Self {
            io,
            header: FileHeader::new(),
            header_dirty: true,
            loaded_page_id: None,
            loaded_page_dirty: false,
            loaded_page: [0_u8; PAGE_SIZE],
        };
        file.io.set_len(FILE_HEADER_SIZE as u64)?;
        file.flush()?;
        Ok(file)
    }

    pub fn open(mut io: I) -> PageStoreResult<Self> {
        let len = io.len()?;
        if len < FILE_HEADER_SIZE as u64 {
            return Err(PageStoreError::Corrupted(
                "file is smaller than the mandatory file header",
            ));
        }

        let mut header_buf = [0_u8; FILE_HEADER_SIZE];
        io.read_exact_at(0, &mut header_buf)?;
        let header = FileHeader::decode(&header_buf)?;
        let minimum_len = FILE_HEADER_SIZE as u64 + u64::from(header.num_pages) * PAGE_SIZE as u64;
        if len < minimum_len {
            return Err(PageStoreError::Corrupted(
                "file is shorter than the number of pages declared in the header",
            ));
        }

        Ok(Self {
            io,
            header,
            header_dirty: false,
            loaded_page_id: None,
            loaded_page_dirty: false,
            loaded_page: [0_u8; PAGE_SIZE],
        })
    }

    pub fn num_pages(&self) -> u32 {
        self.header.num_pages
    }

    pub fn first_free_page(&self) -> i32 {
        self.header.first_free_page
    }

    pub fn allocate_page(&mut self) -> PageStoreResult<PageWriteGuard<'_>> {
        let page_id = if self.header.first_free_page != FREE_LIST_END {
            let page_id = self.header.first_free_page as PageId;
            self.load_page(page_id)?;
            let next = self.current_page_header().next_free;
            self.header.first_free_page = next;
            self.write_current_page_header(PageHeader::used());
            self.page_data_mut().fill(0);
            page_id
        } else {
            let page_id = self.header.num_pages;
            self.flush_loaded_page()?;
            self.header.num_pages = self
                .header
                .num_pages
                .checked_add(1)
                .ok_or(PageStoreError::Corrupted("page count overflow"))?;
            self.io.set_len(
                FILE_HEADER_SIZE as u64 + u64::from(self.header.num_pages) * PAGE_SIZE as u64,
            )?;
            self.loaded_page.fill(0);
            self.loaded_page_id = Some(page_id);
            self.loaded_page_dirty = true;
            self.write_current_page_header(PageHeader::used());
            page_id
        };

        self.header_dirty = true;
        self.loaded_page_dirty = true;

        Ok(PageWriteGuard {
            page_id,
            data: self.page_data_mut(),
        })
    }

    pub fn read_page(&mut self, page_id: PageId) -> PageStoreResult<PageReadGuard<'_>> {
        self.load_page(page_id)?;
        self.ensure_current_page_is_used(page_id)?;
        Ok(PageReadGuard {
            page_id,
            data: self.page_data(),
        })
    }

    pub fn write_page(&mut self, page_id: PageId) -> PageStoreResult<PageWriteGuard<'_>> {
        self.load_page(page_id)?;
        self.ensure_current_page_is_used(page_id)?;
        self.loaded_page_dirty = true;
        Ok(PageWriteGuard {
            page_id,
            data: self.page_data_mut(),
        })
    }

    pub fn dispose_page(&mut self, page_id: PageId) -> PageStoreResult<()> {
        self.load_page(page_id)?;
        if self.current_page_header().next_free != PAGE_USED {
            return Err(PageStoreError::PageAlreadyFree(page_id));
        }

        self.page_data_mut().fill(0);
        self.write_current_page_header(PageHeader {
            next_free: self.header.first_free_page,
        });
        self.header.first_free_page = page_id as i32;
        self.header_dirty = true;
        self.loaded_page_dirty = true;
        Ok(())
    }

    pub fn flush(&mut self) -> PageStoreResult<()> {
        self.flush_loaded_page()?;
        self.flush_header()?;
        self.io.sync_all()?;
        Ok(())
    }

    fn flush_header(&mut self) -> PageStoreResult<()> {
        if !self.header_dirty {
            return Ok(());
        }

        self.io.write_all_at(0, &self.header.encode())?;
        self.header_dirty = false;
        Ok(())
    }

    fn flush_loaded_page(&mut self) -> PageStoreResult<()> {
        let Some(page_id) = self.loaded_page_id else {
            return Ok(());
        };

        if !self.loaded_page_dirty {
            return Ok(());
        }

        let offset = page_offset(page_id)?;
        self.io.write_all_at(offset, &self.loaded_page)?;
        self.loaded_page_dirty = false;
        Ok(())
    }

    fn load_page(&mut self, page_id: PageId) -> PageStoreResult<()> {
        self.ensure_page_id_in_range(page_id)?;
        if self.loaded_page_id == Some(page_id) {
            return Ok(());
        }

        self.flush_loaded_page()?;
        let offset = page_offset(page_id)?;
        self.io.read_exact_at(offset, &mut self.loaded_page)?;
        self.loaded_page_id = Some(page_id);
        self.loaded_page_dirty = false;
        Ok(())
    }

    fn ensure_page_id_in_range(&self, page_id: PageId) -> PageStoreResult<()> {
        if page_id >= self.header.num_pages {
            return Err(PageStoreError::InvalidPageId(page_id));
        }
        Ok(())
    }

    fn ensure_current_page_is_used(&self, page_id: PageId) -> PageStoreResult<()> {
        if self.current_page_header().next_free != PAGE_USED {
            return Err(PageStoreError::InvalidPageId(page_id));
        }
        Ok(())
    }

    fn current_page_header(&self) -> PageHeader {
        PageHeader::decode(
            self.loaded_page[..PAGE_HEADER_SIZE]
                .try_into()
                .expect("slice length"),
        )
    }

    fn write_current_page_header(&mut self, header: PageHeader) {
        self.loaded_page[..PAGE_HEADER_SIZE].copy_from_slice(&header.encode());
    }

    fn page_data(&self) -> &[u8] {
        &self.loaded_page[PAGE_HEADER_SIZE..]
    }

    fn page_data_mut(&mut self) -> &mut [u8] {
        &mut self.loaded_page[PAGE_HEADER_SIZE..]
    }
}

impl<I: PageIo> Drop for PageFile<I> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

pub struct PageReadGuard<'a> {
    page_id: PageId,
    data: &'a [u8],
}

impl<'a> PageReadGuard<'a> {
    pub fn page_id(&self) -> PageId {
        self.page_id
    }
}

impl Deref for PageReadGuard<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

pub struct PageWriteGuard<'a> {
    page_id: PageId,
    data: &'a mut [u8],
}

impl<'a> PageWriteGuard<'a> {
    pub fn page_id(&self) -> PageId {
        self.page_id
    }
}

impl Deref for PageWriteGuard<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl DerefMut for PageWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

fn page_offset(page_id: PageId) -> PageStoreResult<u64> {
    let logical_offset = u64::from(page_id)
        .checked_mul(PAGE_SIZE as u64)
        .ok_or(PageStoreError::Corrupted("page offset overflow"))?;
    (FILE_HEADER_SIZE as u64)
        .checked_add(logical_offset)
        .ok_or(PageStoreError::Corrupted("page offset overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct MemoryPageIo {
        bytes: Vec<u8>,
    }

    impl PageIo for MemoryPageIo {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            let start = offset as usize;
            let end = start + buf.len();
            if end > self.bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "read beyond end of buffer",
                ));
            }
            buf.copy_from_slice(&self.bytes[start..end]);
            Ok(())
        }

        fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
            let start = offset as usize;
            let end = start + buf.len();
            if end > self.bytes.len() {
                self.bytes.resize(end, 0);
            }
            self.bytes[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn set_len(&mut self, len: u64) -> io::Result<()> {
            self.bytes.resize(len as usize, 0);
            Ok(())
        }

        fn len(&mut self) -> io::Result<u64> {
            Ok(self.bytes.len() as u64)
        }

        fn sync_all(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn mock_io_supports_round_trip_and_reuse() -> PageStoreResult<()> {
        let io = MemoryPageIo::default();
        let mut file = PageFile::create(io)?;

        let recycled_page = {
            let page0_id = {
                let mut page0 = file.allocate_page()?;
                page0[..4].copy_from_slice(b"rust");
                page0.page_id()
            };

            let page1_id = file.allocate_page()?.page_id();
            file.dispose_page(page0_id)?;
            file.dispose_page(page1_id)?;
            file.allocate_page()?.page_id()
        };

        assert_eq!(recycled_page, 1);
        assert_eq!(file.first_free_page(), 0);
        Ok(())
    }
}
