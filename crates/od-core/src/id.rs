//! Object identity.
//!
//! Ids are `(actor, seq)` pairs rather than a dense index, because two people
//! editing the same drawing offline must be able to create objects without
//! coordinating. That is the same property the sync layer needs later
//! (`docs/03-data-model.md`), and retrofitting it onto a dense index means
//! rewriting every reference in every file ever saved. So it is paid for now.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Identifies one editing session's stream of new objects. Generated once per
/// device+document and stored in the file, so ids stay stable across saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActorId(pub u64);

impl ActorId {
    /// The actor used for objects created by an importer, where there is no
    /// human author. Keeping it fixed makes imports reproducible, which is what
    /// the round-trip tests compare against.
    pub const IMPORT: Self = Self(1);

    /// The actor for objects a fresh document seeds itself with.
    pub const SYSTEM: Self = Self(0);
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", self.0)
    }
}

/// A stable reference to an object within a document.
///
/// Serialised as the string `"actor-seq"` rather than a struct: ids are used as
/// map keys throughout the document, and JSON — plus most of the formats we
/// export to — only accepts string keys. Encoding it once here keeps every
/// container that holds ids serialisable without a bespoke key adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    pub actor: ActorId,
    pub seq: u64,
}

impl ObjectId {
    #[must_use]
    pub const fn new(actor: ActorId, seq: u64) -> Self {
        Self { actor, seq }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}-{:x}", self.actor.0, self.seq)
    }
}

impl Serialize for ObjectId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("malformed object id: {0}")]
pub struct ParseIdError(String);

impl FromStr for ObjectId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| ParseIdError(s.to_owned()))?;
        let actor = u64::from_str_radix(a, 16).map_err(|_| ParseIdError(s.to_owned()))?;
        let seq = u64::from_str_radix(b, 16).map_err(|_| ParseIdError(s.to_owned()))?;
        Ok(Self::new(ActorId(actor), seq))
    }
}

/// Hands out ids for one actor. Monotonic, so an id is never reused even after
/// the object it named is deleted — dangling references must be detectable
/// rather than silently rebound to something new.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdGenerator {
    actor: ActorId,
    next: u64,
}

impl IdGenerator {
    #[must_use]
    pub const fn new(actor: ActorId) -> Self {
        Self { actor, next: 1 }
    }

    #[must_use]
    pub const fn actor(&self) -> ActorId {
        self.actor
    }

    pub fn next_id(&mut self) -> ObjectId {
        let id = ObjectId::new(self.actor, self.next);
        self.next += 1;
        id
    }

    /// Ensures future ids do not collide with one already present — used after
    /// loading a file written by this same actor.
    pub fn observe(&mut self, id: ObjectId) {
        if id.actor == self.actor && id.seq >= self.next {
            self.next = id.seq + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_per_actor_without_coordination() {
        let mut a = IdGenerator::new(ActorId(0xa1));
        let mut b = IdGenerator::new(ActorId(0xb2));
        let from_a: Vec<_> = (0..3).map(|_| a.next_id()).collect();
        let from_b: Vec<_> = (0..3).map(|_| b.next_id()).collect();
        for x in &from_a {
            assert!(!from_b.contains(x), "actors must not collide");
        }
    }

    #[test]
    fn ids_are_never_reused() {
        let mut g = IdGenerator::new(ActorId(7));
        let first = g.next_id();
        let second = g.next_id();
        assert_ne!(first, second);
        assert!(second.seq > first.seq);
    }

    #[test]
    fn observing_a_loaded_id_avoids_collision() {
        let mut g = IdGenerator::new(ActorId(7));
        g.observe(ObjectId::new(ActorId(7), 500));
        assert!(g.next_id().seq > 500);
        // Another actor's ids do not move our counter.
        g.observe(ObjectId::new(ActorId(9), 9000));
        assert!(g.next_id().seq < 9000);
    }

    #[test]
    fn ids_survive_being_used_as_map_keys() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert(ObjectId::new(ActorId(1), 2), "a");
        let json = serde_json::to_string(&m).expect("serialises");
        assert_eq!(json, r#"{"1-2":"a"}"#);
        let back: BTreeMap<ObjectId, String> = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn display_and_parse_round_trip() {
        let id = ObjectId::new(ActorId(0xdead), 0xbeef);
        assert_eq!(id.to_string(), "dead-beef");
        assert_eq!("dead-beef".parse::<ObjectId>().expect("parses"), id);
        assert!("nonsense".parse::<ObjectId>().is_err());
    }
}
