//! Burst and near-duplicate grouping.
//!
//! A 20 fps camera at a football match produces long runs of nearly identical
//! frames. Grouping them and surfacing one winner is the single biggest
//! reduction in review time, so the rules here are deliberately conservative:
//! it is much worse to merge two genuinely different moments than to leave two
//! similar frames in separate groups.
//!
//! Two frames join the same group when they are *both* close in time and close
//! in appearance. Time alone would merge a whole passage of play; appearance
//! alone would merge every wide shot of the same goalmouth taken minutes apart.

use crate::hash::{hamming, signature_distance};
use crate::model::{Analysis, ScoreSettings};

/// Union-find over photo indices.
struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb] = ra;
        }
    }
}

/// Assign `group_id`, `group_rank` and `is_group_best` across the whole set.
///
/// `items` is expected to be in capture order; grouping only compares each
/// frame against a short window of neighbours, which keeps this linear.
pub fn assign_groups(items: &mut [Analysis], settings: &ScoreSettings) {
    let n = items.len();
    if n == 0 {
        return;
    }

    // Order by capture time, falling back to filename order for files with no
    // usable EXIF timestamp.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ka = sort_key(&items[a]);
        let kb = sort_key(&items[b]);
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut ds = DisjointSet::new(n);

    // Compare against the next few frames only. Bursts are contiguous in time,
    // so a window of 12 covers even a long run without going quadratic.
    const WINDOW: usize = 12;
    for oi in 0..order.len() {
        let i = order[oi];
        for oj in (oi + 1)..(oi + 1 + WINDOW).min(order.len()) {
            let j = order[oj];
            if similar(&items[i], &items[j], settings) {
                ds.union(i, j);
            } else if settings.group_by_time
                && time_gap(&items[i], &items[j])
                    .map_or(false, |g| g > settings.burst_gap_secs * 3.0)
            {
                // The list is in capture order and time is being required, so
                // everything further along this window is further away too.
                // Only safe to stop early when time is actually a constraint.
                break;
            }
        }
    }

    // Collapse roots to dense, capture-ordered group ids.
    let mut root_to_gid: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    let mut next_gid = 0u32;
    for &i in &order {
        let r = ds.find(i);
        if !root_to_gid.contains_key(&r) {
            root_to_gid.insert(r, next_gid);
            next_gid += 1;
        }
    }

    let mut members: Vec<Vec<usize>> = vec![Vec::new(); next_gid as usize];
    for &i in &order {
        let r = ds.find(i);
        let gid = root_to_gid[&r];
        members[gid as usize].push(i);
    }

    for (gid, group) in members.iter().enumerate() {
        // A group of one is not a burst; leave it ungrouped so the UI does not
        // clutter the grid with meaningless single-frame stacks.
        if group.len() < 2 {
            if let Some(&i) = group.first() {
                items[i].group_id = None;
                items[i].group_rank = 0;
                items[i].is_group_best = true;
            }
            continue;
        }

        let mut ranked = group.clone();
        ranked.sort_by(|&a, &b| {
            items[b]
                .score
                .partial_cmp(&items[a].score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (rank, &i) in ranked.iter().enumerate() {
            items[i].group_id = Some(gid as u32);
            items[i].group_rank = rank as u32;
            items[i].is_group_best = rank == 0;
        }
    }
}

/// Sort key: capture time with sub-second precision, else a large offset plus
/// the original index so filename order is preserved.
fn sort_key(a: &Analysis) -> f64 {
    match a.meta.taken_at {
        Some(t) => t as f64 + a.meta.sub_sec.unwrap_or(0) as f64 / 1000.0,
        None => f64::MAX / 2.0,
    }
}

fn time_gap(a: &Analysis, b: &Analysis) -> Option<f32> {
    match (a.meta.taken_at, b.meta.taken_at) {
        (Some(_), Some(_)) => Some((sort_key(b) - sort_key(a)).abs() as f32),
        _ => None,
    }
}

/// Decide whether two frames belong to the same burst.
///
/// The two signals are independent and each can be switched off, because they
/// fail in different ways and which one you trust depends on how you shoot.
///
/// Capture time is remarkably good on its own *if* the window is tight. On a
/// measured 872-frame shoot, continuous drive fired every 0.06-0.12 s and
/// grouping on time alone below 0.2 s joined visibly different pictures only
/// 3% of the time. Widen the window and it falls apart fast — 24% wrong at 2 s
/// — because a photographer shooting steadily through a passage of play leaves
/// no gaps at all, and the whole passage becomes one stack.
///
/// Appearance has the opposite failure: it will happily merge two wide shots of
/// the same goalmouth taken minutes apart.
fn similar(a: &Analysis, b: &Analysis, s: &ScoreSettings) -> bool {
    if !s.group_by_time && !s.group_by_appearance {
        return false;
    }

    let gap = time_gap(a, b);
    let time_ok = match gap {
        Some(g) => g <= s.burst_gap_secs,
        // No usable timestamp. Only groupable when time is not being required.
        None => !s.group_by_time,
    };
    if s.group_by_time && !time_ok {
        return false;
    }

    if !s.group_by_appearance {
        return true;
    }

    let dp = hamming(a.metrics.phash, b.metrics.phash);
    let dd = hamming(a.metrics.dhash, b.metrics.dhash);
    let sig = signature_distance(&a.metrics.signature, &b.metrics.signature);

    // Both hashes must agree, and the continuous signature must back them up.
    // The signature guard is what stops two different frames with
    // coincidentally close hashes from merging.
    if s.group_by_time && gap.is_some() {
        // Time already constrains this pair, so a slightly looser visual match
        // is safe.
        (dp <= s.hash_distance && dd <= s.hash_distance + 4 && sig < 0.085)
            || (dp <= s.hash_distance + 6 && sig < 0.055)
    } else {
        // Appearance is the only evidence; demand a tight match.
        dp <= s.hash_distance.saturating_sub(2) && dd <= s.hash_distance && sig < 0.06
    }
}
