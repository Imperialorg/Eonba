//! Deadlock-free two-phase resource acquisition (docs/implementation/01, section 4.1).
//!
//! Requirements are merged and checked in one pass, in canonical key order. A request either
//! takes every resource it needs or takes nothing and queues, so hold-and-wait never happens and
//! circular wait is impossible. The queue is strict priority: a waiting request is never bypassed
//! by a lower-priority request, which prevents starvation of large requests.

use std::collections::BTreeMap;

use eon_primitives::{EonError, EonResult, LeaseId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceRank {
    CpuCores = 0,
    GpuCompute = 1,
    MemoryMb = 2,
    NetworkBandwidth = 3,
    FileLock = 4,
}

/// Total order over resources. Every request is sorted by this key before acquisition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalResourceKey {
    pub rank: ResourceRank,
    /// Empty for countable pools; a canonical URI for file locks.
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demand {
    /// Takes `units` from a countable pool.
    Pool(u64),
    /// Shared read access to a lock.
    SharedRead,
    /// Exclusive write access to a lock.
    ExclusiveWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub key: CanonicalResourceKey,
    pub demand: Demand,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Acquisition {
    Granted(LeaseId),
    /// Nothing was taken. The caller is identified by `(priority, seq)` and receives a `Grant` later.
    Queued,
}

/// A queued request that was granted by a later `release`.
#[derive(Debug, PartialEq, Eq)]
pub struct Grant {
    pub seq: u64,
    pub lease: LeaseId,
}

#[derive(Debug, Default)]
struct LockState {
    readers: u32,
    writer: bool,
}

#[derive(Debug)]
struct Held {
    pools: Vec<(CanonicalResourceKey, u64)>,
    locks: Vec<(CanonicalResourceKey, Demand)>,
}

#[derive(Default)]
pub struct ResourceManager {
    capacity: BTreeMap<CanonicalResourceKey, u64>,
    in_use: BTreeMap<CanonicalResourceKey, u64>,
    locks: BTreeMap<CanonicalResourceKey, LockState>,
    leases: BTreeMap<LeaseId, Held>,
    /// Waiters ordered by `(priority, seq)`, lower values first.
    waiting: BTreeMap<(u32, u64), Vec<Requirement>>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_capacity(&mut self, key: CanonicalResourceKey, units: u64) {
        self.capacity.insert(key, units);
    }

    /// Acquires everything or queues. `priority` is lower-first; `seq` breaks ties in arrival order.
    ///
    /// Errors when a pool demand exceeds total capacity, since such a request could never be granted
    /// and would block every request queued behind it.
    pub fn acquire(&mut self, priority: u32, seq: u64, requirements: Vec<Requirement>) -> EonResult<Acquisition> {
        let merged = merge(requirements)?;
        for req in &merged {
            if let Demand::Pool(units) = req.demand {
                let cap = self.capacity.get(&req.key).copied().unwrap_or(0);
                if units > cap {
                    return Err(EonError::ResourceExhausted {
                        resource: format!("{:?}:{}", req.key.rank, req.key.id),
                        requested: units,
                        available: cap,
                    });
                }
            }
        }

        let key = (priority, seq);
        let ahead_in_queue = self.waiting.range(..key).next().is_some();
        if !ahead_in_queue && self.satisfiable(&merged) {
            return Ok(Acquisition::Granted(self.take(merged)));
        }
        self.waiting.insert(key, merged);
        Ok(Acquisition::Queued)
    }

    /// Releases a lease, then grants queued requests in priority order until the first one that does not fit.
    pub fn release(&mut self, lease: &LeaseId) -> EonResult<Vec<Grant>> {
        let held = self.leases.remove(lease).ok_or_else(|| EonError::ResourceLeaseTimeout {
            lease_id: lease.to_string(),
        })?;
        for (key, units) in held.pools {
            *self.in_use.get_mut(&key).expect("pool was held") -= units;
        }
        for (key, demand) in held.locks {
            let state = self.locks.get_mut(&key).expect("lock was held");
            match demand {
                Demand::SharedRead => state.readers -= 1,
                Demand::ExclusiveWrite => state.writer = false,
                Demand::Pool(_) => unreachable!("pools are not stored as locks"),
            }
        }
        Ok(self.grant_in_order())
    }

    pub fn available(&self, key: &CanonicalResourceKey) -> u64 {
        let cap = self.capacity.get(key).copied().unwrap_or(0);
        cap - self.in_use.get(key).copied().unwrap_or(0)
    }

    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }

    fn satisfiable(&self, reqs: &[Requirement]) -> bool {
        reqs.iter().all(|req| match req.demand {
            Demand::Pool(units) => self.available(&req.key) >= units,
            Demand::SharedRead => !self.locks.get(&req.key).is_some_and(|s| s.writer),
            Demand::ExclusiveWrite => !self
                .locks
                .get(&req.key)
                .is_some_and(|s| s.writer || s.readers > 0),
        })
    }

    fn take(&mut self, reqs: Vec<Requirement>) -> LeaseId {
        let id = LeaseId::new();
        let mut held = Held {
            pools: Vec::new(),
            locks: Vec::new(),
        };
        for req in reqs {
            match req.demand {
                Demand::Pool(units) => {
                    *self.in_use.entry(req.key.clone()).or_insert(0) += units;
                    held.pools.push((req.key, units));
                }
                Demand::SharedRead => {
                    self.locks.entry(req.key.clone()).or_default().readers += 1;
                    held.locks.push((req.key, Demand::SharedRead));
                }
                Demand::ExclusiveWrite => {
                    self.locks.entry(req.key.clone()).or_default().writer = true;
                    held.locks.push((req.key, Demand::ExclusiveWrite));
                }
            }
        }
        self.leases.insert(id.clone(), held);
        id
    }

    /// Head-of-line grant: stop at the first waiter that does not fit, so no lower priority request overtakes it.
    fn grant_in_order(&mut self) -> Vec<Grant> {
        let mut granted = Vec::new();
        while let Some((&key, reqs)) = self.waiting.iter().next() {
            if !self.satisfiable(reqs) {
                break;
            }
            let reqs = self.waiting.remove(&key).expect("key was just observed");
            granted.push(Grant {
                seq: key.1,
                lease: self.take(reqs),
            });
        }
        granted
    }
}

/// Collapses duplicate keys. Pools sum. Lock demands upgrade to the strongest mode.
/// Mixing a pool and a lock on one key is a programming error.
fn merge(requirements: Vec<Requirement>) -> EonResult<Vec<Requirement>> {
    let mut by_key: BTreeMap<CanonicalResourceKey, Demand> = BTreeMap::new();
    for req in requirements {
        let merged = match by_key.get(&req.key).copied() {
            None => req.demand,
            Some(existing) => match (existing, req.demand) {
                (Demand::Pool(a), Demand::Pool(b)) => Demand::Pool(a + b),
                (Demand::ExclusiveWrite, _) | (_, Demand::ExclusiveWrite) => Demand::ExclusiveWrite,
                (Demand::SharedRead, Demand::SharedRead) => Demand::SharedRead,
                _ => {
                    return Err(EonError::ValidationError(format!(
                        "conflicting demand kinds on {:?}",
                        req.key
                    )));
                }
            },
        };
        by_key.insert(req.key, merged);
    }
    Ok(by_key
        .into_iter()
        .map(|(key, demand)| Requirement { key, demand })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(rank: ResourceRank, id: &str, units: u64) -> Requirement {
        Requirement {
            key: CanonicalResourceKey { rank, id: id.into() },
            demand: Demand::Pool(units),
        }
    }

    fn lock(id: &str, demand: Demand) -> Requirement {
        Requirement {
            key: CanonicalResourceKey {
                rank: ResourceRank::FileLock,
                id: id.into(),
            },
            demand,
        }
    }

    fn cpu_key() -> CanonicalResourceKey {
        CanonicalResourceKey {
            rank: ResourceRank::CpuCores,
            id: String::new(),
        }
    }

    fn granted(acq: Acquisition) -> LeaseId {
        match acq {
            Acquisition::Granted(lease) => lease,
            Acquisition::Queued => panic!("expected a grant"),
        }
    }

    #[test]
    fn partial_request_takes_nothing_when_one_resource_is_short() {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), 4);
        rm.set_capacity(CanonicalResourceKey { rank: ResourceRank::GpuCompute, id: String::new() }, 1);
        granted(rm.acquire(0, 1, vec![pool(ResourceRank::GpuCompute, "", 1)]).unwrap());

        let greedy = rm
            .acquire(1, 2, vec![pool(ResourceRank::CpuCores, "", 2), pool(ResourceRank::GpuCompute, "", 1)])
            .unwrap();
        assert_eq!(greedy, Acquisition::Queued);
        assert_eq!(rm.available(&cpu_key()), 4, "a queued request must not hold partial CPU");
    }

    #[test]
    fn opposite_order_requests_cannot_deadlock() {
        let mut rm = ResourceManager::new();
        for id in ["/a", "/b"] {
            rm.set_capacity(CanonicalResourceKey { rank: ResourceRank::FileLock, id: id.into() }, 1);
        }
        granted(rm.acquire(0, 1, vec![lock("/b", Demand::ExclusiveWrite), lock("/a", Demand::ExclusiveWrite)]).unwrap());
        let second = rm
            .acquire(0, 2, vec![lock("/a", Demand::ExclusiveWrite), lock("/b", Demand::ExclusiveWrite)])
            .unwrap();
        assert_eq!(second, Acquisition::Queued);
    }

    #[test]
    fn release_grants_highest_priority_waiter_with_its_seq() {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), 2);
        let holder = granted(rm.acquire(0, 1, vec![pool(ResourceRank::CpuCores, "", 2)]).unwrap());
        assert_eq!(rm.acquire(5, 2, vec![pool(ResourceRank::CpuCores, "", 2)]).unwrap(), Acquisition::Queued);
        assert_eq!(rm.acquire(1, 3, vec![pool(ResourceRank::CpuCores, "", 2)]).unwrap(), Acquisition::Queued);

        let grants = rm.release(&holder).unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].seq, 3, "priority 1 must beat priority 5");
        assert_eq!(rm.waiting_len(), 1);
    }

    #[test]
    fn large_high_priority_request_is_not_starved_by_small_later_ones() {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), 4);
        let holder = granted(rm.acquire(0, 1, vec![pool(ResourceRank::CpuCores, "", 3)]).unwrap());
        assert_eq!(rm.acquire(0, 2, vec![pool(ResourceRank::CpuCores, "", 4)]).unwrap(), Acquisition::Queued);
        // One CPU is free, so this would fit now, but a higher-priority waiter is ahead of it.
        assert_eq!(rm.acquire(9, 3, vec![pool(ResourceRank::CpuCores, "", 1)]).unwrap(), Acquisition::Queued);

        let grants = rm.release(&holder).unwrap();
        assert_eq!(grants.iter().map(|g| g.seq).collect::<Vec<_>>(), vec![2], "the 4-CPU request goes first");
    }

    #[test]
    fn request_above_capacity_is_refused_instead_of_queued_forever() {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), 2);
        let err = rm.acquire(0, 1, vec![pool(ResourceRank::CpuCores, "", 3)]).unwrap_err();
        assert!(matches!(err, EonError::ResourceExhausted { requested: 3, .. }));
        assert_eq!(rm.waiting_len(), 0);
    }

    #[test]
    fn shared_readers_coexist_but_writer_waits() {
        let mut rm = ResourceManager::new();
        let r1 = granted(rm.acquire(0, 1, vec![lock("/doc", Demand::SharedRead)]).unwrap());
        granted(rm.acquire(0, 2, vec![lock("/doc", Demand::SharedRead)]).unwrap());
        assert_eq!(rm.acquire(0, 3, vec![lock("/doc", Demand::ExclusiveWrite)]).unwrap(), Acquisition::Queued);
        assert!(rm.release(&r1).unwrap().is_empty(), "one reader still holds the lock");
    }

    #[test]
    fn duplicate_pool_requests_are_summed_before_checking() {
        let mut rm = ResourceManager::new();
        rm.set_capacity(cpu_key(), 3);
        let err = rm
            .acquire(0, 1, vec![pool(ResourceRank::CpuCores, "", 2), pool(ResourceRank::CpuCores, "", 2)])
            .unwrap_err();
        assert!(matches!(err, EonError::ResourceExhausted { requested: 4, .. }), "2+2 must be summed, then refused");
    }

    #[test]
    fn releasing_unknown_lease_is_an_error() {
        let mut rm = ResourceManager::new();
        assert!(matches!(rm.release(&LeaseId::new()), Err(EonError::ResourceLeaseTimeout { .. })));
    }
}
