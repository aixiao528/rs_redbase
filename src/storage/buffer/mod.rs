use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::storage::page::{PAGE_DATA_SIZE, PageFile, PageId, PageIo, PageStoreError};

pub type FileId = u32;
pub type BufferPoolResult<T> = Result<T, BufferPoolError>;

#[derive(Debug)]
pub enum BufferPoolError {
    PageStore(PageStoreError),
    UnknownFile(FileId),
    FileHasPinnedPages(FileId),
    NoFrameAvailable,
    Corrupted(&'static str),
    LockPoisoned(&'static str),
}

impl Display for BufferPoolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PageStore(error) => write!(f, "{error}"),
            Self::UnknownFile(file_id) => write!(f, "unknown file id: {file_id}"),
            Self::FileHasPinnedPages(file_id) => {
                write!(f, "file {file_id} still has pinned pages")
            }
            Self::NoFrameAvailable => write!(f, "no buffer frame available"),
            Self::Corrupted(message) => write!(f, "corrupted buffer pool state: {message}"),
            Self::LockPoisoned(name) => write!(f, "lock poisoned: {name}"),
        }
    }
}

impl Error for BufferPoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PageStore(error) => Some(error),
            Self::UnknownFile(_)
            | Self::FileHasPinnedPages(_)
            | Self::NoFrameAvailable
            | Self::Corrupted(_)
            | Self::LockPoisoned(_) => None,
        }
    }
}

impl From<PageStoreError> for BufferPoolError {
    fn from(value: PageStoreError) -> Self {
        Self::PageStore(value)
    }
}

#[derive(Clone)]
pub struct BufferPool<I: PageIo + Send> {
    state: Arc<Mutex<BufferPoolState<I>>>,
}

impl<I: PageIo + Send> BufferPool<I> {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(BufferPoolState::new(capacity))),
        }
    }

    pub fn register_file(&self, file: PageFile<I>) -> BufferPoolResult<FileId> {
        let mut state = self.lock_state()?;
        state.register_file(file)
    }

    pub fn get_page(
        &self,
        file_id: FileId,
        page_id: PageId,
    ) -> BufferPoolResult<BufferReadGuard<I>> {
        let mut state = self.lock_state()?;
        let key = BufferKey { file_id, page_id };

        let frame_id = if let Some(&frame_id) = state.page_table.get(&key) {
            state.pin_frame(frame_id);
            frame_id
        } else {
            let frame_id = state.acquire_frame()?;
            let data = state.read_page_data(key)?;
            state.install_frame(frame_id, key, data, false)?;
            frame_id
        };

        let data = state.frames[frame_id].data.clone();
        drop(state);

        Ok(BufferReadGuard {
            state: Arc::clone(&self.state),
            frame_id,
            file_id,
            page_id,
            data,
        })
    }

    pub fn get_page_mut(
        &self,
        file_id: FileId,
        page_id: PageId,
    ) -> BufferPoolResult<BufferWriteGuard<I>> {
        let mut state = self.lock_state()?;
        let key = BufferKey { file_id, page_id };

        let frame_id = if let Some(&frame_id) = state.page_table.get(&key) {
            state.pin_frame(frame_id);
            state.frames[frame_id].dirty = true;
            frame_id
        } else {
            let frame_id = state.acquire_frame()?;
            let data = state.read_page_data(key)?;
            state.install_frame(frame_id, key, data, true)?;
            frame_id
        };

        let data = state.frames[frame_id].data.clone();
        drop(state);

        Ok(BufferWriteGuard {
            state: Arc::clone(&self.state),
            frame_id,
            file_id,
            page_id,
            data,
        })
    }

    pub fn allocate_page(&self, file_id: FileId) -> BufferPoolResult<BufferWriteGuard<I>> {
        let mut state = self.lock_state()?;
        let frame_id = state.acquire_frame()?;
        let page_id = state.allocate_page_id(file_id)?;
        let key = BufferKey { file_id, page_id };
        state.install_frame(frame_id, key, [0_u8; PAGE_DATA_SIZE], true)?;
        let data = state.frames[frame_id].data.clone();
        drop(state);

        Ok(BufferWriteGuard {
            state: Arc::clone(&self.state),
            frame_id,
            file_id,
            page_id,
            data,
        })
    }

    pub fn force_file(&self, file_id: FileId, page_id: Option<PageId>) -> BufferPoolResult<()> {
        let mut state = self.lock_state()?;
        state.force_file(file_id, page_id)
    }

    pub fn flush_file(&self, file_id: FileId) -> BufferPoolResult<()> {
        let mut state = self.lock_state()?;
        state.flush_file(file_id)
    }

    pub fn remove_file(&self, file_id: FileId) -> BufferPoolResult<PageFile<I>> {
        let mut state = self.lock_state()?;
        state.flush_file(file_id)?;
        state
            .files
            .remove(&file_id)
            .ok_or(BufferPoolError::UnknownFile(file_id))
    }

    fn lock_state(&self) -> BufferPoolResult<MutexGuard<'_, BufferPoolState<I>>> {
        self.state
            .lock()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer pool state"))
    }
}

