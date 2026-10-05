//! One seed in, many seeds out.
//!
//! The user supplies a single `--rng-seed`. A run needs more than one: a
//! seed per chain, and -- because the chain is run in segments so it can be
//! stopped between them -- a seed per segment of each chain. Those have to
//! be derived from the one the user gave, or the whole run stops being
//! reproducible from a number they can write down.
//!
//! # Why not addition
//!
//! The obvious derivation, `seed + chain` and then `+ segment`, collides.
//! With chains at 11, 12, 13 and segments adding 0, 1, 2, chain 1's second
//! segment draws seed 12, which is chain 2's first. rustrecom hands the
//! seed to `SmallRng::seed_from_u64`, so equal seeds mean an identical
//! stream, and the two chains would run the same random walk from that
//! point. R-hat would then report better agreement than the chains earned,
//! which is the dangerous direction for a convergence diagnostic to fail in.
//!
//! Mixing instead of adding does not make collisions impossible -- the
//! output is 64 bits, so two coordinates can land together with probability
//! around 2^-64 -- but it stops them being structural. Addition collides on
//! every multi-chain run; this does not.

/// The seed for one segment of one chain.
///
/// Distinct `(chain, segment)` pairs give unrelated streams, and the whole
/// family is reproducible from `seed` alone.
pub fn derive(seed: u64, chain: u64, segment: u64) -> u64 {
    let mut z = seed;
    for coordinate in [chain, segment] {
        z = splitmix64(z.wrapping_add(coordinate));
    }
    z
}

/// splitmix64, the standard finalizer. Every output bit depends on every
/// input bit, which is the only property this needs.
fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The failure that motivated this: with addition, chain 1 segment 1
    /// draws the same seed as chain 2 segment 0. Nothing here may.
    #[test]
    fn no_two_chain_segment_pairs_share_a_seed() {
        let mut seen = HashSet::new();
        for chain in 0..64 {
            for segment in 0..4096 {
                let s = derive(11, chain, segment);
                assert!(
                    seen.insert(s),
                    "chain {chain} segment {segment} reuses a seed already issued"
                );
            }
        }
        assert_eq!(seen.len(), 64 * 4096);
    }

    /// The specific collision addition produces, stated as a test so the
    /// derivation cannot quietly regress to it.
    #[test]
    fn addition_would_have_collided_here() {
        // What the old scheme did: seed + chain + segment. Chain 1's second
        // segment and chain 2's first both land on 13.
        let added = |chain: u64, segment: u64| 11 + chain + segment;
        assert_eq!(added(1, 1), added(2, 0));
        assert_ne!(derive(11, 1, 1), derive(11, 2, 0));
    }

    /// Different user seeds must not alias either, segment for segment.
    #[test]
    fn neighbouring_user_seeds_stay_apart() {
        for segment in 0..256 {
            assert_ne!(derive(11, 0, segment), derive(12, 0, segment));
        }
    }

    /// Fixed vectors. A change here changes every ensemble the tool has
    /// produced, so it should be deliberate enough to edit a test for.
    #[test]
    fn derivation_is_pinned() {
        assert_eq!(derive(11, 0, 0), 10_520_313_552_068_553_934);
        assert_eq!(derive(11, 1, 0), 17_827_584_815_388_034_219);
        assert_eq!(derive(0, 0, 0), 12_035_550_249_420_947_055);
    }
}
