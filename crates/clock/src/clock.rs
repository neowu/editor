use std::cmp;
use std::fmt;

/// A logical timestamp. Buffers have a single writer, so one counter orders
/// every event.
pub type Seq = u32;

/// A timestamp, used to identify edits, undos and transactions.
#[derive(Clone, Copy, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Lamport {
    pub value: Seq,
}

/// The newest timestamp a buffer has applied. Because edits are applied in
/// timestamp order, every timestamp up to it has been seen.
#[derive(Clone, Default, Hash, Eq, PartialEq)]
pub struct Global(Seq);

impl Global {
    pub const fn new() -> Self {
        Self(0)
    }

    pub const fn value(&self) -> Seq {
        self.0
    }

    pub fn observe(&mut self, timestamp: Lamport) {
        self.0 = cmp::max(self.0, timestamp.value);
    }

    /// Takes the newer of both versions.
    pub fn join(&mut self, other: &Self) {
        self.0 = cmp::max(self.0, other.0);
    }

    pub const fn observed(&self, timestamp: Lamport) -> bool {
        self.0 >= timestamp.value
    }

    pub const fn observed_all(&self, other: &Self) -> bool {
        self.0 >= other.0
    }

    pub const fn changed_since(&self, other: &Self) -> bool {
        self.0 > other.0
    }
}

impl Lamport {
    pub const MIN: Self = Self { value: Seq::MIN };

    pub const MAX: Self = Self { value: Seq::MAX };

    pub const fn new() -> Self {
        Self { value: 1 }
    }

    pub const fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.value += 1;
        timestamp
    }
}

impl Default for Lamport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Lamport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::MAX => write!(f, "Lamport {{MAX}}"),
            Self::MIN => write!(f, "Lamport {{MIN}}"),
            _ => write!(f, "Lamport {{{}}}", self.value),
        }
    }
}

impl fmt::Debug for Global {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Global {{{}}}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: Seq) -> Global {
        Global(value)
    }

    #[test]
    fn lamport_ticks_and_orders() {
        let mut clock = Lamport::new();
        let first = clock.tick();
        let second = clock.tick();
        assert_eq!(first.value, 1);
        assert!(first < second);
        assert!(Lamport::MIN < first && second < Lamport::MAX);
    }

    #[test]
    fn global_observe_and_join() {
        let mut global = Global::new();
        global.observe(Lamport { value: 3 });
        global.observe(Lamport { value: 2 });
        assert_eq!(global.value(), 3);

        global.join(&version(5));
        assert_eq!(global.value(), 5);
    }

    #[test]
    fn global_comparisons() {
        let global = version(4);
        assert!(global.observed(Lamport { value: 4 }));
        assert!(!global.observed(Lamport { value: 5 }));
        assert!(global.observed_all(&version(4)));
        assert!(!global.observed_all(&version(5)));
        assert!(global.changed_since(&version(3)));
        assert!(!global.changed_since(&version(4)));
    }
}
