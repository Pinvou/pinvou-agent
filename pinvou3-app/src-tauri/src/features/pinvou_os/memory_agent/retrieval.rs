//! Memory Agent 当前投影的非序列化结构索引。
//!
//! 这里保存的 posting 只用于缩小确定性查询的候选集合；它不是记忆真相，也不会
//! 进入 [`MemoryOrganizerState`](super::domain::MemoryOrganizerState)。状态恢复时可从
//! records 完整重建，因此索引损坏或丢失不会造成记忆丢失。

use std::collections::{BTreeMap, BTreeSet};

use super::domain::{OrganizedMemory, OrganizedMemoryKind, OrganizedMemoryQuery};

#[derive(Debug, Clone, Default)]
pub(super) struct MemoryRetrievalIndex {
    by_space: BTreeMap<String, BTreeSet<String>>,
    by_kind: BTreeMap<OrganizedMemoryKind, BTreeSet<String>>,
    by_subject: BTreeMap<String, BTreeSet<String>>,
    by_predicate: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct MemoryRetrievalCandidates {
    pub(super) ids: BTreeSet<String>,
    /// 为构造候选集而复制的最小 posting 大小。
    pub(super) seed_posting_count: usize,
    /// 在其他结构化条件中执行的 membership 检查次数。
    pub(super) membership_check_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeedDimension {
    Space,
    Kind,
    Subject,
    Predicate,
}

impl MemoryRetrievalIndex {
    pub(super) fn from_records<'a>(records: impl Iterator<Item = &'a OrganizedMemory>) -> Self {
        let mut index = Self::default();
        for record in records {
            index.insert(record);
        }
        index
    }

    pub(super) fn insert(&mut self, record: &OrganizedMemory) {
        insert_posting(
            &mut self.by_space,
            record.applicability.space_id.clone(),
            &record.memory_id,
        );
        insert_posting(&mut self.by_kind, record.kind, &record.memory_id);
        insert_posting(
            &mut self.by_subject,
            record.subject.clone(),
            &record.memory_id,
        );
        insert_posting(
            &mut self.by_predicate,
            record.predicate.clone(),
            &record.memory_id,
        );
    }

    /// 返回满足结构化等值过滤的候选 id。时间、环境、状态和关注词仍由整理器按
    /// 查询时刻计算，避免把会随时间变化的结果固化进派生索引。
    pub(super) fn candidate_ids(&self, query: &OrganizedMemoryQuery) -> MemoryRetrievalCandidates {
        let Some(space_ids) = self.by_space.get(&query.space_id) else {
            return MemoryRetrievalCandidates::default();
        };
        let mut seed = (SeedDimension::Space, space_ids.len());
        for candidate in [
            posting_group_size(SeedDimension::Kind, &self.by_kind, &query.kinds),
            posting_group_size(SeedDimension::Subject, &self.by_subject, &query.subjects),
            posting_group_size(
                SeedDimension::Predicate,
                &self.by_predicate,
                &query.predicates,
            ),
        ]
        .into_iter()
        .flatten()
        {
            if candidate.1 < seed.1 {
                seed = candidate;
            }
        }
        if seed.1 == 0 {
            return MemoryRetrievalCandidates::default();
        }

        let mut ids = match seed.0 {
            SeedDimension::Space => space_ids.clone(),
            SeedDimension::Kind => collect_posting_group(&self.by_kind, &query.kinds),
            SeedDimension::Subject => collect_posting_group(&self.by_subject, &query.subjects),
            SeedDimension::Predicate => {
                collect_posting_group(&self.by_predicate, &query.predicates)
            }
        };
        let mut membership_check_count = 0usize;
        if seed.0 != SeedDimension::Space {
            retain_matching(&mut ids, &mut membership_check_count, |memory_id| {
                space_ids.contains(memory_id)
            });
        }
        if seed.0 != SeedDimension::Kind && !query.kinds.is_empty() {
            retain_matching(&mut ids, &mut membership_check_count, |memory_id| {
                posting_group_contains(&self.by_kind, &query.kinds, memory_id)
            });
        }
        if seed.0 != SeedDimension::Subject && !query.subjects.is_empty() {
            retain_matching(&mut ids, &mut membership_check_count, |memory_id| {
                posting_group_contains(&self.by_subject, &query.subjects, memory_id)
            });
        }
        if seed.0 != SeedDimension::Predicate && !query.predicates.is_empty() {
            retain_matching(&mut ids, &mut membership_check_count, |memory_id| {
                posting_group_contains(&self.by_predicate, &query.predicates, memory_id)
            });
        }
        MemoryRetrievalCandidates {
            ids,
            seed_posting_count: seed.1,
            membership_check_count,
        }
    }

    #[cfg(test)]
    pub(super) fn record_count(&self) -> usize {
        self.by_space.values().map(BTreeSet::len).sum()
    }
}

fn insert_posting<K>(postings: &mut BTreeMap<K, BTreeSet<String>>, key: K, memory_id: &str)
where
    K: Ord,
{
    postings
        .entry(key)
        .or_default()
        .insert(memory_id.to_string());
}

fn posting_group_size<K>(
    dimension: SeedDimension,
    postings: &BTreeMap<K, BTreeSet<String>>,
    requested: &[K],
) -> Option<(SeedDimension, usize)>
where
    K: Ord,
{
    if requested.is_empty() {
        return None;
    }
    Some((
        dimension,
        requested
            .iter()
            .filter_map(|key| postings.get(key))
            .map(BTreeSet::len)
            .sum(),
    ))
}

fn collect_posting_group<K>(
    postings: &BTreeMap<K, BTreeSet<String>>,
    requested: &[K],
) -> BTreeSet<String>
where
    K: Ord,
{
    requested
        .iter()
        .filter_map(|key| postings.get(key))
        .flat_map(|ids| ids.iter().cloned())
        .collect()
}

fn posting_group_contains<K>(
    postings: &BTreeMap<K, BTreeSet<String>>,
    requested: &[K],
    memory_id: &str,
) -> bool
where
    K: Ord,
{
    requested
        .iter()
        .filter_map(|key| postings.get(key))
        .any(|ids| ids.contains(memory_id))
}

fn retain_matching(
    candidates: &mut BTreeSet<String>,
    membership_check_count: &mut usize,
    mut predicate: impl FnMut(&str) -> bool,
) {
    *membership_check_count = (*membership_check_count).saturating_add(candidates.len());
    candidates.retain(|memory_id| predicate(memory_id));
}
