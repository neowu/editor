use rustc_hash::FxHashSet;

const ADJECTIVES: &[&str] = &[
    "able", "agate", "airy", "alpine", "amber", "ample", "aqua", "arctic", "arid", "ashen", "astral", "autumn", "avid",
    "balmy", "birch", "bold", "boreal", "brave", "breezy", "brief", "bright", "brisk", "broad", "bronze", "calm",
    "cerith", "cheery", "civil", "clean", "clear", "clever", "cobalt", "cool", "copper", "coral", "cozy", "crisp",
    "cubic", "cyan", "deft", "dense", "dewy", "direct", "dusky", "dusty", "early", "earnest", "earthy", "elder",
    "elfin", "equal", "even", "exact", "faint", "fair", "fast", "fawn", "ferny", "fiery", "fine", "firm", "fleet",
    "floral", "focal", "fond", "frank", "fresh", "frosty", "full", "gentle", "gilded", "glacial", "glad", "glossy",
    "golden", "grand", "green", "gusty", "hale", "happy", "hardy", "hazel", "hearty", "hilly", "humble", "hushed",
    "icy", "ideal", "inky", "iron", "ivory", "jade", "jovial", "keen", "kind", "lapis", "leafy", "level", "light",
    "lilac", "limber", "lively", "lofty", "loyal", "lucid", "lunar", "major", "maple", "marshy", "mellow", "merry",
    "mild", "milky", "misty", "modest", "mossy", "muted", "narrow", "naval", "neat", "nimble", "noble", "north",
    "novel", "oaken", "ochre", "olive", "onyx", "opal", "optic", "ornate", "oval", "owed", "ozone", "pale", "pastel",
    "pearl", "pecan", "peppy", "pilot", "placid", "plain", "plucky", "plum", "plush", "poised", "polar", "polished",
    "poplar", "prime", "proof", "proud", "quartz", "quick", "quiet", "rainy", "rapid", "raspy", "ready", "regal",
    "roomy", "rooted", "rosy", "round", "royal", "ruddy", "russet", "sage", "salty", "sandy", "satin", "scenic",
    "sedge", "serene", "sheer", "silky", "silver", "sleek", "smart", "smooth", "snowy", "snug", "solar", "solid",
    "south", "spry", "stark", "steady", "steel", "steep", "still", "stocky", "stoic", "stony", "stout", "sturdy",
    "suede", "sunny", "supple", "sure", "tall", "tangy", "tawny", "teal", "terse", "thick", "tidal", "tidy", "timber",
    "topaz", "total", "trim", "tropic", "tulip", "upper", "urban", "vast", "velvet", "verde", "vivid", "vocal", "warm",
    "waxen", "west", "whole", "wide", "wild", "wise", "witty", "woven", "young", "zealous", "zephyr", "zesty", "zinc",
];

