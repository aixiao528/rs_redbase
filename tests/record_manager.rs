use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rs_redbase::storage::record::{
    CompoundPredicate, LogicFilter, PredicateClause, RecordError, RecordManager, RecordResult, Rid,
    ScanCompOp, ScanFieldRef, ScanPredicate, ScanValue,
};

fn unique_test_file(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!("rs_redbase_{prefix}_{nanos}.db"))
}

#[test]
fn supports_insert_update_delete_and_reopen() -> RecordResult<()> {
    let path = unique_test_file("record_roundtrip");
    let initial = *b"alpha-record-000";
    let updated = *b"omega-record-111";
    let replacement = *b"slot-reused-2222";

    RecordManager::create_file(&path, initial.len())?;

    let rid = {
        let mut file = RecordManager::open_file(&path)?;
        let rid = file.insert(&initial)?;
        let record = file.get(rid)?;
        assert_eq!(record.data(), &initial);

        file.update(rid, &updated)?;
        let record = file.get(rid)?;
        assert_eq!(record.data(), &updated);

        file.delete(rid)?;
        let error = file.get(rid).expect_err("deleted rid should be empty");
        assert!(matches!(error, RecordError::EmptySlot(found) if found == rid));

        let reused = file.insert(&replacement)?;
        assert_eq!(reused, rid);
        file.flush()?;
        reused
    };

    {
        let mut file = RecordManager::open_file(&path)?;
        let record = file.get(rid)?;
        assert_eq!(record.data(), &replacement);
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn rejects_invalid_rid_and_wrong_record_size() -> RecordResult<()> {
    let path = unique_test_file("record_errors");
    RecordManager::create_file(&path, 8)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        let error = file
            .insert(b"too long!!")
            .expect_err("size mismatch expected");
        assert!(matches!(
            error,
            RecordError::SizeMismatch {
                expected: 8,
                actual: 10
            }
        ));

        let invalid = Rid::new(1, 99);
        let error = file.get(invalid).expect_err("rid should be invalid");
        assert!(matches!(error, RecordError::InvalidRid(found) if found == invalid));
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn scans_records_in_page_and_slot_order() -> RecordResult<()> {
    let path = unique_test_file("record_scan_order");
    RecordManager::create_file(&path, 4)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&1_u32.to_le_bytes())?;
        file.insert(&2_u32.to_le_bytes())?;
        file.insert(&3_u32.to_le_bytes())?;

        let mut scan = file.scan();
        let mut values = Vec::new();
        while let Some(record) = scan.next_record()? {
            values.push(u32::from_le_bytes(
                record.data().try_into().expect("u32 record"),
            ));
        }

        assert_eq!(values, vec![1, 2, 3]);
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn scan_skips_deleted_slots_and_reuses_filter_after_reopen() -> RecordResult<()> {
    let path = unique_test_file("record_scan_filter");
    RecordManager::create_file(&path, 4)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        let rid0 = file.insert(&10_u32.to_le_bytes())?;
        let _rid1 = file.insert(&11_u32.to_le_bytes())?;
        let rid2 = file.insert(&12_u32.to_le_bytes())?;
        file.delete(rid0)?;
        file.flush()?;

        let mut scan = file.scan();
        let mut seen = Vec::new();
        while let Some(record) = scan.next_record()? {
            seen.push((
                record.rid(),
                u32::from_le_bytes(record.data().try_into().expect("u32")),
            ));
        }

        assert_eq!(seen, vec![(Rid::new(rid2.page_id(), 1), 11), (rid2, 12)]);
    }

    {
        let mut file = RecordManager::open_file(&path)?;
        let mut scan =
            file.scan_with(|bytes| u32::from_le_bytes(bytes.try_into().expect("u32")) % 2 == 0);
        let mut values = Vec::new();
        while let Some(record) = scan.next_record()? {
            values.push(u32::from_le_bytes(record.data().try_into().expect("u32")));
        }

        assert_eq!(values, vec![12]);
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn scan_with_typed_predicates_filters_int_float_and_bytes() -> RecordResult<()> {
    let path = unique_test_file("record_typed_predicate");
    RecordManager::create_file(&path, 12)?;

    let record1 = build_typed_record(1, 1.5, b"ab");
    let record2 = build_typed_record(2, 2.5, b"bc");
    let record3 = build_typed_record(3, 2.5, b"cd");

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&record1)?;
        file.insert(&record2)?;
        file.insert(&record3)?;

        {
            let mut int_scan =
                file.scan_with_predicate(ScanPredicate::int32(0, ScanCompOp::Ge, 2))?;
            let mut int_values = Vec::new();
            while let Some(record) = int_scan.next_record()? {
                int_values.push(read_i32(record.data(), 0));
            }
            assert_eq!(int_values, vec![2, 3]);
        }

        {
            let mut float_scan =
                file.scan_with_predicate(ScanPredicate::float32(4, ScanCompOp::Eq, 2.5))?;
            let mut float_values = Vec::new();
            while let Some(record) = float_scan.next_record()? {
                float_values.push(read_i32(record.data(), 0));
            }
            assert_eq!(float_values, vec![2, 3]);
        }

        {
            let mut bytes_scan = file.scan_with_predicate(ScanPredicate::bytes(
                8,
                2,
                ScanCompOp::Lt,
                b"cd".to_vec(),
            ))?;
            let mut byte_values = Vec::new();
            while let Some(record) = bytes_scan.next_record()? {
                byte_values.push(read_i32(record.data(), 0));
            }
            assert_eq!(byte_values, vec![1, 2]);
        }
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn typed_predicates_support_no_op_and_reject_out_of_bounds() -> RecordResult<()> {
    let path = unique_test_file("record_typed_predicate_errors");
    RecordManager::create_file(&path, 8)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&build_typed_record(7, 3.5, b"zz")[..8])?;

        {
            let mut no_op_scan = file.scan_with_predicate(ScanPredicate::always_true())?;
            let record = no_op_scan
                .next_record()?
                .expect("NO_OP predicate should return the first record");
            assert_eq!(read_i32(record.data(), 0), 7);
        }

        let error = match file.scan_with_predicate(ScanPredicate::int32(6, ScanCompOp::Eq, 7)) {
            Ok(_) => panic!("out-of-bounds predicate should be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RecordError::PredicateOutOfBounds {
                offset: 6,
                length: 4,
                record_size: 8
            }
        ));
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn compound_filter_supports_field_to_field_and_and_clauses() -> RecordResult<()> {
    let path = unique_test_file("record_compound_filter");
    RecordManager::create_file(&path, 12)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&build_compound_record(1, 3, b"az"))?;
        file.insert(&build_compound_record(2, 2, b"bx"))?;
        file.insert(&build_compound_record(9, 4, b"cz"))?;

        let filter = CompoundPredicate::and(vec![
            PredicateClause::field_compares_field(
                ScanFieldRef::int32(0),
                ScanCompOp::Gt,
                ScanFieldRef::int32(4),
            ),
            PredicateClause::field_equals_value(
                ScanFieldRef::bytes(8, 2),
                ScanCompOp::Eq,
                ScanValue::Bytes(b"cz".to_vec()),
            ),
        ])?;

        let mut scan = file.scan_with_filter(filter)?;
        let mut ids = Vec::new();
        while let Some(record) = scan.next_record()? {
            ids.push(read_i32(record.data(), 0));
        }

        assert_eq!(ids, vec![9]);
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn compound_filter_rejects_field_mismatch_and_out_of_bounds() -> RecordResult<()> {
    let path = unique_test_file("record_compound_filter_errors");
    RecordManager::create_file(&path, 12)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&build_typed_record(5, 4.0, b"xy"))?;

        let mismatch = CompoundPredicate::and(vec![PredicateClause::field_compares_field(
            ScanFieldRef::int32(0),
            ScanCompOp::Eq,
            ScanFieldRef::float32(4),
        )])?;
        let error = match file.scan_with_filter(mismatch) {
            Ok(_) => panic!("type mismatch should be rejected"),
            Err(error) => error,
        };
        assert!(
            matches!(error, RecordError::InvalidPredicate(message) if message.contains("matching types and lengths"))
        );

        let out_of_bounds = CompoundPredicate::and(vec![PredicateClause::field_equals_value(
            ScanFieldRef::bytes(11, 2),
            ScanCompOp::Eq,
            ScanValue::Bytes(b"zz".to_vec()),
        )])?;
        let error = match file.scan_with_filter(out_of_bounds) {
            Ok(_) => panic!("out-of-bounds field should be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RecordError::PredicateOutOfBounds {
                offset: 11,
                length: 2,
                record_size: 12
            }
        ));
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn logic_filter_supports_or_not_and_nested_trees() -> RecordResult<()> {
    let path = unique_test_file("record_logic_tree");
    RecordManager::create_file(&path, 12)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&build_compound_record(1, 3, b"az"))?;
        file.insert(&build_compound_record(2, 2, b"bx"))?;
        file.insert(&build_compound_record(9, 4, b"cz"))?;

        let id_is_two = LogicFilter::clause(PredicateClause::field_equals_value(
            ScanFieldRef::int32(0),
            ScanCompOp::Eq,
            ScanValue::Int32(2),
        ));
        let tag_is_cz = LogicFilter::clause(PredicateClause::field_equals_value(
            ScanFieldRef::bytes(8, 2),
            ScanCompOp::Eq,
            ScanValue::Bytes(b"cz".to_vec()),
        ));
        let mut or_scan = file.scan_with_logic(LogicFilter::or(vec![id_is_two, tag_is_cz])?)?;
        let mut or_ids = Vec::new();
        while let Some(record) = or_scan.next_record()? {
            or_ids.push(read_i32(record.data(), 0));
        }
        assert_eq!(or_ids, vec![2, 9]);
    }

    {
        let mut file = RecordManager::open_file(&path)?;
        let not_id_two =
            LogicFilter::negate(LogicFilter::clause(PredicateClause::field_equals_value(
                ScanFieldRef::int32(0),
                ScanCompOp::Eq,
                ScanValue::Int32(2),
            )));
        let mut not_scan = file.scan_with_logic(not_id_two)?;
        let mut not_ids = Vec::new();
        while let Some(record) = not_scan.next_record()? {
            not_ids.push(read_i32(record.data(), 0));
        }
        assert_eq!(not_ids, vec![1, 9]);
    }

    {
        let mut file = RecordManager::open_file(&path)?;
        let lhs_gt_rhs = LogicFilter::all(CompoundPredicate::and(vec![
            PredicateClause::field_compares_field(
                ScanFieldRef::int32(0),
                ScanCompOp::Gt,
                ScanFieldRef::int32(4),
            ),
        ])?);
        let tag_is_az = LogicFilter::clause(PredicateClause::field_equals_value(
            ScanFieldRef::bytes(8, 2),
            ScanCompOp::Eq,
            ScanValue::Bytes(b"az".to_vec()),
        ));
        let nested = LogicFilter::or(vec![lhs_gt_rhs, LogicFilter::negate(tag_is_az)])?;
        let mut nested_scan = file.scan_with_logic(nested)?;
        let mut nested_ids = Vec::new();
        while let Some(record) = nested_scan.next_record()? {
            nested_ids.push(read_i32(record.data(), 0));
        }
        assert_eq!(nested_ids, vec![2, 9]);
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

#[test]
fn logic_filter_rejects_empty_nodes_and_propagates_errors() -> RecordResult<()> {
    let path = unique_test_file("record_logic_tree_errors");
    RecordManager::create_file(&path, 12)?;

    {
        let mut file = RecordManager::open_file(&path)?;
        file.insert(&build_compound_record(7, 1, b"xy"))?;

        let error = match LogicFilter::or(Vec::new()) {
            Ok(_) => panic!("empty OR should be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RecordError::InvalidPredicate("logic OR nodes must contain at least one child")
        ));

        let invalid_branch = LogicFilter::clause(PredicateClause::field_equals_value(
            ScanFieldRef::bytes(11, 2),
            ScanCompOp::Eq,
            ScanValue::Bytes(b"zz".to_vec()),
        ));
        let valid_branch = LogicFilter::clause(PredicateClause::field_equals_value(
            ScanFieldRef::int32(0),
            ScanCompOp::Eq,
            ScanValue::Int32(7),
        ));
        let tree = LogicFilter::or(vec![valid_branch, invalid_branch])?;
        let error = match file.scan_with_logic(tree) {
            Ok(_) => panic!("invalid branch should propagate an error"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RecordError::PredicateOutOfBounds {
                offset: 11,
                length: 2,
                record_size: 12
            }
        ));
    }

    RecordManager::destroy_file(&path)?;
    Ok(())
}

fn build_typed_record(id: i32, rating: f32, tag: &[u8; 2]) -> [u8; 12] {
    let mut record = [0_u8; 12];
    record[..4].copy_from_slice(&id.to_le_bytes());
    record[4..8].copy_from_slice(&rating.to_le_bytes());
    record[8..10].copy_from_slice(tag);
    record
}

fn build_compound_record(lhs: i32, rhs: i32, tag: &[u8; 2]) -> [u8; 12] {
    let mut record = [0_u8; 12];
    record[..4].copy_from_slice(&lhs.to_le_bytes());
    record[4..8].copy_from_slice(&rhs.to_le_bytes());
    record[8..10].copy_from_slice(tag);
    record
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4-byte i32"))
}
