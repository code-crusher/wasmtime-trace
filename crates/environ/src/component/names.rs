use crate::collections::TryCow;
use crate::error::{Result, bail};
use crate::{Atom, StringPool, prelude::*};
use alloc::sync::Arc;
use core::borrow::Borrow;
use core::hash::Hash;
use semver::Version;
use serde_derive::{Deserialize, Serialize};

/// A semver-aware map for imports/exports of a component.
///
/// This data structure is used when looking up the names of imports/exports of
/// a component to enable semver-compatible matching of lookups. This will
/// enable lookups of `a:b/c@0.2.0` to match entries defined as `a:b/c@0.2.1`
/// which is currently considered a key feature of WASI's compatibility story.
///
/// On the outside this looks like a map of `K` to `V`.
#[derive(Serialize, Deserialize, Debug)]
pub struct NameMap<K, V>
where
    K: TryClone + Hash + Eq + Ord,
{
    /// A map of keys to the value that they define.
    ///
    /// Note that this map is "exact" where the name here is the exact name that
    /// was specified when the `insert` was called. This doesn't have any
    /// semver-mangling or anything like that.
    ///
    /// This map is always consulted first during lookups.
    definitions: TryIndexMap<K, V>,

    /// An auxiliary map tracking semver-compatible names. This is a map from
    /// "semver compatible alternate name" to a name present in `definitions`
    /// and the semver version it was registered at.
    ///
    /// An example map would be:
    ///
    /// ```text
    /// {
    ///     "a:b/c@0.2": ("a:b/c@0.2.1", 0.2.1),
    ///     "a:b/c@2": ("a:b/c@2.0.0+abc", 2.0.0+abc),
    /// }
    /// ```
    ///
    /// As names are inserted into `definitions` each name may have up to one
    /// semver-compatible name with extra numbers/info chopped off which is
    /// inserted into this map. This map is the lookup table from `@0.2` to
    /// `@0.2.x` where `x` is what was inserted manually.
    ///
    /// The `Version` here is tracked to ensure that when multiple versions on
    /// one track are defined that only the maximal version here is retained.
    alternate_lookups: TryIndexMap<K, (K, TryVersion)>,
}

/// A wrapper around `Version` that implements `TryClone`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct TryVersion(Arc<Version>);

impl TryFrom<Version> for TryVersion {
    type Error = OutOfMemory;

    fn try_from(value: Version) -> Result<Self, Self::Error> {
        Ok(Self(try_new::<Arc<_>>(value)?))
    }
}

impl TryClone for TryVersion {
    #[inline]
    fn try_clone(&self) -> Result<Self, OutOfMemory> {
        Ok(Self(self.0.clone()))
    }
}

impl core::ops::Deref for TryVersion {
    type Target = Version;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Borrow<Version> for TryVersion {
    #[inline]
    fn borrow(&self) -> &Version {
        &self.0
    }
}

impl serde::Serialize for TryVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for TryVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let v = Version::deserialize(deserializer)?;
        let v = try_new::<Arc<_>>(v).map_err(|oom| D::Error::custom(oom))?;
        Ok(Self(v))
    }
}

impl<K, V> TryClone for NameMap<K, V>
where
    K: TryClone + Hash + Eq + Ord,
    V: TryClone,
{
    fn try_clone(&self) -> Result<Self, OutOfMemory> {
        Ok(Self {
            definitions: self.definitions.try_clone()?,
            alternate_lookups: self.alternate_lookups.try_clone()?,
        })
    }
}