pub struct BufferReadGuard<I: PageIo + Send> {
    state: Arc<Mutex<BufferPoolState<I>>>,
    frame_id: usize,
    file_id: FileId,
    page_id: PageId,
    data: Arc<RwLock<[u8; PAGE_DATA_SIZE]>>,
}

impl<I: PageIo + Send> BufferReadGuard<I> {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn page_id(&self) -> PageId {
        self.page_id
    }

    pub fn read(&self) -> BufferPoolResult<RwLockReadGuard<'_, [u8; PAGE_DATA_SIZE]>> {
        self.data
            .read()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer frame read"))
    }
}

impl<I: PageIo + Send> Drop for BufferReadGuard<I> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.unpin_frame(self.frame_id);
        }
    }
}

pub struct BufferWriteGuard<I: PageIo + Send> {
    state: Arc<Mutex<BufferPoolState<I>>>,
    frame_id: usize,
    file_id: FileId,
    page_id: PageId,
    data: Arc<RwLock<[u8; PAGE_DATA_SIZE]>>,
}

impl<I: PageIo + Send> BufferWriteGuard<I> {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn page_id(&self) -> PageId {
        self.page_id
    }

    pub fn read(&self) -> BufferPoolResult<RwLockReadGuard<'_, [u8; PAGE_DATA_SIZE]>> {
        self.data
            .read()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer frame read"))
    }

    pub fn write(&self) -> BufferPoolResult<RwLockWriteGuard<'_, [u8; PAGE_DATA_SIZE]>> {
        self.data
            .write()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer frame write"))
    }
}

