//! Unit-conversion evaluator.
//!
//! Parses `<number> <unit> (in|to|nach|zu) <unit>` and returns a formatted
//! result string, or `None` when the input does not match this shape.
//!
//! The number may use either `.` or `,` as the decimal separator (but not
//! both in the same query). The result mirrors the separator the input used.
//! There may be no space between the number and its unit.
//!
//! # Supported categories and units
//!
//! | Category    | Units (canonical symbols in **bold**)                              |
//! |-------------|---------------------------------------------------------------------|
//! | Length      | **mm** cm **m** **km** **in** **ft** **yd** **mi**              |
//! | Mass        | **mg** **g** **kg** **t** **oz** **lb**                         |
//! | Temperature | **C** **F** **K** (with or without `°`)                         |
//! | Time        | **ms** **s** **min** **h** **d** week                           |
//! | Data        | **B** **KB** **MB** **GB** **TB** **KiB** **MiB** **GiB** **TiB** |
//! | Speed       | **m/s** **km/h** **mph** **kn**                                 |
//! | Volume      | **ml** **l** **gal** **fl oz** **cup**                          |
//!
//! Aliases (case-insensitive): meter/meters/metres/Meter, kilometer/kilometre,
//! mile/miles/Meilen, foot/feet, inch/inches, yard/yards, pound/pounds/Pfund,
//! gram/gramm, kilogram, tonne, ounce/ounces, second/Sekunde, minute/Minute,
//! hour/Stunde, day/Tag/days/Tage, knot/knots, liter/litre/Liter, gallon/gallons.
//!
//! Incompatible categories → `None`.

use crate::calc::format_number;

// ── Unit kinds ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Category {
    Length,
    Mass,
    Temperature,
    Time,
    Data,
    Speed,
    Volume,
}

/// A resolved unit: its category, a factor (to convert *from* this unit *to*
/// the category's base unit), and the canonical display symbol.
struct Unit {
    category: Category,
    /// Multiply value_in_this_unit by factor to get value_in_base_unit.
    /// For temperature this field is ignored; use `to_base` / `from_base`.
    factor: f64,
    symbol: &'static str,
}

// ── Conversion helpers for temperature ───────────────────────────────────────

fn temp_to_celsius(value: f64, sym: &str) -> f64 {
    match sym {
        "F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        _ => value, // C
    }
}

fn celsius_to(celsius: f64, sym: &str) -> f64 {
    match sym {
        "F" => celsius * 9.0 / 5.0 + 32.0,
        "K" => celsius + 273.15,
        _ => celsius, // C
    }
}

// ── Unit table ────────────────────────────────────────────────────────────────

