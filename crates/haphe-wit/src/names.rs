use std::collections::HashMap;

use crate::WitGenError;

/// WIT keywords that must be `%`-escaped when used as identifiers.
const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "bool",
    "borrow",
    "char",
    "constructor",
    "enum",
    "export",
    "f32",
    "f64",
    "flags",
    "func",
    "future",
    "import",
    "include",
    "interface",
    "list",
    "option",
    "own",
    "package",
    "record",
    "resource",
    "result",
    "s16",
    "s32",
    "s64",
    "s8",
    "static",
    "stream",
    "string",
    "tuple",
    "type",
    "u16",
    "u32",
    "u64",
    "u8",
    "use",
    "variant",
    "with",
    "world",
];

/// Converts a Rust identifier (`snake_case`, CamelCase, or `SCREAMING_CASE`) to
/// WIT kebab-case, `%`-escaping WIT keywords.
pub fn to_kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
            continue;
        }
        if c.is_uppercase() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1);
            // Word boundary: previous is lowercase/digit, or this starts a new
            // word after an acronym run (next is lowercase).
            let boundary = match prev {
                Some('_') => false,
                Some(p) if p.is_lowercase() || p.is_ascii_digit() => true,
                Some(p) if p.is_uppercase() => next.is_some_and(|n| n.is_lowercase()),
                _ => false,
            };
            if boundary && !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    escape(out)
}

fn escape(kebab: String) -> String {
    if KEYWORDS.binary_search(&kebab.as_str()).is_ok() {
        format!("%{kebab}")
    } else {
        kebab
    }
}

/// Tracks kebab-case names within one WIT scope, detecting collisions between
/// distinct source names that map to the same kebab identifier.
#[derive(Default)]
pub(crate) struct NameMap {
    seen: HashMap<String, String>,
}

impl NameMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `original` under its kebab form, returning the kebab name or
    /// a collision error if a different original already claimed it.
    pub fn insert(&mut self, original: &str) -> Result<String, WitGenError> {
        self.insert_as(original, original)
    }

    /// Like [`insert`](Self::insert), but with a distinct `identity` for the
    /// collision check — used by trait projections so a user member with the
    /// same spelling still collides descriptively.
    pub(crate) fn insert_as(
        &mut self,
        original: &str,
        identity: &str,
    ) -> Result<String, WitGenError> {
        let kebab = to_kebab(original);
        match self.seen.get(&kebab) {
            Some(first) if first != identity => Err(WitGenError::NameCollision {
                kebab,
                first: first.clone(),
                second: identity.to_string(),
            }),
            _ => {
                self.seen.insert(kebab.clone(), identity.to_string());
                Ok(kebab)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_sorted() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
    }

    #[test]
    fn snake_case() {
        assert_eq!(to_kebab("distance_to"), "distance-to");
        assert_eq!(to_kebab("x"), "x");
    }

    #[test]
    fn camel_case() {
        assert_eq!(to_kebab("Point"), "point");
        assert_eq!(to_kebab("MyType"), "my-type");
    }

    #[test]
    fn acronym_runs() {
        assert_eq!(to_kebab("HTTPServer"), "http-server");
        assert_eq!(to_kebab("ParseURL"), "parse-url");
    }

    #[test]
    fn digits() {
        assert_eq!(to_kebab("Vec3"), "vec3");
        assert_eq!(to_kebab("Vec3D"), "vec3-d");
    }

    #[test]
    fn screaming_case() {
        assert_eq!(to_kebab("MAX_VERTICES"), "max-vertices");
        assert_eq!(to_kebab("PI"), "pi");
    }

    #[test]
    fn keyword_escaped() {
        assert_eq!(to_kebab("record"), "%record");
        assert_eq!(to_kebab("Type"), "%type");
    }

    #[test]
    fn name_map_collision() {
        let mut map = NameMap::new();
        map.insert("MyType").unwrap();
        let err = map.insert("my_type").unwrap_err();
        assert!(matches!(err, WitGenError::NameCollision { .. }));
    }

    #[test]
    fn name_map_same_original_ok() {
        let mut map = NameMap::new();
        map.insert("Point").unwrap();
        map.insert("Point").unwrap();
    }
}