const NOUNS: &[&str] = &[
    "acorn", "almond", "anvil", "apricot", "arbor", "atlas", "badge", "badger", "basin", "bay", "beacon", "beam",
    "bell", "birch", "blade", "bloom", "bluff", "bobcat", "bolt", "breeze", "bridge", "brook", "bunting", "burrow",
    "cabin", "cairn", "canyon", "cape", "cedar", "chasm", "cliff", "clover", "coast", "cobble", "colt", "comet",
    "conch", "condor", "coral", "cove", "coyote", "crane", "crater", "creek", "crest", "curlew", "daisy", "dale",
    "dawn", "den", "dove", "drake", "drift", "drum", "dune", "dusk", "eagle", "eel", "egret", "elk", "emu", "falcon",
    "fawn", "fennel", "fern", "ferret", "ferry", "fig", "finch", "fjord", "flicker", "flint", "flower", "fox", "frost",
    "gale", "garnet", "gate", "gazelle", "geyser", "glade", "glen", "gorge", "granite", "grove", "gull", "harbor",
    "hare", "haven", "hawk", "hazel", "heath", "hedge", "heron", "hill", "hollow", "horizon", "ibis", "inlet", "isle",
    "ivy", "jackal", "jasper", "juniper", "kinglet", "kitten", "knoll", "lagoon", "lake", "lantern", "larch", "lark",
    "laurel", "lava", "leaf", "ledge", "lily", "linden", "lodge", "loft", "loon", "lotus", "mantle", "maple", "marble",
    "marsh", "marten", "meadow", "merlin", "mill", "minnow", "moon", "moose", "moss", "moth", "newt", "north",
    "nutmeg", "oak", "oasis", "obsidian", "orbit", "orchid", "oriole", "osprey", "otter", "owl", "palm", "panther",
    "pass", "peach", "peak", "pebble", "pelican", "peony", "perch", "pier", "pike", "pine", "plover", "plume", "pond",
    "poppy", "prairie", "prism", "quail", "quarry", "quartz", "rain", "rampart", "raven", "ravine", "reed", "reef",
    "ridge", "river", "robin", "rook", "rowan", "sage", "salmon", "sequoia", "shore", "shrew", "shrike", "sigma",
    "sky", "slope", "snipe", "snow", "sparrow", "spruce", "stag", "star", "starling", "stoat", "stone", "stork",
    "storm", "strand", "summit", "sycamore", "tern", "terrace", "thistle", "thorn", "thrush", "tide", "timber",
    "toucan", "trail", "trout", "tulip", "tundra", "turtle", "vale", "valley", "veranda", "violet", "viper", "vole",
    "walrus", "warbler", "willow", "wolf", "wren", "yak", "zenith",
];

/// Generates a worktree name in `"adjective-noun"` format (e.g. `"calm-river"`).
///
/// Tries up to 10 combinations derived from `seed`, skipping any name that already
/// appears in `existing_names`. Returns `None` if no unused name is found.
pub fn generate_worktree_name(existing_names: &[&str], mut seed: u64) -> Option<String> {
    let existing: FxHashSet<&str> = existing_names.iter().copied().collect();

    for _ in 0..10 {
        let adjective = ADJECTIVES[(splitmix64(&mut seed) % ADJECTIVES.len() as u64) as usize];
        let noun = NOUNS[(splitmix64(&mut seed) % NOUNS.len() as u64) as usize];
        let name = format!("{adjective}-{noun}");

        if !existing.contains(name.as_str()) {
            return Some(name);
        }
    }

    None
}

/// Non-cryptographic seed; names only need to look random, collisions are retried.
pub fn random_seed() -> u64 {
    use std::hash::BuildHasher as _;
    std::hash::RandomState::new().hash_one(std::time::SystemTime::now())
}

const fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_worktree_name_is_adjective_noun() {
        let name = generate_worktree_name(&[], 42).unwrap();
        let (adjective, noun) = name.split_once('-').unwrap();
        assert!(ADJECTIVES.contains(&adjective));
        assert!(NOUNS.contains(&noun));
    }

    #[test]
    fn generate_worktree_name_is_deterministic_per_seed() {
        assert_eq!(generate_worktree_name(&[], 7), generate_worktree_name(&[], 7));
    }

    #[test]
    fn generate_worktree_name_skips_existing() {
        let first = generate_worktree_name(&[], 1).unwrap();
        let second = generate_worktree_name(&[first.as_str()], 1).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn generate_worktree_name_gives_up_after_ten_tries() {
        let mut taken = Vec::new();
        let mut seed = 3;
        for _ in 0..10 {
            let adjective = ADJECTIVES[(splitmix64(&mut seed) % ADJECTIVES.len() as u64) as usize];
            let noun = NOUNS[(splitmix64(&mut seed) % NOUNS.len() as u64) as usize];
            taken.push(format!("{adjective}-{noun}"));
        }
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        assert_eq!(generate_worktree_name(&taken, 3), None);
    }

    #[test]
    fn splitmix64_advances_state() {
        let mut state = 0;
        let a = splitmix64(&mut state);
        let b = splitmix64(&mut state);
        assert_ne!(a, b);
        assert_ne!(state, 0);
    }

    #[test]
    fn random_seed_varies() {
        assert_ne!(random_seed(), random_seed());
    }
}
