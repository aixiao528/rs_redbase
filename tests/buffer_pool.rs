use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rs_redbase::storage::buffer::{BufferPool, BufferPoolResult};
use rs_redbase::storage::page::PageManager;

fn unique_test_file(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!("rs_redbase_{prefix}_{nanos}.db"))
}

#[test]
fn persists_evicted_pages_after_remove_file() -> BufferPoolResult<()> {
    let path = unique_test_file("buffer_pool");
    PageManager::create_file(&path)?;

    let pool = BufferPool::new(1);
    let file_id = pool.register_file(PageManager::open_file(&path)?)?;

    let page0_id = {
        let page = pool.allocate_page(file_id)?;
        let mut data = page.write()?;
        data[..5].copy_from_slice(b"alpha");
        page.page_id()
    };

    let page1_id = {
        let page = pool.allocate_page(file_id)?;
        let mut data = page.write()?;
        data[..4].copy_from_slice(b"beta");
        page.page_id()
    };

    assert_eq!((page0_id, page1_id), (0, 1));

    pool.force_file(file_id, None)?;
    let file = pool.remove_file(file_id)?;
    drop(file);

    {
        let mut reopened = PageManager::open_file(&path)?;
        let page0 = reopened.read_page(page0_id)?;
        assert_eq!(&page0[..5], b"alpha");

        let page1 = reopened.read_page(page1_id)?;
        assert_eq!(&page1[..4], b"beta");
    }

    PageManager::destroy_file(&path)?;
    Ok(())
}
