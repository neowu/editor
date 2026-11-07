use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;

use crate::Bias;
use crate::ContextLessSummary;
use crate::Dimension;
use crate::Edit;
use crate::Item;
use crate::KeyedItem;
use crate::SeekTarget;
use crate::SumTree;

/// A cheaply-cloneable ordered map based on a [SumTree](crate::SumTree).
#[derive(Clone, PartialEq, Eq)]
pub struct TreeMap<K, V>(SumTree<MapEntry<K, V>>)
where
    K: Clone + Ord,
    V: Clone;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry<K, V> {
    key: K,
    value: V,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MapKey<K>(Option<K>);

impl<K> Default for MapKey<K> {
    fn default() -> Self {
        Self(None)
    }
}

#[derive(Clone, Debug)]
pub struct MapKeyRef<'a, K>(Option<&'a K>);

impl<K> Default for MapKeyRef<'_, K> {
    fn default() -> Self {
        Self(None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeSet<K>(TreeMap<K, ()>)
where
    K: Clone + Ord;

impl<K: Clone + Ord, V: Clone> TreeMap<K, V> {
    /// Whether both maps share the same tree, i.e. one is an unmodified clone of the other.
    pub fn ptr_eq(&self, other: &TreeMap<K, V>) -> bool {
        self.0.ptr_eq(&other.0)
    }

    pub fn from_ordered_entries(entries: impl IntoIterator<Item = (K, V)>) -> Self {
        let tree = SumTree::from_iter(entries.into_iter().map(|(key, value)| MapEntry { key, value }), ());
        Self(tree)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let (.., item) = self.0.find::<MapKeyRef<'_, K>, _>((), &MapKeyRef(Some(key)), Bias::Left);
        if let Some(item) = item {
            if Some(key) == item.key().0.as_ref() { Some(&item.value) } else { None }
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.0.insert_or_replace(MapEntry { key, value }, ());
    }

    pub fn insert_or_replace(&mut self, key: K, value: V) -> Option<V> {
        self.0.insert_or_replace(MapEntry { key, value }, ()).map(|it| it.value)
    }

    pub fn extend(&mut self, iter: impl IntoIterator<Item = (K, V)>) {
        let edits: Vec<_> = iter.into_iter().map(|(key, value)| Edit::Insert(MapEntry { key, value })).collect();
        self.0.edit(edits, ());
    }

    pub fn clear(&mut self) {
        self.0 = SumTree::default();
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let mut removed = None;
        let mut cursor = self.0.cursor::<MapKeyRef<'_, K>>(());
        let key = MapKeyRef(Some(key));
        let mut new_tree = cursor.slice(&key, Bias::Left);
        if key.cmp(&cursor.end(), ()) == Ordering::Equal {
            removed = Some(cursor.item().unwrap().value.clone());
            cursor.next();
        }
        new_tree.append(cursor.suffix(), ());
        drop(cursor);
        self.0 = new_tree;
        removed
    }

    /// Returns the key-value pair with the greatest key less than or equal to the given key.
    pub fn closest(&self, key: &K) -> Option<(&K, &V)> {
        let mut cursor = self.0.cursor::<MapKeyRef<'_, K>>(());
        let key = MapKeyRef(Some(key));
        cursor.seek(&key, Bias::Right);
        cursor.prev();
        cursor.item().map(|item| (&item.key, &item.value))
    }

    pub fn iter_from<'a>(&'a self, from: &K) -> impl Iterator<Item = (&'a K, &'a V)> + 'a {
        let mut cursor = self.0.cursor::<MapKeyRef<'_, K>>(());
        let from_key = MapKeyRef(Some(from));
        cursor.seek(&from_key, Bias::Left);

        cursor.map(|map_entry| (&map_entry.key, &map_entry.value))
    }

    pub fn update<F, T>(&mut self, key: &K, f: F) -> Option<T>
    where
        F: FnOnce(&mut V) -> T,
    {
        let mut cursor = self.0.cursor::<MapKeyRef<'_, K>>(());
        let key = MapKeyRef(Some(key));
        let mut new_tree = cursor.slice(&key, Bias::Left);
        let mut result = None;
        if key.cmp(&cursor.end(), ()) == Ordering::Equal {
            let mut updated = cursor.item().unwrap().clone();
            result = Some(f(&mut updated.value));
            new_tree.push(updated, ());
            cursor.next();
        }
        new_tree.append(cursor.suffix(), ());
        drop(cursor);
        self.0 = new_tree;
        result
    }

    pub fn retain<F: FnMut(&K, &V) -> bool>(&mut self, mut predicate: F) {
        let mut new_map = SumTree::<MapEntry<K, V>>::default();

        let mut cursor = self.0.cursor::<MapKeyRef<'_, K>>(());
        cursor.next();
        while let Some(item) = cursor.item() {
            if predicate(&item.key, &item.value) {
                new_map.push(item.clone(), ());
            }
            cursor.next();
        }
        drop(cursor);

        self.0 = new_map;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        self.0.iter().map(|entry| (&entry.key, &entry.value))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.0.iter().map(|entry| &entry.value)
    }

    pub fn first(&self) -> Option<(&K, &V)> {
        self.0.first().map(|entry| (&entry.key, &entry.value))
    }

    pub fn last(&self) -> Option<(&K, &V)> {
        self.0.last().map(|entry| (&entry.key, &entry.value))
    }
}

impl<K, V> Debug for TreeMap<K, V>
where
    K: Clone + Debug + Ord,
    V: Clone + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> Default for TreeMap<K, V>
where
    K: Clone + Ord,
    V: Clone,
{
    fn default() -> Self {
        Self(SumTree::default())
    }
}

impl<K, V> Item for MapEntry<K, V>
where
    K: Clone + Ord,
    V: Clone,
{
    type Summary = MapKey<K>;

    fn summary(&self, _cx: ()) -> Self::Summary {
        self.key()
    }
}

impl<K, V> KeyedItem for MapEntry<K, V>
where
    K: Clone + Ord,
    V: Clone,
{
    type Key = MapKey<K>;

    fn key(&self) -> Self::Key {
        MapKey(Some(self.key.clone()))
    }
}

impl<K> ContextLessSummary for MapKey<K>
where
    K: Clone,
{
    fn zero() -> Self {
        MapKey::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        *self = summary.clone()
    }
}

impl<'a, K> Dimension<'a, MapKey<K>> for MapKeyRef<'a, K>
where
    K: Clone + Ord,
{
    fn zero(_cx: ()) -> Self {
        MapKeyRef::default()
    }

    fn add_summary(&mut self, summary: &'a MapKey<K>, _: ()) {
        self.0 = summary.0.as_ref();
    }
}

impl<'a, K> SeekTarget<'a, MapKey<K>, MapKeyRef<'a, K>> for MapKeyRef<'_, K>
where
    K: Clone + Ord,
{
    fn cmp(&self, cursor_location: &MapKeyRef<K>, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.0)
    }
}