impl<K, V> NameMap<K, V>
where
    K: TryClone + Hash + Eq + Ord,
{
    /// Inserts the `name` specified into this map.
    ///
    /// The name is intern'd through the `cx` argument and shadowing is
    /// controlled by the `allow_shadowing` variable.
    ///
    /// This function will automatically insert an entry in
    /// `self.alternate_lookups` if `name` is a semver-looking name.
    ///
    /// Returns an error if `allow_shadowing` is `false` and the `name` is
    /// already present in this map (by exact match). Otherwise returns the
    /// intern'd version of `name`.
    pub fn insert<I>(&mut self, name: &str, cx: &mut I, allow_shadowing: bool, item: V) -> Result<K>
    where
        I: NameMapIntern<Key = K>,
        I::BorrowedKey: Hash + Eq,
    {
        // Always insert `name` and `item` as an exact definition.
        let key = cx.intern(name)?;
        if !allow_shadowing && self.definitions.contains_key(&key) {
            bail!("map entry `{name}` defined twice")
        }
        self.definitions.insert(key.try_to_owned()?, item)?;

        // If `name` is a semver-looking thing, like `a:b/c@1.0.0`, then also
        // insert an entry in the semver-compatible map under a key such as
        // `a:b/c@1`.
        //
        // This key is used during `get` later on.
        if let Some((alternate_key, version)) = alternate_lookup_key(name) {
            let alternate_key = cx.intern(alternate_key)?;
            let version = TryVersion::try_from(version)?;
            if let Some((prev_key, prev_version)) = self.alternate_lookups.insert(
                alternate_key.try_clone()?,
                (key.try_clone()?, version.clone()),
            )? {
                // Prefer the latest version, so only do this if we're
                // greater than the prior version.
                if version < prev_version {
                    self.alternate_lookups
                        .insert(alternate_key, (prev_key, prev_version))?;
                }
            }
        }
        Ok(key)
    }

    /// Looks up `name` within this map, using the interning specified by
    /// `cx`.
    ///
    /// This may return a definition even if `name` wasn't exactly defined in
    /// this map, such as looking up `a:b/c@0.2.0` when the map only has
    /// `a:b/c@0.2.1` defined.
    pub fn get<I>(&self, name: &str, cx: &I) -> Option<&V>
    where
        I: NameMapIntern<Key = K>,
        I::Key: Borrow<I::BorrowedKey>,
        I::BorrowedKey: Hash + Eq,
    {
        // First look up an exact match and if that's found return that. This
        // enables defining multiple versions in the map and the requested
        // version is returned if it matches exactly.
        let candidate = cx.lookup(name).and_then(|k| self.definitions.get(&*k));
        if let Some(def) = candidate {
            return Some(def);
        }

        // Failing that, then try to look for a semver-compatible alternative.
        // This looks up the key based on `name`, if any, and then looks to see
        // if that was intern'd in `strings`. Given all that look to see if it
        // was defined in `alternate_lookups` and finally at the end that exact
        // key is then used to look up again in `self.definitions`.
        let (alternate_name, _version) = alternate_lookup_key(name)?;
        let alternate_key = cx.lookup(alternate_name)?;
        let (exact_key, _version) = self.alternate_lookups.get(&alternate_key)?;
        self.definitions.get(exact_key.borrow())
    }

    /// Returns an iterator over inserted values in this map.
    ///
    /// Note that the iterator return yields intern'd keys and additionally does
    /// not do anything special with semver names and such, it only literally
    /// yields what's been inserted with [`NameMap::insert`].
    pub fn raw_iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.definitions.iter()
    }

    /// TODO
    pub fn raw_get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.definitions.get_mut(key)
    }
}

impl<K, V> Default for NameMap<K, V>
where
    K: TryClone + Hash + Eq + Ord,
{
    fn default() -> NameMap<K, V> {
        NameMap {
            definitions: Default::default(),
            alternate_lookups: Default::default(),
        }
    }
}

/// A helper trait used in conjunction with [`NameMap`] to optionally intern
/// keys to non-strings.
pub trait NameMapIntern {
    /// The key that this interning context generates.
    type Key: Borrow<Self::BorrowedKey>;

    /// The borrowed version of the key type.
    type BorrowedKey: ?Sized + TryToOwned<Owned = Self::Key>;

    /// Inserts `s` into `self` and returns the intern'd key `Self::Key`.
    fn intern(&mut self, s: &str) -> Result<Self::Key, OutOfMemory>;

    /// Looks up `s` in `self` returning `Some` if it was found or `None` if
    /// it's not present.
    fn lookup<'a>(&'a self, s: &'a str) -> Option<TryCow<'a, Self::BorrowedKey>>;
}

/// For use with [`NameMap`] when no interning should happen and instead string
/// keys are copied as-is.
pub struct NameMapNoIntern;

impl NameMapIntern for NameMapNoIntern {
    type Key = TryString;
    type BorrowedKey = str;

    fn intern(&mut self, s: &str) -> Result<Self::Key, OutOfMemory> {
        TryString::try_from(s)
    }

    fn lookup<'a>(&'a self, s: &'a str) -> Option<TryCow<'a, Self::BorrowedKey>> {
        Some(TryCow::Borrowed(s))
    }
}

impl NameMapIntern for StringPool {
    type Key = Atom;
    type BorrowedKey = Atom;

    fn intern(&mut self, string: &str) -> Result<Atom, OutOfMemory> {
        self.insert(string)
    }

    fn lookup(&self, string: &str) -> Option<TryCow<'_, Atom>> {
        self.get_atom(string).map(TryCow::Owned)
    }
}

