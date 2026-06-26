use rs_redbase::storage::page::{PageManager, PageStoreResult};
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_test_file(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("rs-redbase-{name}-{}-{nanos}.db", process::id()))
}

#[test]
fn writes_and_reads_back_after_reopen() -> PageStoreResult<()> {
    let path = unique_test_file("roundtrip");
    let payload = b"hello from rs-redbase";

    PageManager::create_file(&path)?;

    {
        let mut file = PageManager::open_file(&path)?;
        let page_id = {
            let mut page = file.allocate_page()?;
            page[..payload.len()].copy_from_slice(payload);
            page.page_id()
        };
        assert_eq!(page_id, 0);
        file.flush()?;
    }

    {
        let mut file = PageManager::open_file(&path)?;
        assert_eq!(file.num_pages(), 1);
        let page = file.read_page(0)?;
        assert_eq!(&page[..payload.len()], payload);
    }

    PageManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn reuses_freed_pages_in_lifo_order() -> PageStoreResult<()> {
    let path = unique_test_file("lifo");
    PageManager::create_file(&path)?;

    {
        let mut file = PageManager::open_file(&path)?;
        let page0 = file.allocate_page()?.page_id();
        let page1 = file.allocate_page()?.page_id();
        let page2 = file.allocate_page()?.page_id();
        assert_eq!((page0, page1, page2), (0, 1, 2));

        file.dispose_page(page1)?;
        file.dispose_page(page2)?;
        file.flush()?;
    }

    {
        let mut file = PageManager::open_file(&path)?;
        assert_eq!(file.allocate_page()?.page_id(), 2);
        assert_eq!(file.allocate_page()?.page_id(), 1);
        file.flush()?;
    }

    PageManager::destroy_file(&path)?;
    Ok(())
}