/// Return `Some(Unit)` for a known unit name, `None` otherwise.
/// The lookup is case-insensitive (caller must lowercase before calling).
fn lookup(s: &str) -> Option<Unit> {
    // We match the lower-cased string but store the canonical symbol as-is.
    use Category::*;
    macro_rules! u {
        ($cat:expr, $factor:expr, $sym:expr) => {
            Some(Unit {
                category: $cat,
                factor: $factor,
                symbol: $sym,
            })
        };
    }
    match s {
        // ── Length (base: m) ──────────────────────────────────────────────
        "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => {
            u!(Length, 1e-3, "mm")
        }
        "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" => {
            u!(Length, 1e-2, "cm")
        }
        "m" | "meter" | "meters" | "metre" | "metres" => u!(Length, 1.0, "m"),
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" => u!(Length, 1e3, "km"),
        "in" | "inch" | "inches" | "zoll" => u!(Length, 0.0254, "in"),
        "ft" | "foot" | "feet" | "fuss" | "fuß" => u!(Length, 0.3048, "ft"),
        "yd" | "yard" | "yards" => u!(Length, 0.9144, "yd"),
        "mi" | "mile" | "miles" | "meile" | "meilen" => u!(Length, 1609.344, "mi"),

        // ── Mass (base: g) ────────────────────────────────────────────────
        "mg" | "milligram" | "milligrams" | "milligramm" => u!(Mass, 1e-3, "mg"),
        "g" | "gram" | "grams" | "gramm" => u!(Mass, 1.0, "g"),
        "kg" | "kilogram" | "kilograms" | "kilogramm" => u!(Mass, 1e3, "kg"),
        "t" | "tonne" | "tonnes" | "ton" | "tons" => u!(Mass, 1e6, "t"),
        "oz" | "ounce" | "ounces" | "unze" | "unzen" => u!(Mass, 28.349_523_125, "oz"),
        "lb" | "lbs" | "pound" | "pounds" | "pfund" => u!(Mass, 453.592_37, "lb"),

        // ── Temperature ───────────────────────────────────────────────────
        "c" | "°c" | "celsius" | "grad" => u!(Temperature, 0.0, "°C"),
        "f" | "°f" | "fahrenheit" => u!(Temperature, 0.0, "°F"),
        "k" | "kelvin" => u!(Temperature, 0.0, "K"),

        // ── Time (base: s) ────────────────────────────────────────────────
        "ms" | "millisecond" | "milliseconds" | "millisekunde" | "millisekunden" => {
            u!(Time, 1e-3, "ms")
        }
        "s" | "sec" | "secs" | "second" | "seconds" | "sekunde" | "sekunden" => u!(Time, 1.0, "s"),
        "min" | "minute" | "minutes" | "minuten" => u!(Time, 60.0, "min"),
        "h" | "hr" | "hour" | "hours" | "stunde" | "stunden" => u!(Time, 3600.0, "h"),
        "d" | "day" | "days" | "tag" | "tage" => u!(Time, 86_400.0, "d"),
        "week" | "weeks" | "woche" | "wochen" => u!(Time, 604_800.0, "week"),

        // ── Data (base: B) ────────────────────────────────────────────────
        "b" | "byte" | "bytes" => u!(Data, 1.0, "B"),
        "kb" | "kilobyte" | "kilobytes" => u!(Data, 1e3, "KB"),
        "mb" | "megabyte" | "megabytes" => u!(Data, 1e6, "MB"),
        "gb" | "gigabyte" | "gigabytes" => u!(Data, 1e9, "GB"),
        "tb" | "terabyte" | "terabytes" => u!(Data, 1e12, "TB"),
        "kib" | "kibibyte" | "kibibytes" => u!(Data, 1024.0, "KiB"),
        "mib" | "mebibyte" | "mebibytes" => u!(Data, 1_048_576.0, "MiB"),
        "gib" | "gibibyte" | "gibibytes" => u!(Data, 1_073_741_824.0, "GiB"),
        "tib" | "tebibyte" | "tebibytes" => u!(Data, 1_099_511_627_776.0, "TiB"),

        // ── Speed (base: m/s) ─────────────────────────────────────────────
        "m/s" => u!(Speed, 1.0, "m/s"),
        "km/h" | "kmh" | "kph" => u!(Speed, 1.0 / 3.6, "km/h"),
        "mph" => u!(Speed, 0.44704, "mph"),
        "kn" | "kt" | "knot" | "knots" | "knoten" => u!(Speed, 0.514_444_4, "kn"),

        // ── Volume (base: ml) ─────────────────────────────────────────────
        "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" => {
            u!(Volume, 1.0, "ml")
        }
        "l" | "liter" | "liters" | "litre" | "litres" => u!(Volume, 1000.0, "l"),
        "gal" | "gallon" | "gallons" => u!(Volume, 3_785.411_78, "gal"),
        "fl oz" | "floz" | "fl_oz" => u!(Volume, 29.573_529_6, "fl oz"),
        "cup" | "cups" => u!(Volume, 236.588_236, "cup"),

        _ => None,
    }
}

