//! Model-based tests for the resource manager (docs/implementation/01, section 4.1).
//!
//! Random sequences of acquire and release operations run against both the real manager and a simple
//! model. After every step the test checks capacity, exclusive-lock safety, and that the grant set matches
//! the model. At the end every outstanding lease is released and the queue must drain completely.

use std::collections::{BTreeMap, BTreeSet};

use eon_kernel::resource::{
    Acquisition, CanonicalResourceKey, Demand, Grant, Requirement, ResourceManager, ResourceRank,
};
use eon_primitives::LeaseId;
use proptest::prelude::*;

const CPU_CAPACITY: u64 = 4;
const FILES: [&str; 3] = ["/src/a.rs", "/src/b.rs", "/src/c.rs"];

#[derive(Debug, Clone)]
enum Op {
    Acquire { priority: u32, cpu: u64, file: usize, write: bool },
    Release { index: usize },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0u32..4, 1u64..=CPU_CAPACITY, 0usize..FILES.len(), any::<bool>())
            .prop_map(|(priority, cpu, file, write)| Op::Acquire { priority, cpu, file, write }),
        2 => any::<prop::sample::Index>().prop_map(|index| Op::Release { index: index.index(usize::MAX) }),
    ]
}

fn cpu_key() -> CanonicalResourceKey {
    CanonicalResourceKey { rank: ResourceRank::CpuCores, id: String::new() }
}

fn file_key(file: usize) -> CanonicalResourceKey {
    CanonicalResourceKey { rank: ResourceRank::FileLock, id: FILES[file].into() }
}

fn requirements(cpu: u64, file: usize, write: bool) -> Vec<Requirement> {
    vec![
        Requirement { key: cpu_key(), demand: Demand::Pool(cpu) },
        Requirement {
            key: file_key(file),
            demand: if write { Demand::ExclusiveWrite } else { Demand::SharedRead },
        },
    ]
}

/// What the model knows about each granted lease.
#[derive(Clone)]
struct ModelLease {
    cpu: u64,
    file: usize,
    write: bool,
}

#[derive(Default)]
struct Model {
    held: BTreeMap<u64, ModelLease>,
    lease_of_seq: BTreeMap<u64, LeaseId>,
    queued: BTreeSet<u64>,
}

impl Model {
    fn check(&self, rm: &ResourceManager) -> Result<(), TestCaseError> {
        let cpu_used: u64 = self.held.values().map(|l| l.cpu).sum();
        prop_assert!(cpu_used <= CPU_CAPACITY, "CPU over-committed: {cpu_used}");
        prop_assert_eq!(rm.available(&cpu_key()), CPU_CAPACITY - cpu_used);

        for (file, _) in FILES.iter().enumerate() {
            let writers = self.held.values().filter(|l| l.file == file && l.write).count();
            let readers = self.held.values().filter(|l| l.file == file && !l.write).count();
            prop_assert!(writers <= 1, "two writers on {}", FILES[file]);
            prop_assert!(writers == 0 || readers == 0, "writer coexists with readers on {}", FILES[file]);
        }
        prop_assert_eq!(rm.waiting_len(), self.queued.len());
        Ok(())
    }

    fn apply_grants(&mut self, grants: Vec<Grant>, waiting: &mut BTreeMap<u64, (u64, usize, bool)>) -> Result<(), TestCaseError> {
        for grant in grants {
            prop_assert!(self.queued.remove(&grant.seq), "granted seq {} was not queued", grant.seq);
            let (cpu, file, write) = waiting.remove(&grant.seq).expect("queued request is tracked");
            self.held.insert(grant.seq, ModelLease { cpu, file, write });
            self.lease_of_seq.insert(grant.seq, grant.lease);
        }
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn random_operations_respect_capacity_and_exclusivity_and_always_drain(
        ops in prop::collection::vec(op_strategy(), 1..80),
    ) {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), CPU_CAPACITY);
        for file in 0..FILES.len() {
            rm.set_capacity(file_key(file), 1);
        }

        let mut model = Model::default();
        let mut pending_requests: BTreeMap<u64, (u64, usize, bool)> = BTreeMap::new();
        let mut next_seq = 1u64;

        for op in ops {
            match op {
                Op::Acquire { priority, cpu, file, write } => {
                    let seq = next_seq;
                    next_seq += 1;
                    match rm.acquire(priority, seq, requirements(cpu, file, write)).unwrap() {
                        Acquisition::Granted(lease) => {
                            model.held.insert(seq, ModelLease { cpu, file, write });
                            model.lease_of_seq.insert(seq, lease);
                        }
                        Acquisition::Queued => {
                            model.queued.insert(seq);
                            pending_requests.insert(seq, (cpu, file, write));
                        }
                    }
                }
                Op::Release { index } => {
                    let keys: Vec<u64> = model.held.keys().copied().collect();
                    if keys.is_empty() {
                        continue;
                    }
                    let seq = keys[index % keys.len()];
                    let lease = model.lease_of_seq.remove(&seq).unwrap();
                    model.held.remove(&seq);
                    let grants = rm.release(&lease).unwrap();
                    model.apply_grants(grants, &mut pending_requests)?;
                }
            }
            model.check(&rm)?;
        }

        // Liveness: releasing every held lease must eventually grant every queued request.
        while let Some(&seq) = model.held.keys().next() {
            let lease = model.lease_of_seq.remove(&seq).unwrap();
            model.held.remove(&seq);
            let grants = rm.release(&lease).unwrap();
            model.apply_grants(grants, &mut pending_requests)?;
            model.check(&rm)?;
        }
        prop_assert!(model.queued.is_empty(), "requests never drained: {:?}", model.queued);
        prop_assert_eq!(rm.waiting_len(), 0);
        prop_assert_eq!(rm.available(&cpu_key()), CPU_CAPACITY);
    }
}