/// Determines a version-based "alternate lookup key" for the `name` specified.
///
/// Some examples are:
///
/// * `foo` => `None`
/// * `foo:bar/baz` => `None`
/// * `foo:bar/baz@1.1.2` => `Some(foo:bar/baz@1)`
/// * `foo:bar/baz@0.1.0` => `Some(foo:bar/baz@0.1)`
/// * `foo:bar/baz@0.0.1` => `None`
/// * `foo:bar/baz@0.1.0-rc.2` => `None`
///
/// Note that canonical names such as `foo:bar/baz@1` or `foo:bar/baz@0.1`,
/// which are already reduced, are returned as-is. This enables lookups of a
/// canonical name to match a full version (e.g. `foo:bar/baz@1.2.3`) and
/// vice versa.
///
/// This alternate lookup key is intended to serve the purpose where a
/// semver-compatible definition can be located, if one is defined, at perhaps
/// either a newer or an older version.
pub fn alternate_lookup_key(name: &str) -> Option<(&str, Version)> {
    let at = name.find('@')?;
    let version_string = &name[at + 1..];

    // A "canonical" name such as `a:b/c@1` or `a:b/c@0.1` doesn't have a full
    // semver version, so fill in the missing numbers here to get a `Version`
    // for the compatible track that the name is on.
    let (alternate_key, version) = match Version::parse(version_string) {
        // Full semver versions are reduced below to the canonical form of the
        // version.
        Ok(version) => (
            reduce_to_canonical(name, at, version_string, &version)?,
            version,
        ),

        // Canonical names such as `@1` or `@0.1` are already in reduced form
        // and are returned as-is so they hash to the same key as all the full
        // versions on the same compatible track. That enables matching a
        // definition of a canonical name with a lookup of a full version, or
        // vice versa, during the transition to canonical names.
        Err(_) => match canonical_version(version_string) {
            Some(version) => (&name[..at + 1 + version_string.len()], version),
            None => return None,
        },
    };
    Some((alternate_key, version))
}

/// Reduces the full semver `version` of the name `name` (with `@` at `at` and
/// the version string starting at `at + 1`) to the "canonical" form, such as
/// `a:b/c@1` for `a:b/c@1.2.3` or `a:b/c@0.1` for `a:b/c@0.1.2`.
///
/// Returns `None` for versions that aren't compatible with any other version,
/// such as prereleases or `0.0.x`.
fn reduce_to_canonical<'a>(
    name: &'a str,
    at: usize,
    version_string: &str,
    version: &Version,
) -> Option<&'a str> {
    if !version.pre.is_empty() {
        // If there's a prerelease then don't consider that compatible with any
        // other version number.
        return None;
    }
    if version.major != 0 {
        // If the major number is nonzero then compatibility is up to the major
        // version number, so return up to the first decimal.
        let first_dot = version_string.find('.')?;
        Some(&name[..at + 1 + first_dot])
    } else if version.minor != 0 {
        // Like the major version if the minor is nonzero then patch releases
        // are all considered to be on a "compatible track".
        let first_dot = version_string.find('.')?;
        let second_dot = version_string[first_dot + 1..].find('.')? + first_dot + 1;
        Some(&name[..at + 1 + second_dot])
    } else if version.patch == 0 {
        // 0.0.0 matches itself, and build metadata doesn't affect
        // compatibility, so reduce to just `0.0.0`.
        let len = version_string.find('+').unwrap_or(version_string.len());
        Some(&name[..at + 1 + len])
    } else {
        // If the patch number is the first nonzero entry then nothing can be
        // compatible with this patch, e.g. 0.0.1 isn't compatible with
        // any other version inherently.
        None
    }
}

/// Parses a canonical version such as `1` or `0.1`, filling in the missing
/// numbers, returning `None` if `version` isn't a canonical version.
fn canonical_version(version: &str) -> Option<Version> {
    // Reject anything with a prerelease or build metadata: those are never
    // canonical versions and would parse ambiguously below.
    if version.contains('-') || version.contains('+') {
        return None;
    }
    let mut numbers = version.split('.');
    let (major, minor, patch) = match (numbers.next(), numbers.next(), numbers.next()) {
        // `0` and `0.0` aren't canonical versions: only a nonzero major or
        // `0.minor` with a nonzero minor are.
        (Some("0"), None, None) | (Some("0"), Some("0"), None) => return None,
        (Some(major), None, None) if major != "0" => (major, "0", "0"),
        (Some("0"), Some(minor), None) if minor != "0" => ("0", minor, "0"),
        _ => return None,
    };
    let is_number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !is_number(major) || !is_number(minor) || !is_number(patch) {
        return None;
    }
    Version::parse(&format!("{major}.{minor}.{patch}")).ok()
}

#[cfg(test)]
mod tests {
    use super::{NameMap, NameMapNoIntern};