// ── Number parsing ────────────────────────────────────────────────────────────

/// Parse a decimal number that may use `.` or `,` as the decimal separator.
/// Returns `(value, used_comma)`.
fn parse_number(s: &str) -> Option<(f64, bool)> {
    let s = s.trim();
    let comma = s.contains(',');
    let dot = s.contains('.');
    if comma && dot {
        return None; // ambiguous
    }
    let normalized = if comma {
        s.replace(',', ".")
    } else {
        s.to_string()
    };
    let v: f64 = normalized.parse().ok()?;
    Some((v, comma))
}

// ── Query parsing ─────────────────────────────────────────────────────────────

/// Try to interpret `input` as a unit-conversion query.
/// Returns the formatted result string (e.g. `"6.213712 mi"`) or `None`.
pub fn try_convert(input: &str) -> Option<String> {
    let s = input.trim();
    let s = s.strip_suffix('=').unwrap_or(s).trim_end();

    // Must contain a digit.
    if !s.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }

    // The connector splits the query.  We look for the connector as a
    // standalone word (surrounded by spaces or at position / end-of-string).
    // Supported: "in", "to", "nach", "zu" — case-insensitive.
    let s_lower = s.to_ascii_lowercase();
    let connector_pos = find_connector(&s_lower)?;

    let left = s[..connector_pos.0].trim();
    let right = s[connector_pos.1..].trim();

    // right must be a single unit name (no digits).
    if right.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let to_unit = lookup(&right.to_ascii_lowercase())?;

    // left must be NUMBER [optional-space] UNIT
    let (value, from_unit, decimal_comma) = parse_value_unit(left)?;

    // Categories must match.
    if from_unit.category != to_unit.category {
        return None;
    }

    let result = convert_value(value, &from_unit, &to_unit);
    if !result.is_finite() {
        return None;
    }

    let num_str = format_number(result);
    let num_str = if decimal_comma {
        num_str.replace('.', ",")
    } else {
        num_str
    };

    // Speed units like "km/h" contain "/" which may confuse some renderers, but
    // we just return the canonical symbol as-is.
    Some(format!("{} {}", num_str, to_unit.symbol))
}

/// Find the connector word ("in"/"to"/"nach"/"zu") as a standalone token.
/// Returns (start, end) byte positions in the lowercased string, or None.
fn find_connector(s_lower: &str) -> Option<(usize, usize)> {
    for conn in &["nach", "to", "zu", "in"] {
        let clen = conn.len();
        let mut search_from = 0;
        while search_from + clen <= s_lower.len() {
            if let Some(pos) = s_lower[search_from..].find(conn) {
                let abs = search_from + pos;
                let end = abs + clen;
                // Must be bounded by space/start/end.
                let left_ok = abs == 0 || s_lower.as_bytes()[abs - 1] == b' ';
                let right_ok = end == s_lower.len() || s_lower.as_bytes()[end] == b' ';
                if left_ok && right_ok {
                    return Some((abs, end));
                }
                search_from = abs + 1;
            } else {
                break;
            }
        }
    }
    None
}

/// Parse `"1.5 km"` or `"10km"` into `(value, Unit, decimal_comma)`.
fn parse_value_unit(s: &str) -> Option<(f64, Unit, bool)> {
    let s = s.trim();
    // Find where the numeric part ends.
    // A number is: optional sign is NOT expected here (it would be part of an
    // arithmetic expression).  Digits, one decimal point or comma, digits.
    let num_end = s
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.' || *c == ',')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;

    let num_str = &s[..num_end];
    let unit_str = s[num_end..].trim().to_ascii_lowercase();

    if unit_str.is_empty() {
        return None; // no unit
    }

    let (value, decimal_comma) = parse_number(num_str)?;
    let unit = lookup(&unit_str)?;
    Some((value, unit, decimal_comma))
}

