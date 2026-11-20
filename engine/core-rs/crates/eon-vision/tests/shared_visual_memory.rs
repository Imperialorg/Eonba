#![cfg(target_os = "linux")]

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::process::Command;

use eon_vision::shared::SharedVisualMemory;
use rustix::fs::{SealFlags, fcntl_get_seals};

#[test]
fn selected_frame_can_be_mapped_in_another_process_but_not_modified() {
    let mut ring = SharedVisualMemory::with_budget(32);
    for (time, data) in [(0, b"private" as &[u8]), (1000, b"selected")] {
        ring.observe(time, (32, 32), &[0; 1024], data).unwrap();
    }
    let frame = ring.recent(1000, 1000).unwrap().next().unwrap();
    assert_eq!(frame.sequence_number, 2);
    let fd = frame.jpeg_bytes.as_fd();
    assert!(
        fcntl_get_seals(fd)
            .unwrap()
            .contains(SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)
    );
    let file = File::from(fd.try_clone_to_owned().unwrap());
    assert!(file.set_len(1).is_err());
    assert!(file.write_all_at(b"changed!", 0).is_err());

    // Drop the producer's history before a real child maps the one authorized frame.
    // The fd cannot be used to see either a previous frame or any future frames.
    drop(ring);
    let output = Command::new("python3")
        .args(["-I", "-c", "import mmap,sys; m=mmap.mmap(0,0,access=mmap.ACCESS_READ); sys.stdout.buffer.write(m[:])"])
        .stdin(file)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"selected");
}

#[test]
fn sampling_and_eviction_use_the_same_budget_and_clock_as_local_history() {
    let mut ring = SharedVisualMemory::with_budget(4);
    assert_eq!(ring.observe(0, (1, 1), &[0; 1024], b"aa").unwrap(), Some(1));
    assert_eq!(
        ring.observe(199, (1, 1), &[255; 1024], b"bb").unwrap(),
        None
    );
    assert_eq!(
        ring.observe(200, (1, 1), &[255; 1024], b"bb").unwrap(),
        Some(2)
    );
    assert!(ring.observe(400, (1, 1), &[0; 1024], b"large").is_err());
    assert_eq!(ring.recent(0, 400).unwrap().count(), 2);
    assert_eq!(
        ring.observe(400, (1, 1), &[0; 1024], b"cc").unwrap(),
        Some(3)
    );
    let seqs: Vec<_> = ring
        .recent(0, 400)
        .unwrap()
        .map(|f| f.sequence_number)
        .collect();
    assert_eq!(seqs, [2, 3]);
    assert!(ring.recent(0, 399).is_err());
    assert_eq!(ring.recent(0, 30_200).unwrap().count(), 1);
    assert_eq!(ring.recent(0, 30_400).unwrap().count(), 0);
}
