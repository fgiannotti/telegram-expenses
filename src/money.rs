/// es-AR currency rendering: dot as the thousands separator, no decimals,
/// because every amount the bot stores is a whole number of pesos.
pub fn format_amount(amount: i64) -> String {
    let mut out = String::new();
    if amount < 0 {
        out.push('-');
    }
    out.push('$');
    out.push_str(&group_thousands(amount.unsigned_abs()));
    out
}

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push('.');
        }
        out.push(char::from(*byte));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_thousands() {
        assert_eq!(format_amount(0), "$0");
        assert_eq!(format_amount(7), "$7");
        assert_eq!(format_amount(999), "$999");
        assert_eq!(format_amount(1_000), "$1.000");
        assert_eq!(format_amount(4_500), "$4.500");
        assert_eq!(format_amount(104_500), "$104.500");
        assert_eq!(format_amount(1_234_567), "$1.234.567");
    }

    #[test]
    fn negatives_keep_the_sign_outside_the_symbol() {
        assert_eq!(format_amount(-4_500), "-$4.500");
    }

    #[test]
    fn handles_the_extremes_without_overflow() {
        assert_eq!(format_amount(i64::MIN), "-$9.223.372.036.854.775.808");
        assert_eq!(format_amount(i64::MAX), "$9.223.372.036.854.775.807");
    }
}
