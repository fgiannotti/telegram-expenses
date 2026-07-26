use crate::config::{Category, UnknownCategoryError};
use std::str::FromStr;

/// The outcome of reading one free-text message: `<monto> <categoria> [descripcion]`.
///
/// Every failure mode is its own variant rather than an `Option`, so the
/// handler is forced to produce a specific message for each one and adding a
/// new failure mode breaks the build at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseResult {
    Expense {
        amount: i64,
        category: Category,
        description: String,
    },
    UnknownCategory(String),
    MissingCategory,
    NoAmount,
    Empty,
}

pub fn parse_message(input: &str) -> ParseResult {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.is_empty() {
        return ParseResult::Empty;
    }

    let Some((amount, next)) = take_amount(&tokens) else {
        return ParseResult::NoAmount;
    };

    let Some(category_token) = tokens.get(next) else {
        return ParseResult::MissingCategory;
    };
    let category = match Category::from_str(category_token) {
        Ok(category) => category,
        Err(UnknownCategoryError) => {
            return ParseResult::UnknownCategory((*category_token).to_string())
        }
    };

    ParseResult::Expense {
        amount,
        category,
        description: tokens[next + 1..].join(" "),
    }
}

/// Reads the leading amount and returns it with the index of the first token
/// after it. Spaces count as thousand separators, so the amount can span
/// several tokens (`1 500 super`); a `k` suffix ends it immediately.
fn take_amount(tokens: &[&str]) -> Option<(i64, usize)> {
    let mut chunks = String::new();
    let mut index = 0;
    let mut scaled_by_k = false;

    while let Some(token) = tokens.get(index) {
        if let Some(head) = strip_k_suffix(token) {
            chunks.push_str(head);
            scaled_by_k = true;
            index += 1;
            break;
        }
        if !is_digit_group(token) {
            break;
        }
        chunks.push_str(token);
        index += 1;
    }

    if chunks.is_empty() {
        return None;
    }

    let amount = if scaled_by_k {
        thousands_from_k(&chunks)?
    } else {
        // No `k`, so `.` and `,` are purely cosmetic grouping: 1.500 is 1500.
        let digits: String = chunks.chars().filter(char::is_ascii_digit).collect();
        digits.parse::<i64>().ok()?
    };

    (amount > 0).then_some((amount, index))
}

fn is_digit_group(token: &str) -> bool {
    token.chars().any(|c| c.is_ascii_digit())
        && token
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

fn strip_k_suffix(token: &str) -> Option<&str> {
    let head = token.strip_suffix(['k', 'K'])?;
    is_digit_group(head).then_some(head)
}

/// In the `k` form the separator is a decimal point, not a grouping mark:
/// `1,5k` and `1.5k` are both 1500.
fn thousands_from_k(text: &str) -> Option<i64> {
    let normalized = text.replace(',', ".");
    let mut parts = normalized.split('.');
    let whole: i64 = parts.next()?.parse().ok()?;
    let fraction = match parts.next() {
        None => 0,
        Some(digits) => {
            if parts.next().is_some() {
                return None;
            }
            // Interpret as a fraction of 1000, truncating below the peso.
            let mut milli: String = digits.chars().take(3).collect();
            while milli.len() < 3 {
                milli.push('0');
            }
            milli.parse::<i64>().ok()?
        }
    };
    whole.checked_mul(1_000)?.checked_add(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expense(input: &str) -> (i64, Category, String) {
        match parse_message(input) {
            ParseResult::Expense {
                amount,
                category,
                description,
            } => (amount, category, description),
            other => panic!("expected an expense from {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn plain_amount_with_description() {
        assert_eq!(
            expense("4500 cafe con Juan"),
            (4500, Category::Cafe, "con Juan".to_string())
        );
    }

    #[test]
    fn no_description_yields_an_empty_string() {
        assert_eq!(expense("12000 super"), (12_000, Category::Super, String::new()));
    }

    #[test]
    fn dot_is_a_thousand_separator() {
        assert_eq!(
            expense("1.500 transporte subte"),
            (1500, Category::Transporte, "subte".to_string())
        );
    }

    #[test]
    fn comma_is_a_thousand_separator() {
        assert_eq!(expense("1,500 salud").0, 1500);
        assert_eq!(expense("1.234.567 otros").0, 1_234_567);
        assert_eq!(expense("2,500,000 otros").0, 2_500_000);
    }

    #[test]
    fn space_is_a_thousand_separator() {
        assert_eq!(expense("1 500 comida").0, 1500);
        assert_eq!(expense("12 000 comida milanesa").0, 12_000);
    }

    #[test]
    fn k_suffix_multiplies_by_a_thousand() {
        assert_eq!(expense("12k comida").0, 12_000);
        assert_eq!(expense("1,5k cafe").0, 1500);
        assert_eq!(expense("1.5k cafe").0, 1500);
        assert_eq!(expense("1,25k cafe").0, 1250);
        assert_eq!(expense("1,125k cafe").0, 1125);
        assert_eq!(expense("2K super").0, 2000);
    }

    #[test]
    fn k_fraction_truncates_below_the_peso() {
        assert_eq!(expense("1,1259k cafe").0, 1125);
    }

    #[test]
    fn k_suffix_ends_the_amount_even_if_more_digits_follow() {
        // `2` is the description here, not more of the amount.
        assert_eq!(
            expense("3k comida 2 empanadas"),
            (3000, Category::Comida, "2 empanadas".to_string())
        );
    }

    #[test]
    fn accented_and_plural_categories_are_accepted() {
        assert_eq!(expense("800 café").1, Category::Cafe);
        assert_eq!(expense("800 cafés").1, Category::Cafe);
        assert_eq!(expense("800 Cafes medialunas").1, Category::Cafe);
        assert_eq!(expense("9000 comidas").1, Category::Comida);
    }

    #[test]
    fn unknown_category_is_reported_verbatim() {
        assert_eq!(
            parse_message("4500 nafta"),
            ParseResult::UnknownCategory("nafta".to_string())
        );
        assert_eq!(
            parse_message("4500 Nafta ypf"),
            ParseResult::UnknownCategory("Nafta".to_string())
        );
    }

    #[test]
    fn amount_without_a_category() {
        assert_eq!(parse_message("4500"), ParseResult::MissingCategory);
        assert_eq!(parse_message("  1.500   "), ParseResult::MissingCategory);
        assert_eq!(parse_message("3k"), ParseResult::MissingCategory);
    }

    #[test]
    fn missing_or_unusable_amount() {
        assert_eq!(parse_message("cafe con Juan"), ParseResult::NoAmount);
        assert_eq!(parse_message("k comida"), ParseResult::NoAmount);
        assert_eq!(parse_message("0 cafe"), ParseResult::NoAmount);
        assert_eq!(parse_message("-500 cafe"), ParseResult::NoAmount);
        // Overflows i64 rather than wrapping into a bogus expense.
        assert_eq!(
            parse_message("99999999999999999999 cafe"),
            ParseResult::NoAmount
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse_message(""), ParseResult::Empty);
        assert_eq!(parse_message("   \n  "), ParseResult::Empty);
    }

    #[test]
    fn extra_whitespace_is_collapsed_in_the_description() {
        assert_eq!(
            expense("  4500   cafe    con   Juan  "),
            (4500, Category::Cafe, "con Juan".to_string())
        );
    }
}
