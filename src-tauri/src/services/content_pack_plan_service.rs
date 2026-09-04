use std::collections::{BTreeMap, HashSet};

use crate::error::AppError;
use crate::models::DrivePackManifest;

const FULL_PACK_THRESHOLD_PERCENT: u64 = 25;
const RANGE_COALESCE_GAP: u64 = 64 * 1024;
const RANGE_MAX_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PackTransferMode {
    Full,
    Ranges(Vec<ByteRange>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

impl ByteRange {
    pub fn len(self) -> Result<u64, AppError> {
        content_pack_core::planner::ByteRange {
            start: self.start,
            end_inclusive: self.end_inclusive,
        }
        .len()
        .map_err(Into::into)
    }

    pub fn contains(self, start: u64, size: u64) -> bool {
        size.checked_sub(1)
            .and_then(|tail| start.checked_add(tail))
            .is_some_and(|end| start >= self.start && end <= self.end_inclusive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackFetchPlan {
    pub pack_sha256: String,
    pub mode: PackTransferMode,
    pub required_chunks: Vec<String>,
}

pub fn plan_pack_fetches(
    manifest: &DrivePackManifest,
    required_raw_chunks_in_first_use_order: &[String],
) -> Result<Vec<PackFetchPlan>, AppError> {
    let plans = legacy_plan_pack_fetches(manifest, required_raw_chunks_in_first_use_order)?;
    plans
        .into_iter()
        .map(|plan| measured_plan(manifest, plan, Default::default()))
        .collect()
}

/// Kept only for adopting an already-written cache from older launchers.
pub(crate) fn legacy_plan_pack_fetches(
    manifest: &DrivePackManifest,
    required_raw_chunks_in_first_use_order: &[String],
) -> Result<Vec<PackFetchPlan>, AppError> {
    manifest.validate()?;
    let mut seen = HashSet::new();
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    let mut pack_order = Vec::new();
    for raw_sha in required_raw_chunks_in_first_use_order {
        if !seen.insert(raw_sha.clone()) {
            continue;
        }
        let chunk = manifest.chunks.get(raw_sha).ok_or_else(|| {
            AppError::InvalidData("pack plan references an unknown raw chunk".into())
        })?;
        if !grouped.contains_key(&chunk.pack_sha256) {
            pack_order.push(chunk.pack_sha256.clone());
        }
        grouped
            .entry(chunk.pack_sha256.clone())
            .or_default()
            .push(raw_sha.clone());
    }

    pack_order
        .into_iter()
        .map(|pack_sha256| {
            let required_chunks = grouped.remove(&pack_sha256).unwrap_or_default();
            let pack = manifest.packs.get(&pack_sha256).ok_or_else(|| {
                AppError::InvalidData("pack plan references an unknown pack".into())
            })?;
            let required_bytes = required_chunks.iter().try_fold(0_u64, |total, raw_sha| {
                total
                    .checked_add(manifest.chunks[raw_sha].compressed_size)
                    .ok_or_else(|| AppError::InvalidData("pack plan size overflow".into()))
            })?;
            let full = required_bytes
                .checked_mul(100)
                .ok_or_else(|| AppError::InvalidData("pack threshold overflow".into()))?
                >= pack.size.saturating_mul(FULL_PACK_THRESHOLD_PERCENT);
            let mode = if full {
                PackTransferMode::Full
            } else {
                PackTransferMode::Ranges(coalesced_ranges(manifest, &required_chunks)?)
            };
            Ok(PackFetchPlan {
                pack_sha256,
                mode,
                required_chunks,
            })
        })
        .collect()
}

pub(crate) fn required_spans(
    manifest: &DrivePackManifest,
    plan: &PackFetchPlan,
) -> Result<Vec<content_pack_core::planner::ByteRange>, AppError> {
    plan.required_chunks
        .iter()
        .map(|raw| {
            let c = manifest
                .chunks
                .get(raw)
                .filter(|c| c.pack_sha256 == plan.pack_sha256)
                .ok_or_else(|| AppError::InvalidData("unknown chunk in saved pack plan".into()))?;
            Ok(content_pack_core::planner::ByteRange {
                start: c.offset,
                end_inclusive: c
                    .offset
                    .checked_add(c.compressed_size)
                    .and_then(|n| n.checked_sub(1))
                    .ok_or_else(|| AppError::InvalidData("pack span overflow".into()))?,
            })
        })
        .collect()
}
pub(crate) fn measured_plan(
    manifest: &DrivePackManifest,
    mut plan: PackFetchPlan,
    estimate: content_pack_core::planner::CostEstimate,
) -> Result<PackFetchPlan, AppError> {
    let mode = content_pack_core::planner::choose_plan(
        manifest.packs[&plan.pack_sha256].size,
        &required_spans(manifest, &plan)?,
        estimate,
    )?;
    plan.mode = match mode {
        content_pack_core::planner::TransferMode::Full => PackTransferMode::Full,
        content_pack_core::planner::TransferMode::Ranges(ranges) => PackTransferMode::Ranges(
            ranges
                .into_iter()
                .map(|r| ByteRange {
                    start: r.start,
                    end_inclusive: r.end_inclusive,
                })
                .collect(),
        ),
    };
    Ok(plan)
}
pub(crate) fn validate_fetch_plan(
    manifest: &DrivePackManifest,
    plan: &PackFetchPlan,
) -> Result<(), AppError> {
    let pack = manifest
        .packs
        .get(&plan.pack_sha256)
        .ok_or_else(|| AppError::InvalidData("unknown frozen pack".into()))?;
    let mode = match &plan.mode {
        PackTransferMode::Full => content_pack_core::planner::TransferMode::Full,
        PackTransferMode::Ranges(r) => content_pack_core::planner::TransferMode::Ranges(
            r.iter()
                .map(|s| content_pack_core::planner::ByteRange {
                    start: s.start,
                    end_inclusive: s.end_inclusive,
                })
                .collect(),
        ),
    };
    content_pack_core::planner::validate_plan(&mode, pack.size, &required_spans(manifest, plan)?)?;
    Ok(())
}

fn coalesced_ranges(
    manifest: &DrivePackManifest,
    required_chunks: &[String],
) -> Result<Vec<ByteRange>, AppError> {
    let mut spans = required_chunks
        .iter()
        .map(|raw_sha| {
            let chunk = &manifest.chunks[raw_sha];
            let end_inclusive = chunk
                .offset
                .checked_add(chunk.compressed_size)
                .and_then(|end| end.checked_sub(1))
                .ok_or_else(|| AppError::InvalidData("pack range overflow".into()))?;
            Ok(ByteRange {
                start: chunk.offset,
                end_inclusive,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    spans.sort_unstable_by_key(|range| range.start);

    let mut ranges = Vec::new();
    for span in spans {
        let Some(previous) = ranges.last_mut() else {
            ranges.push(span);
            continue;
        };
        let gap = span
            .start
            .saturating_sub(previous.end_inclusive.saturating_add(1));
        let combined_size = span
            .end_inclusive
            .checked_sub(previous.start)
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| AppError::InvalidData("coalesced pack range overflow".into()))?;
        if gap <= RANGE_COALESCE_GAP && combined_size <= RANGE_MAX_SIZE {
            previous.end_inclusive = span.end_inclusive;
        } else {
            ranges.push(span);
        }
    }
    Ok(ranges)
}
