use std::fs;
use std::os::unix::fs::symlink;

use eon_cognition::compiler::{Attention, CONFIRMATION_WINDOW, CandidateDecision, MAX_READ_BYTES};
use eon_kernel::TaskId;

use super::{Fixture, capture, checkout_candidates, objective, write};

#[test]
fn two_edits_to_one_captured_range_are_private_even_after_source_replacement() {
    let f = Fixture::new();
    let path = f.root().join("Data");
    let original = f.root().join("Moved");
    let outside = f.path().join("outside");
    write(&path, b"before-12345-after");
    write(&outside, b"not captured");
    let ws = f.workspace(&["Data"]);
    let intent = "read 5 bytes from Data at offset 7";
    let a = capture(&ws, intent).unwrap();
    let b = capture(&ws, intent).unwrap();
    drop(ws);
    fs::rename(&path, &original).unwrap();
    symlink(&outside, &path).unwrap();

    for (snapshot, index, expected) in [(a, 0, b"abc45"), (b, 1, b"12XYZ")] {
        let mut branches = snapshot.checkout_variants().unwrap();
        let choices: Vec<_> = branches.choices().collect();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].request, choices[1].request);
        let ids = [choices[0].id.clone(), choices[1].id.clone()];
        assert_ne!(ids[0], ids[1]);
        for (off, bytes) in [
            (6, b"x".as_slice()),
            (12, b"x"),
            (11, b"xy"),
            (u64::MAX, b"x"),
            (7, b""),
        ] {
            assert!(branches.overwrite(&ids[0], off, bytes).is_err());
        }
        branches.overwrite(&ids[0], 7, b"abc").unwrap();
        branches.overwrite(&ids[1], 9, b"XYZ").unwrap();
        assert!(branches.overwrite(&ids[1], 7, b"!").is_err());
        let selected = branches
            .present(Attention::Verified)
            .unwrap()
            .select_checkout(
                CandidateDecision::Confirm(ids[index].clone()),
                Attention::Verified,
            )
            .unwrap();
        assert_eq!(selected.choice.id, ids[index]);
        assert_eq!(selected.choice.request.path, "Data");
        assert_eq!(
            selected.choice.overwrite,
            Some([7..10, 9..12][index].clone())
        );
        assert_eq!(selected.bytes, expected);
        assert_eq!(selected.file_len, 18);
    }
    assert_eq!(fs::read(original).unwrap(), b"before-12345-after");
    assert_eq!(fs::read(outside).unwrap(), b"not captured");
    assert!(path.is_symlink());
    assert_eq!(fs::read_dir(f.root()).unwrap().count(), 2);
}

#[test]
fn named_checkouts_cannot_cross_packed_extents_or_promote_an_edit_to_a_goal() {
    let f = Fixture::new();
    let data = vec![b'a'; MAX_READ_BYTES + 5];
    write(&f.root().join("A"), &data);
    write(&f.root().join("B"), b"short");
    symlink(f.path(), f.root().join("Link")).unwrap();
    let ws = f.workspace(&["A", "B", "Link/outside"]);
    write(&f.path().join("outside"), b"unapproved");
    assert!(checkout_candidates(&objective("read file A or Link/outside"), &ws).is_err());
    assert!(
        capture(&ws, "read file A then read file A")
            .unwrap()
            .checkout_variants()
            .is_err()
    );
    let mut branches = checkout_candidates(&objective("read file A or B"), &ws).unwrap();
    let ids: Vec<_> = branches.choices().map(|c| c.id.clone()).collect();
    assert!(branches.overwrite(&TaskId::new(), 0, b"x").is_err());
    assert!(
        branches
            .overwrite(&ids[0], MAX_READ_BYTES as u64 - 1, b"xx")
            .is_err()
    );
    assert!(branches.overwrite(&ids[1], 5, b"x").is_err());
    let literal = b"then write file B";
    branches.overwrite(&ids[0], 0, literal).unwrap();
    branches.overwrite(&ids[1], 4, &[0xff]).unwrap();
    let selected = branches
        .present(Attention::Verified)
        .unwrap()
        .select_checkout(
            CandidateDecision::Confirm(ids[0].clone()),
            Attention::Verified,
        )
        .unwrap();
    assert_eq!(selected.file_len, data.len());
    assert_eq!(selected.bytes.len(), MAX_READ_BYTES);
    assert_eq!(&selected.bytes[..literal.len()], literal);
    assert_eq!(
        &selected.bytes[literal.len()..],
        &data[literal.len()..MAX_READ_BYTES]
    );
    assert_eq!(fs::read(f.root().join("A")).unwrap(), data);
    assert_eq!(fs::read(f.root().join("B")).unwrap(), b"short");
}

#[test]
fn an_empty_extent_cannot_expose_or_overwrite_the_mmap_sentinel() {
    let f = Fixture::new();
    write(&f.root().join("Empty"), b"");
    let ws = f.workspace(&["Empty"]);
    let mut branches = capture(&ws, "read file Empty")
        .unwrap()
        .checkout_variants()
        .unwrap();
    let id = branches.choices().next().unwrap().id.clone();
    assert!(branches.overwrite(&id, 0, b"x").is_err());
    let selected = branches
        .present(Attention::Verified)
        .unwrap()
        .select_checkout(CandidateDecision::Confirm(id), Attention::Verified)
        .unwrap();
    assert!(selected.bytes.is_empty());
    assert!(selected.choice.overwrite.is_none());
    assert_eq!(fs::metadata(f.root().join("Empty")).unwrap().len(), 0);
}

#[test]
fn countermand_foreign_choice_attention_loss_and_expiry_discard_private_edits() {
    let f = Fixture::new();
    write(&f.root().join("A"), b"abc");
    write(&f.root().join("B"), b"def");
    let ws = f.workspace(&["A", "B"]);
    let stage = || {
        let mut branches = checkout_candidates(&objective("read file A or B"), &ws).unwrap();
        let id = branches.choices().last().unwrap().id.clone();
        branches.overwrite(&id, 0, b"XYZ").unwrap();
        (branches, CandidateDecision::Confirm(id))
    };
    assert!(stage().0.present(Attention::Unverified).is_err());
    let window = || {
        let (branches, choice) = stage();
        (branches.present(Attention::Verified).unwrap(), choice)
    };
    let (withdrawn, stale) = window();
    withdrawn.cancel();
    assert!(
        window()
            .0
            .select_checkout(stale, Attention::Verified)
            .is_err()
    );
    assert!(
        window()
            .0
            .select_checkout(CandidateDecision::Nack, Attention::Verified)
            .is_err()
    );
    let (lost, id) = window();
    assert!(lost.select_checkout(id, Attention::Unverified).is_err());
    let (pending, id) = window();
    std::thread::sleep(CONFIRMATION_WINDOW);
    assert!(pending.select_checkout(id, Attention::Verified).is_err());
    assert_eq!(fs::read(f.root().join("A")).unwrap(), b"abc");
    assert_eq!(fs::read(f.root().join("B")).unwrap(), b"def");
    assert_eq!(fs::read_dir(f.root()).unwrap().count(), 2);
}