fn convert_value(value: f64, from: &Unit, to: &Unit) -> f64 {
    if from.category == Category::Temperature {
        let celsius = temp_to_celsius(value, temp_sym(from.symbol));
        celsius_to(celsius, temp_sym(to.symbol))
    } else {
        value * from.factor / to.factor
    }
}

/// Strip the `°` from a temperature symbol for matching.
fn temp_sym(sym: &str) -> &str {
    sym.strip_prefix('°').unwrap_or(sym)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(s: &str) -> Option<String> {
        try_convert(s)
    }

    // ── Basic conversions ─────────────────────────────────────────────────

    #[test]
    fn length_km_to_mi() {
        assert_eq!(cv("10 km in mi"), Some("6.213712 mi".into()));
    }

    #[test]
    fn length_ft_to_cm() {
        let r = cv("5 ft to cm").unwrap();
        assert!(r.starts_with("152.4 cm"), "got: {r}");
    }

    #[test]
    fn mass_kg_to_lb() {
        let r = cv("1,5 kg in lb").unwrap();
        // 1.5 kg = 3.30693... lb, decimal comma in → comma in output
        assert!(r.contains(','), "should use comma: {r}");
        assert!(r.starts_with("3,306"), "got: {r}");
    }

    #[test]
    fn temperature_celsius_to_fahrenheit() {
        assert_eq!(cv("20 °C in F"), Some("68 °F".into()));
        assert_eq!(cv("0 c to f"), Some("32 °F".into()));
        assert_eq!(cv("20c to f"), Some("68 °F".into()));
    }

    #[test]
    fn temperature_fahrenheit_to_celsius() {
        // 98.6 F → 37 C
        let r = cv("98.6 f in c").unwrap();
        assert!(r.starts_with("37 "), "got: {r}");
    }

    #[test]
    fn time_hours_to_minutes() {
        assert_eq!(cv("3 h in min"), Some("180 min".into()));
    }

    #[test]
    fn data_gb_to_mb() {
        assert_eq!(cv("1 GB in MB"), Some("1000 MB".into()));
    }

    #[test]
    fn data_gib_to_mb() {
        // 1 GiB = 1 073 741 824 B = 1073.741824 MB
        let r = cv("1 GiB in MB").unwrap();
        assert!(r.starts_with("1073.741824 MB"), "got: {r}");
    }

    #[test]
    fn speed_kmh_to_mph() {
        // 100 km/h = 62.137119... mph
        let r = cv("100 km/h in mph").unwrap();
        assert!(r.starts_with("62.13711"), "got: {r}");
    }

    #[test]
    fn volume_l_to_ml() {
        assert_eq!(cv("1 l in ml"), Some("1000 ml".into()));
    }

    #[test]
    fn german_connectors() {
        assert_eq!(cv("10 km nach mi"), Some("6.213712 mi".into()));
        assert_eq!(cv("10 km zu mi"), Some("6.213712 mi".into()));
    }

    #[test]
    fn no_space_between_number_and_unit() {
        let r = cv("20c to f").unwrap();
        assert!(r.starts_with("68 °F"), "got: {r}");
    }

    // ── False-positive guards ─────────────────────────────────────────────

    #[test]
    fn rejects_app_search_queries() {
        assert_eq!(cv("7zip"), None);
        assert_eq!(cv("vlc"), None);
        assert_eq!(cv("2048"), None);
        assert_eq!(cv("h2o"), None);
        assert_eq!(cv("mp3 player"), None);
        assert_eq!(cv("in"), None);
        assert_eq!(cv("k3b"), None);
    }

    #[test]
    fn rejects_incompatible_categories() {
        assert_eq!(cv("10 km in kg"), None);
        assert_eq!(cv("100 MB in km"), None);
    }

    #[test]
    fn rejects_unknown_units() {
        assert_eq!(cv("10 xyz in km"), None);
        assert_eq!(cv("10 km in xyz"), None);
    }
}