    #[test]
    fn alternate_lookup_key() {
        fn alt(s: &str) -> Option<&str> {
            super::alternate_lookup_key(s).map(|(s, _)| s)
        }

        assert_eq!(alt("x"), None);
        assert_eq!(alt("x:y/z"), None);
        assert_eq!(alt("x:y/z@1.0.0"), Some("x:y/z@1"));
        assert_eq!(alt("x:y/z@1.1.0"), Some("x:y/z@1"));
        assert_eq!(alt("x:y/z@1.1.2"), Some("x:y/z@1"));
        assert_eq!(alt("x:y/z@2.1.2"), Some("x:y/z@2"));
        assert_eq!(alt("x:y/z@2.1.2+abc"), Some("x:y/z@2"));
        assert_eq!(alt("x:y/z@0.1.2"), Some("x:y/z@0.1"));
        assert_eq!(alt("x:y/z@0.1.3"), Some("x:y/z@0.1"));
        assert_eq!(alt("x:y/z@0.2.3"), Some("x:y/z@0.2"));
        assert_eq!(alt("x:y/z@0.2.3+abc"), Some("x:y/z@0.2"));
        assert_eq!(alt("x:y/z@0.0.1"), None);
        assert_eq!(alt("x:y/z@0.0.1-pre"), None);
        assert_eq!(alt("x:y/z@0.1.0-pre"), None);
        assert_eq!(alt("x:y/z@1.0.0-pre"), None);

        // Canonical names are already in their reduced form and are returned
        // as-is so that they hash the same as full versions on the same
        // compatible track.
        assert_eq!(alt("x:y/z@1"), Some("x:y/z@1"));
        assert_eq!(alt("x:y/z@2"), Some("x:y/z@2"));
        assert_eq!(alt("x:y/z@0.1"), Some("x:y/z@0.1"));
        assert_eq!(alt("x:y/z@0.2"), Some("x:y/z@0.2"));
        assert_eq!(alt("x:y/z@0.0.0"), Some("x:y/z@0.0.0"));
        assert_eq!(alt("x:y/z@0"), None);
        assert_eq!(alt("x:y/z@0.0"), None);
        assert_eq!(alt("x:y/z@1.2"), None);
        assert_eq!(alt("x:y/z@1-pre"), None);
        assert_eq!(alt("x:y/z@0.1-pre"), None);
    }

    #[test]
    fn name_map_smoke() {
        let mut map = NameMap::default();
        let mut intern = NameMapNoIntern;

        map.insert("a", &mut intern, false, 0).unwrap();
        map.insert("b", &mut intern, false, 1).unwrap();

        assert!(map.insert("a", &mut intern, false, 0).is_err());
        assert!(map.insert("a", &mut intern, true, 0).is_ok());

        assert_eq!(map.get("a", &intern), Some(&0));
        assert_eq!(map.get("b", &intern), Some(&1));
        assert_eq!(map.get("c", &intern), None);

        map.insert("a:b/c@1.0.0", &mut intern, false, 2).unwrap();
        map.insert("a:b/c@1.0.1", &mut intern, false, 3).unwrap();
        assert_eq!(map.get("a:b/c@1.0.0", &intern), Some(&2));
        assert_eq!(map.get("a:b/c@1.0.1", &intern), Some(&3));
        assert_eq!(map.get("a:b/c@1.0.2", &intern), Some(&3));
        assert_eq!(map.get("a:b/c@1.1.0", &intern), Some(&3));
    }

    /// Definitions with a canonical name (e.g. `a:b/c@0.2`, without a full
    /// semver) must be found when looking up a full version name (e.g.
    /// `a:b/c@0.2.1`) and vice versa.
    #[test]
    fn name_map_canonical_names() {
        let mut map = NameMap::default();
        let mut intern = NameMapNoIntern;

        // A full version defined in the map is found via a canonical name
        // lookup, e.g. a component importing `a:b/c@0.2` links against a
        // host definition of `a:b/c@0.2.1`.
        map.insert("a:b/c@0.2.1", &mut intern, false, 0).unwrap();
        assert_eq!(map.get("a:b/c@0.2", &intern), Some(&0));
        assert_eq!(map.get("a:b/c@0.2.1", &intern), Some(&0));
        assert_eq!(map.get("a:b/c@0.2.2", &intern), Some(&0));
        assert_eq!(map.get("a:b/c@0.3", &intern), None);

        // A canonical name defined in the map is found via a full version
        // lookup, e.g. a host definition of `a:b/c@1` links against a
        // component importing `a:b/c@1.2.3`.
        map.insert("a:b/d@1", &mut intern, false, 1).unwrap();
        assert_eq!(map.get("a:b/d@1", &intern), Some(&1));
        assert_eq!(map.get("a:b/d@1.2.3", &intern), Some(&1));
        assert_eq!(map.get("a:b/d@1.9.0", &intern), Some(&1));
        assert_eq!(map.get("a:b/d@2", &intern), None);
    }
}
