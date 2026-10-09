//! Czech formatting of times, numbers and counts.

/// Minutes since midnight -> "HH:MM" (24-hour), rounded to the minute.
pub fn hm(m: f64) -> String {
    let r = m.round() as i64;
    format!("{:02}:{:02}", r.div_euclid(60).rem_euclid(24), r.rem_euclid(60))
}

/// A number with a decimal comma: dec(1.55, 1) -> "1,6".
pub fn dec(v: f64, places: usize) -> String {
    format!("{v:.places$}").replace('.', ",")
}

/// A number with at most one decimal, without a trailing ",0": num(5.0) -> "5", num(4.5) -> "4,5".
pub fn num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        dec(v, 1)
    }
}

/// A duration in minutes -> "45 min" or "3 h 05 min".
pub fn duration(min: f64) -> String {
    let m = min.round().max(0.0) as i64;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

/// Czech plural: plural(3, "hlídka", "hlídky", "hlídek") -> "3 hlídky".
pub fn plural(n: usize, one: &str, few: &str, many: &str) -> String {
    let w = match n {
        1 => one,
        2..=4 => few,
        _ => many,
    };
    format!("{n} {w}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(hm(13.0 * 60.0 + 24.4), "13:24");
        assert_eq!(hm(9.0 * 60.0 + 44.6), "09:45");
        assert_eq!(dec(1.55, 1), "1,6");
        assert_eq!(num(5.0), "5");
        assert_eq!(num(4.5), "4,5");
        assert_eq!(duration(219.0), "3 h 39 min");
        assert_eq!(duration(65.0), "1 h 05 min");
        assert_eq!(duration(9.4), "9 min");
        assert_eq!(plural(1, "hlídka", "hlídky", "hlídek"), "1 hlídka");
        assert_eq!(plural(3, "hlídka", "hlídky", "hlídek"), "3 hlídky");
        assert_eq!(plural(21, "hlídka", "hlídky", "hlídek"), "21 hlídek");
    }
}
