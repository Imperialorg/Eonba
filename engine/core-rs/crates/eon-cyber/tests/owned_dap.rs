#![cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
))]

use eon_cyber::owned_dap::{Visit, inspect_counting_fixture};

#[test]
#[ignore = "requires provisioned LLVM 18 clang, lldb-dap, lldb-server and local child tracing"]
fn real_lldb_dap_hits_only_the_selected_call() {
    for (visit, caller, caller_line, main_line) in [
        (Visit::Second, "second", 12, 21),
        (Visit::Third, "third", 16, 22),
    ] {
        let stack = inspect_counting_fixture(visit).expect("fixed-fixture native LLDB probe");
        assert_eq!(
            stack.each_ref().map(|f| (f.function, f.line)),
            [("count", 4), (caller, caller_line), ("main", main_line)]
        );
        println!("{visit:?}: {stack:?}");
    }
}
