use eon_primitives::EonResult;
use loro::{EncodedBlobMode, ExportMode, ID, IdSpan, JsonMapOp, JsonOpContent, LoroValue, VersionVector};

use super::{Replica, import_checked_range, loro_err, projections, rejected};
use crate::transport::MAX_FRAME_BYTES;

const MAX_PAGE_OPS: i32 = 256;

pub(crate) enum Page {
    Current,
    Update { bytes: Vec<u8>, end: ID }, // exclusive end for this writer
    ItemTooLarge,
}

fn raw_size(write: &JsonMapOp) -> Option<usize> {
    match write {
        JsonMapOp::Insert {
            key,
            value: LoroValue::Binary(bytes),
        } => key.len().checked_add(bytes.len()),
        JsonMapOp::Delete { key } => Some(key.len()),
        _ => None,
    }
}

impl Replica {
    /// Once the pull is complete we can verify that the responder's claimed
    /// push baseline is a causally closed subset of our actual history.
    pub(crate) fn check_paired_frontier(&self, vv: &VersionVector) -> EonResult<()> {
        let frontiers = self.protected.try_vv_to_frontiers(vv).map_err(loro_err)?;
        if self
            .protected
            .try_frontiers_to_vv(&frontiers)
            .map_err(loro_err)?
            .as_ref()
            != Some(vv)
        {
            return Err(rejected());
        }
        Ok(())
    }

    /// Send a prefix of one causally ready change. A suffix of a merged Loro
    /// change depends on its predecessor. Across writers, counter order alone
    /// is insufficient: the first missing change can depend on another writer.
    pub(crate) fn paired_page(
        &self,
        from: &VersionVector,
        tip: &VersionVector,
        fits_frame: impl Fn(&[u8]) -> EonResult<bool>,
    ) -> EonResult<Page> {
        if from.includes_vv(tip) {
            return Ok(Page::Current);
        }
        for missing in tip.sub_iter(from) {
            let peer = missing.peer;
            let start = missing.counter.start;
            let change = self
                .protected
                .try_get_change(ID::new(peer, start))
                .map_err(loro_err)?
                .ok_or_else(rejected)?;
            if !change.deps.iter().all(|dep| from.includes_id(dep)) {
                continue;
            }
            let change_end = change.id.counter + i32::try_from(change.len).map_err(loro_err)?;
            let limit = missing
                .counter
                .end
                .min(change_end)
                .min(start.saturating_add(MAX_PAGE_OPS));
            let mut end = start;
            let mut raw = 0;
            while end < limit {
                // Inspect one map op at a time; Loro binary values are shared.
                // Bound *uncompressed* input before asking Loro to encode a range.
                let ops = self.protected.export_json_in_id_span(IdSpan::new(peer, end, end + 1));
                let [change] = ops.as_slice() else {
                    return Err(rejected());
                };
                let [op] = change.ops.as_slice() else {
                    return Err(rejected());
                };
                let JsonOpContent::Map(write) = &op.content else {
                    return Err(rejected());
                };
                let size = raw_size(write).ok_or_else(rejected)?;
                if size > MAX_FRAME_BYTES - raw {
                    break;
                }
                raw += size;
                end += 1;
            }
            // Framing and Loro metadata count too. Shrink only an already
            // bounded candidate, never materialize the entire missing history.
            while end > start {
                let span = IdSpan::new(peer, start, end);
                let bytes = self
                    .protected
                    .export(ExportMode::updates_in_range([span].as_slice()))
                    .map_err(loro_err)?;
                if fits_frame(&bytes)? {
                    return Ok(Page::Update {
                        bytes,
                        end: ID::new(peer, end),
                    });
                }
                end = start + (end - start) / 2;
            }
            return Ok(Page::ItemTooLarge);
        }
        Err(rejected()) // missing history, but no dependency-ready prefix
    }

    /// A push may replay an already applied page (lost ACK). A pull must
    /// instead strictly advance from the requested frontier towards `source`.
    pub(crate) fn paired_apply_page(&self, bytes: &[u8], source: Option<&VersionVector>) -> EonResult<()> {
        let mut remaining = MAX_FRAME_BYTES;
        let allowed = |root: &str, write: &JsonMapOp| {
            let Some(left) = raw_size(write).and_then(|n| remaining.checked_sub(n)) else {
                return false;
            };
            remaining = left;
            projections::allowed(root, write)
        };
        import_checked_range(&self.protected, bytes, allowed, |meta, before, after| {
            let mut spans = meta.partial_end_vv.sub_iter(&meta.partial_start_vv);
            let Some(span) = spans.next() else { return false };
            if !matches!(meta.mode, EncodedBlobMode::Updates)
                || spans.next().is_some()
                || span.counter.end - span.counter.start > MAX_PAGE_OPS
                || !before.includes_vv(&meta.partial_start_vv)
            {
                return false;
            }
            let mut expected = before.clone();
            expected.merge(&meta.partial_end_vv);
            after == &expected
                && source.is_none_or(|tip| {
                    tip.includes_vv(&meta.partial_end_vv)
                        && before.get(&span.peer).copied().unwrap_or(0) == span.counter.start
                })
        })
    }
}