impl<K> Default for TreeSet<K>
where
    K: Clone + Ord,
{
    fn default() -> Self {
        Self(TreeMap::default())
    }
}

impl<K> TreeSet<K>
where
    K: Clone + Ord,
{
    pub fn from_ordered_entries(entries: impl IntoIterator<Item = K>) -> Self {
        Self(TreeMap::from_ordered_entries(entries.into_iter().map(|key| (key, ()))))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, key: K) {
        self.0.insert(key, ());
    }

    pub fn remove(&mut self, key: &K) -> bool {
        self.0.remove(key).is_some()
    }

    pub fn extend(&mut self, iter: impl IntoIterator<Item = K>) {
        self.0.extend(iter.into_iter().map(|key| (key, ())));
    }

    pub fn contains(&self, key: &K) -> bool {
        self.0.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> + '_ {
        self.0.iter().map(|(k, _)| k)
    }

    pub fn iter_from<'a>(&'a self, key: &K) -> impl Iterator<Item = &'a K> + 'a {
        self.0.iter_from(key).map(move |(k, _)| k)
    }
}

#[cfg(test)]
mod ptr_eq_tests {
    use super::TreeMap;

    #[test]
    // Also covers `SumTree::ptr_eq`, which this delegates to.
    fn ptr_eq_holds_for_clones_until_modified() {
        let map: TreeMap<u32, u32> = TreeMap::from_ordered_entries([(1, 1)]);
        let mut clone = map.clone();
        assert!(map.ptr_eq(&clone));
        clone.insert(2, 2);
        assert!(!map.ptr_eq(&clone));
    }
}
