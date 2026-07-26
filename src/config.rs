use chrono_tz::Tz;
use std::fmt;
use std::str::FromStr;

/// All dates the bot stores or reports are computed in this zone, never UTC and
/// never the host's local time. `chrono-tz` embeds the tzdb so this is correct
/// on a static binary running on a box with no `/usr/share/zoneinfo`.
pub const TIMEZONE: Tz = chrono_tz::America::Argentina::Buenos_Aires;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Cafe,
    Comida,
    Transporte,
    Salud,
    Super,
    Otros,
}

pub const ALL_CATEGORIES: [Category; 6] = [
    Category::Cafe,
    Category::Comida,
    Category::Transporte,
    Category::Salud,
    Category::Super,
    Category::Otros,
];

impl Category {
    /// The canonical spelling, and the exact text stored in the `category`
    /// column. Changing one of these strings orphans existing rows.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Cafe => "cafe",
            Category::Comida => "comida",
            Category::Transporte => "transporte",
            Category::Salud => "salud",
            Category::Super => "super",
            Category::Otros => "otros",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownCategoryError;

impl fmt::Display for UnknownCategoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("categoria desconocida")
    }
}

impl std::error::Error for UnknownCategoryError {}

impl FromStr for Category {
    type Err = UnknownCategoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = normalize(s);
        if let Some(category) = exact(&normalized) {
            return Ok(category);
        }
        // Plurals: `cafes` -> `cafe`, `comidas` -> `comida`. Done after the
        // exact lookup so `otros`, which is canonically plural, is not mangled.
        if let Some(singular) = normalized.strip_suffix('s') {
            if let Some(category) = exact(singular) {
                return Ok(category);
            }
        }
        Err(UnknownCategoryError)
    }
}

/// Lowercase and fold the Spanish accented vowels plus `n~` onto ASCII, so
/// `Café` and `cafe` are the same word.
fn normalize(input: &str) -> String {
    input
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

fn exact(normalized: &str) -> Option<Category> {
    match normalized {
        "cafe" => Some(Category::Cafe),
        "comida" => Some(Category::Comida),
        "transporte" => Some(Category::Transporte),
        "salud" => Some(Category::Salud),
        "super" => Some(Category::Super),
        "otros" | "otro" => Some(Category::Otros),
        _ => None,
    }
}

/// Monday-to-Sunday spending cap, in pesos. A `match` rather than a map so a
/// new `Category` variant fails to compile until someone decides about it.
pub fn weekly_limit(category: Category) -> Option<i64> {
    match category {
        Category::Comida => Some(100_000),
        Category::Cafe
        | Category::Transporte
        | Category::Salud
        | Category::Super
        | Category::Otros => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_parse() {
        for category in ALL_CATEGORIES {
            assert_eq!(Category::from_str(category.as_str()), Ok(category));
        }
    }

    #[test]
    fn accents_are_folded() {
        assert_eq!(Category::from_str("café"), Ok(Category::Cafe));
        assert_eq!(Category::from_str("CAFÉ"), Ok(Category::Cafe));
        assert_eq!(Category::from_str("Café"), Ok(Category::Cafe));
    }

    #[test]
    fn plurals_are_folded() {
        assert_eq!(Category::from_str("cafes"), Ok(Category::Cafe));
        assert_eq!(Category::from_str("cafés"), Ok(Category::Cafe));
        assert_eq!(Category::from_str("comidas"), Ok(Category::Comida));
        assert_eq!(Category::from_str("transportes"), Ok(Category::Transporte));
        assert_eq!(Category::from_str("supers"), Ok(Category::Super));
    }

    #[test]
    fn otros_survives_the_plural_rule() {
        assert_eq!(Category::from_str("otros"), Ok(Category::Otros));
        assert_eq!(Category::from_str("otro"), Ok(Category::Otros));
    }

    #[test]
    fn surrounding_whitespace_and_case_are_ignored() {
        assert_eq!(Category::from_str("  SUPER  "), Ok(Category::Super));
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(Category::from_str("nafta"), Err(UnknownCategoryError));
        assert_eq!(Category::from_str(""), Err(UnknownCategoryError));
        // Not a real plural of anything we know.
        assert_eq!(Category::from_str("s"), Err(UnknownCategoryError));
    }

    #[test]
    fn only_comida_has_a_limit() {
        assert_eq!(weekly_limit(Category::Comida), Some(100_000));
        for category in ALL_CATEGORIES {
            if category != Category::Comida {
                assert_eq!(weekly_limit(category), None);
            }
        }
    }
}
