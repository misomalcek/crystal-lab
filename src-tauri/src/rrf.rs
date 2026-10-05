//! Reciprocal Rank Fusion — same algorithm as factorium `rrf.ts`.
//!
//! Scores from different retrievers are not comparable. RRF uses position
//! only: `score = Σ 1/(k + rank)`. k=60 is Cormack et al. 2009.
//!
//! Do not put edge weights into this formula. Weights order list C *before*
//! fusion; fusion reads rank, never the weight value.

pub const RRF_K: f64 = 60.0;

/// Fuse ranked lists. Lists are pre-sorted, best first. The first sighting
/// of an item is the one kept, so put the list whose payload you prefer first.
#[allow(dead_code)] // release builds call rrf_fuse_scored; this wrapper is what the tests pin
pub fn rrf_fuse<T, F: Fn(&T) -> String>(
    lists: &[Vec<T>],
    id_of: F,
    limit: usize,
    k: f64,
) -> Vec<T>
where
    T: Clone,
{
    rrf_fuse_scored(lists, id_of, limit, k)
        .into_iter()
        .map(|(item, _)| item)
        .collect()
}

pub fn rrf_fuse_scored<T, F: Fn(&T) -> String>(
    lists: &[Vec<T>],
    id_of: F,
    limit: usize,
    k: f64,
) -> Vec<(T, f64)>
where
    T: Clone,
{
    use std::collections::HashMap;
    let mut fused: HashMap<String, f64> = HashMap::new();
    let mut by_id: HashMap<String, T> = HashMap::new();
    for list in lists {
        for (rank, item) in list.iter().enumerate() {
            let id = id_of(item);
            by_id.entry(id.clone()).or_insert_with(|| item.clone());
            *fused.entry(id).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut entries: Vec<_> = fused.into_iter().collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    entries
        .into_iter()
        .take(limit)
        .filter_map(|(id, score)| by_id.remove(&id).map(|item| (item, score)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_beats_a_lone_first_place() {
        // Mirrors the property in rrf.ts: a document seen in two lists
        // outranks one that is first in a single list.
        let a = vec!["only-vector", "shared"];
        let b = vec!["only-lex", "shared"];
        let fused = rrf_fuse(&[a, b], |s: &&str| (*s).to_string(), 3, RRF_K);
        assert_eq!(fused[0], "shared");
    }

    #[test]
    fn k60_first_rank_contribution() {
        let lists = [vec!["x"]];
        let scored = rrf_fuse_scored(&lists, |s: &&str| (*s).to_string(), 1, RRF_K);
        let expect = 1.0 / (60.0 + 1.0);
        assert!((scored[0].1 - expect).abs() < 1e-12);
    }
}
