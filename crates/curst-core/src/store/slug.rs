//! file names made from display names

/// the longest slug, suffix included
pub(crate) const MAX_LEN: usize = 64;

const FALLBACK: &str = "untitled";

/// names Windows refuses for a file, with or without an extension
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// lower-case ASCII letters, digits and single dashes, at most
/// [`MAX_LEN`] characters, never empty and never a name Windows reserves
///
/// ```text
/// "Get a Pet (v2)" -> "get-a-pet-v2"
/// "Café"           -> "caf"
/// "日本"           -> "untitled"
/// "CON"            -> "con-1"
/// ```
pub(crate) fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len().min(MAX_LEN));

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }

    slug.truncate(MAX_LEN);

    let slug = slug.trim_end_matches('-');

    if slug.is_empty() {
        FALLBACK.to_owned()
    } else if WINDOWS_RESERVED.contains(&slug) {
        format!("{slug}-1")
    } else {
        slug.to_owned()
    }
}

/// `base`, or `base-2`, `base-3` and so on, the first one `taken` refuses;
/// the base is shortened so the suffix still fits in [`MAX_LEN`]
pub(crate) fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }

    (2..u32::MAX)
        .map(|n| {
            let suffix = format!("-{n}");
            let room = MAX_LEN - suffix.len();
            let stem = base.get(..room).unwrap_or(base).trim_end_matches('-');

            format!("{stem}{suffix}")
        })
        .find(|candidate| !taken(candidate))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn names_become_slugs() {
        assert_eq!(slugify("Get a Pet (v2)"), "get-a-pet-v2");
        assert_eq!(slugify("  --Hello,,World--  "), "hello-world");
        assert_eq!(slugify("Café"), "caf");
        assert_eq!(slugify("日本"), "untitled");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("CON"), "con-1");
        assert_eq!(slugify("lpt9"), "lpt9-1");
        assert_eq!(slugify("console"), "console");
    }

    #[test]
    fn long_names_are_cut_without_a_trailing_dash() {
        let slug = slugify(&format!("{} b", "a".repeat(63)));

        assert_eq!(slug, "a".repeat(63));
        assert_eq!(slugify(&"x".repeat(100)).len(), MAX_LEN);
    }

    #[test]
    fn collisions_get_a_number_that_fits() {
        let taken = ["a", "a-2"];

        assert_eq!(unique("b", |s| taken.contains(&s)), "b");
        assert_eq!(unique("a", |s| taken.contains(&s)), "a-3");

        let long = "x".repeat(MAX_LEN);
        let next = unique(&long, |s| s == long);

        assert_eq!(next.len(), MAX_LEN);
        assert!(next.ends_with("-2"));

        let dashed = format!("{}-yy", "x".repeat(61));

        assert_eq!(
            unique(&dashed, |s| s == dashed),
            format!("{}-2", "x".repeat(61))
        );
    }

    proptest! {
        #[test]
        fn slugs_are_always_safe(name in ".{0,100}") {
            let slug = slugify(&name);

            prop_assert!(!slug.is_empty());
            prop_assert!(slug.len() <= MAX_LEN);
            prop_assert!(slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
            prop_assert!(!slug.starts_with('-') && !slug.ends_with('-'));
            prop_assert!(!slug.contains("--"));
            prop_assert!(!WINDOWS_RESERVED.contains(&slug.as_str()));
        }
    }
}