impl<I: PageIo + Send> Drop for BufferWriteGuard<I> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.unpin_frame(self.frame_id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BufferKey {
    file_id: FileId,
    page_id: PageId,
}

#[derive(Debug)]
struct Frame {
    key: Option<BufferKey>,
    data: Arc<RwLock<[u8; PAGE_DATA_SIZE]>>,
    dirty: bool,
    pin_count: u32,
    prev: Option<usize>,
    next: Option<usize>,
}

impl Frame {
    fn new() -> Self {
        Self {
            key: None,
            data: Arc::new(RwLock::new([0_u8; PAGE_DATA_SIZE])),
            dirty: false,
            pin_count: 0,
            prev: None,
            next: None,
        }
    }
}

#[derive(Debug)]
struct BufferPoolState<I: PageIo + Send> {
    files: HashMap<FileId, PageFile<I>>,
    next_file_id: FileId,
    page_table: HashMap<BufferKey, usize>,
    frames: Vec<Frame>,
    free_list: Vec<usize>,
    mru_head: Option<usize>,
    lru_tail: Option<usize>,
}

impl<I: PageIo + Send> BufferPoolState<I> {
    fn new(capacity: usize) -> Self {
        Self {
            files: HashMap::new(),
            next_file_id: 0,
            page_table: HashMap::new(),
            frames: (0..capacity).map(|_| Frame::new()).collect(),
            free_list: (0..capacity).rev().collect(),
            mru_head: None,
            lru_tail: None,
        }
    }

    fn register_file(&mut self, file: PageFile<I>) -> BufferPoolResult<FileId> {
        let file_id = self.next_file_id;
        self.next_file_id = self
            .next_file_id
            .checked_add(1)
            .ok_or(BufferPoolError::Corrupted("file id overflow"))?;
        self.files.insert(file_id, file);
        Ok(file_id)
    }

    fn allocate_page_id(&mut self, file_id: FileId) -> BufferPoolResult<PageId> {
        let file = self
            .files
            .get_mut(&file_id)
            .ok_or(BufferPoolError::UnknownFile(file_id))?;

        let page_id = {
            let page = file.allocate_page()?;
            page.page_id()
        };

        Ok(page_id)
    }

    fn read_page_data(&mut self, key: BufferKey) -> BufferPoolResult<[u8; PAGE_DATA_SIZE]> {
        let file = self
            .files
            .get_mut(&key.file_id)
            .ok_or(BufferPoolError::UnknownFile(key.file_id))?;

        let mut data = [0_u8; PAGE_DATA_SIZE];
        {
            let page = file.read_page(key.page_id)?;
            data.copy_from_slice(&page);
        }

        Ok(data)
    }

    fn force_file(&mut self, file_id: FileId, page_id: Option<PageId>) -> BufferPoolResult<()> {
        if !self.files.contains_key(&file_id) {
            return Err(BufferPoolError::UnknownFile(file_id));
        }

        let matching: Vec<usize> = self
            .frames
            .iter()
            .enumerate()
            .filter_map(|(frame_id, frame)| {
                let key = frame.key?;
                let matches_file = key.file_id == file_id;
                let matches_page = page_id.is_none() || page_id == Some(key.page_id);
                (matches_file && matches_page).then_some(frame_id)
            })
            .collect();

        for frame_id in matching {
            self.flush_frame_if_dirty(frame_id)?;
        }

        if let Some(file) = self.files.get_mut(&file_id) {
            file.flush()?;
        }

        Ok(())
    }

    fn flush_file(&mut self, file_id: FileId) -> BufferPoolResult<()> {
        if !self.files.contains_key(&file_id) {
            return Err(BufferPoolError::UnknownFile(file_id));
        }

        if self.frames.iter().any(|frame| {
            matches!(frame.key, Some(key) if key.file_id == file_id) && frame.pin_count > 0
        }) {
            return Err(BufferPoolError::FileHasPinnedPages(file_id));
        }

        let matching: Vec<usize> = self
            .frames
            .iter()
            .enumerate()
            .filter_map(|(frame_id, frame)| {
                matches!(frame.key, Some(key) if key.file_id == file_id).then_some(frame_id)
            })
            .collect();

        for frame_id in matching {
            self.flush_frame_if_dirty(frame_id)?;
            self.evict_frame(frame_id)?;
        }

        if let Some(file) = self.files.get_mut(&file_id) {
            file.flush()?;
        }

        Ok(())
    }

    fn acquire_frame(&mut self) -> BufferPoolResult<usize> {
        if let Some(frame_id) = self.free_list.pop() {
            self.link_head(frame_id);
            return Ok(frame_id);
        }

        let mut candidate = self.lru_tail;
        while let Some(frame_id) = candidate {
            if self.frames[frame_id].pin_count == 0 {
                self.flush_frame_if_dirty(frame_id)?;
                self.recycle_frame(frame_id)?;
                self.touch(frame_id);
                return Ok(frame_id);
            }
            candidate = self.frames[frame_id].prev;
        }

        Err(BufferPoolError::NoFrameAvailable)
    }

    fn install_frame(
        &mut self,
        frame_id: usize,
        key: BufferKey,
        data: [u8; PAGE_DATA_SIZE],
        dirty: bool,
    ) -> BufferPoolResult<()> {
        if self.page_table.contains_key(&key) {
            return Err(BufferPoolError::Corrupted("page installed twice"));
        }

        {
            let mut frame_data = self.frame_data_write(frame_id)?;
            *frame_data = data;
        }

        self.frames[frame_id].key = Some(key);
        self.frames[frame_id].dirty = dirty;
        self.frames[frame_id].pin_count = 1;
        self.page_table.insert(key, frame_id);
        self.touch(frame_id);
        Ok(())
    }

    fn flush_frame_if_dirty(&mut self, frame_id: usize) -> BufferPoolResult<()> {
        if !self.frames[frame_id].dirty {
            return Ok(());
        }

        let key = self.frames[frame_id]
            .key
            .ok_or(BufferPoolError::Corrupted("dirty frame missing key"))?;

        let data = {
            let frame_data = self.frame_data_read(frame_id)?;
            *frame_data
        };

        let file = self
            .files
            .get_mut(&key.file_id)
            .ok_or(BufferPoolError::UnknownFile(key.file_id))?;

        {
            let mut page = file.write_page(key.page_id)?;
            page.copy_from_slice(&data);
        }
        file.flush()?;

        self.frames[frame_id].dirty = false;
        Ok(())
    }

    fn evict_frame(&mut self, frame_id: usize) -> BufferPoolResult<()> {
        if self.frames[frame_id].pin_count != 0 {
            return Err(BufferPoolError::Corrupted(
                "attempted to evict a pinned frame",
            ));
        }

        self.recycle_frame(frame_id)?;
        self.free_list.push(frame_id);
        Ok(())
    }

    fn recycle_frame(&mut self, frame_id: usize) -> BufferPoolResult<()> {
        if self.frames[frame_id].pin_count != 0 {
            return Err(BufferPoolError::Corrupted(
                "attempted to recycle a pinned frame",
            ));
        }

        if let Some(key) = self.frames[frame_id].key.take() {
            self.page_table.remove(&key);
        }

        self.frames[frame_id].dirty = false;
        self.unlink(frame_id);
        Ok(())
    }

    fn pin_frame(&mut self, frame_id: usize) {
        self.frames[frame_id].pin_count += 1;
        self.touch(frame_id);
    }

    fn unpin_frame(&mut self, frame_id: usize) {
        if self.frames[frame_id].pin_count == 0 {
            return;
        }

        self.frames[frame_id].pin_count -= 1;
        if self.frames[frame_id].pin_count == 0 {
            self.touch(frame_id);
        }
    }

    fn touch(&mut self, frame_id: usize) {
        self.unlink(frame_id);
        self.link_head(frame_id);
    }

    fn link_head(&mut self, frame_id: usize) {
        self.frames[frame_id].prev = None;
        self.frames[frame_id].next = self.mru_head;

        if let Some(old_head) = self.mru_head {
            self.frames[old_head].prev = Some(frame_id);
        } else {
            self.lru_tail = Some(frame_id);
        }

        self.mru_head = Some(frame_id);
    }

    fn unlink(&mut self, frame_id: usize) {
        let prev = self.frames[frame_id].prev;
        let next = self.frames[frame_id].next;

        if let Some(prev_id) = prev {
            self.frames[prev_id].next = next;
        } else if self.mru_head == Some(frame_id) {
            self.mru_head = next;
        }

        if let Some(next_id) = next {
            self.frames[next_id].prev = prev;
        } else if self.lru_tail == Some(frame_id) {
            self.lru_tail = prev;
        }

        self.frames[frame_id].prev = None;
        self.frames[frame_id].next = None;
    }

    fn frame_data_read(
        &self,
        frame_id: usize,
    ) -> BufferPoolResult<RwLockReadGuard<'_, [u8; PAGE_DATA_SIZE]>> {
        self.frames[frame_id]
            .data
            .read()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer frame read"))
    }

    fn frame_data_write(
        &self,
        frame_id: usize,
    ) -> BufferPoolResult<RwLockWriteGuard<'_, [u8; PAGE_DATA_SIZE]>> {
        self.frames[frame_id]
            .data
            .write()
            .map_err(|_| BufferPoolError::LockPoisoned("buffer frame write"))
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::storage::page::PageIo;

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
    fn evicts_dirty_pages_and_reads_them_back() -> BufferPoolResult<()> {
        let file = PageFile::create(MemoryPageIo::default())?;
        let pool = BufferPool::new(1);
        let file_id = pool.register_file(file)?;

        let page0_id = {
            let page = pool.allocate_page(file_id)?;
            let mut data = page.write()?;
            data[..4].copy_from_slice(b"rust");
            page.page_id()
        };

        let page1_id = {
            let page = pool.allocate_page(file_id)?;
            let mut data = page.write()?;
            data[..4].copy_from_slice(b"dbms");
            page.page_id()
        };

        assert_eq!((page0_id, page1_id), (0, 1));

        {
            let page0 = pool.get_page(file_id, page0_id)?;
            let data = page0.read()?;
            assert_eq!(&data[..4], b"rust");
        }

        {
            let page1 = pool.get_page(file_id, page1_id)?;
            let data = page1.read()?;
            assert_eq!(&data[..4], b"dbms");
        }

        Ok(())
    }

    #[test]
    fn flush_file_rejects_pinned_pages() -> BufferPoolResult<()> {
        let file = PageFile::create(MemoryPageIo::default())?;
        let pool = BufferPool::new(1);
        let file_id = pool.register_file(file)?;
        let page = pool.allocate_page(file_id)?;

        let error = pool.flush_file(file_id).expect_err("flush should fail");
        assert!(matches!(error, BufferPoolError::FileHasPinnedPages(id) if id == file_id));

        drop(page);
        pool.flush_file(file_id)?;
        Ok(())
    }

    #[test]
    fn returns_no_frame_available_when_all_pages_are_pinned() -> BufferPoolResult<()> {
        let file = PageFile::create(MemoryPageIo::default())?;
        let pool = BufferPool::new(1);
        let file_id = pool.register_file(file)?;

        let page0_id = pool.allocate_page(file_id)?.page_id();
        let page1_id = pool.allocate_page(file_id)?.page_id();

        let pinned = pool.get_page(file_id, page0_id)?;
        let error = match pool.get_page(file_id, page1_id) {
            Ok(_) => panic!("second page should not fit"),
            Err(error) => error,
        };
        assert!(matches!(error, BufferPoolError::NoFrameAvailable));

        drop(pinned);
        Ok(())
    }
}
