//! Curated agent names.
//!
//! Names on the board read as a natural bakery pair (`crumb-marshal`,
//! `sourdough-sentinel`), not two random words. Keep the whole name in this
//! list instead of combining independent adjective and noun pools, so a pair
//! can be chosen for how it sounds.
//!
//! `apps/mcp` asks the server for its auto-assigned name, so this list is the
//! single source of truth for generated names.

use std::collections::HashSet;

use rand::seq::SliceRandom;

/// Complete, curated names. Order does not matter; selection shuffles.
pub const CURATED: &[&str] = &[
    "crumb-marshal",
    "crusty-falcon",
    "rye-ranger",
    "sourdough-sentinel",
    "toasty-ciabatta",
    "zesty-pretzel",
    "brioche-bridge",
    "doughminator",
    "buttery-bandit",
    "flaky-fox",
    "golden-gander",
    "crusty-corsair",
    "saucy-scone",
    "nimble-naan",
    "merry-macaron",
    "plucky-pumpernickel",
    "snazzy-strudel",
    "bouncy-bagel",
    "peppy-pita",
    "hearty-hoagie",
    "spry-spelt",
    "brazen-brioche",
    "nimble-crumpet",
    "frosty-focaccia",
    "doughy-dynamo",
    "crumb-courier",
    "butter-brigadier",
    "toast-tactician",
    "pastry-pilot",
    "biscuit-baron",
    "waffle-warden",
    "scone-scribe",
    "pretzel-pioneer",
    "bagel-buccaneer",
    "loaf-lieutenant",
    "churro-champion",
    "ciabatta-captain",
    "sourdough-skipper",
    "brioche-bard",
    "crumpet-cartographer",
    "cruller-commander",
    "danish-daredevil",
    "eclair-emissary",
    "fritter-fortune",
    "gougere-guardian",
    "jelly-juggernaut",
    "kringle-knight",
    "macaron-maestro",
    "rugelach-ranger",
    "strudel-sorcerer",
    "babka-baroness",
    "challah-champion",
    "cornet-captain",
    "eclair-envoy",
    "grissini-guide",
    "hoagie-harbinger",
    "muffin-marshal",
    "scone-sentinel",
    "baguette-builder",
];

/// Names are case-insensitive and stored lowercased everywhere.
pub fn normalize(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Pick `count` fresh names the roster does not already use. Falls back to a
/// numeric suffix only when the whole curated list is taken.
pub fn pick_names(known: &HashSet<String>, count: usize) -> Vec<String> {
    let mut rng = rand::thread_rng();
    let mut pool: Vec<&str> = CURATED
        .iter()
        .copied()
        .filter(|name| !known.contains(*name))
        .collect();
    pool.shuffle(&mut rng);

    let mut out: Vec<String> = pool.into_iter().take(count).map(str::to_string).collect();
    if out.len() >= count {
        return out;
    }

    // Every curated name is taken: keep the same voice with a suffix.
    let mut suffix = 2u32;
    while out.len() < count && suffix < 10_000 {
        let Some(base) = CURATED.choose(&mut rng).copied() else {
            break;
        };
        let candidate = format!("{base}-{suffix}");
        if !known.contains(&candidate) && !out.contains(&candidate) {
            out.push(candidate);
        }
        suffix += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_names_are_lowercase_hyphenated() {
        for name in CURATED {
            assert_eq!(*name, normalize(name));
            assert!(
                name.contains('-') || *name == "doughminator",
                "{name} should be a natural pair"
            );
            assert!(!name.contains(' '), "{name} should use hyphens");
        }
    }

    #[test]
    fn selection_skips_known_and_is_unique() {
        let mut known = HashSet::new();
        known.insert("crumb-marshal".to_string());
        let picked = pick_names(&known, 5);
        assert_eq!(picked.len(), 5);
        assert!(!picked.contains(&"crumb-marshal".to_string()));
        let unique: HashSet<_> = picked.iter().collect();
        assert_eq!(unique.len(), picked.len());
        for name in &picked {
            assert!(CURATED.contains(&name.as_str()) || name.contains('-'));
        }
    }

    #[test]
    fn exhausted_list_gets_a_suffix() {
        let known: HashSet<String> = CURATED.iter().map(|s| s.to_string()).collect();
        let picked = pick_names(&known, 2);
        assert_eq!(picked.len(), 2);
        for name in &picked {
            assert!(name.contains('-'));
            assert!(!known.contains(name));
        }
    }
}
