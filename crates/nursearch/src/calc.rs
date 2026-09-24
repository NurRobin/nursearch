//! A tiny arithmetic evaluator for inline calculator results.
//!
//! Supports `+ - * / % ^`, parentheses, unary sign, decimal numbers with a
//! point or a (German) comma, the symbols `× · ÷`, and a trailing `=`. It is
//! deliberately conservative: anything that does not look like an arithmetic
//! expression returns `None` so it never competes with normal app search.
//!
//! # Percentage handling
//!
//! The `%` character has two distinct roles:
//!
//! * **Modulo** (`10 % 3` → `1`): triggered when `%` appears between two plain
//!   numbers with no `of`/`von` keyword and no trailing-percent context.
//! * **Percentage-of** (`15% of 240`, `15 % von 240` → `36`): triggered when
//!   the keyword `of` or `von` (case-insensitive) follows the percent operand.
//! * **Trailing percent** (`240 + 15%` → `276`, `240 - 15%` → `204`): triggered
//!   when the expression ends with `%` after a `+` or `-` operator.  The percent
//!   is applied to the *left-hand base value*, not the right-hand side; so
//!   `240 + 15%` means `240 + 240 × 0.15`, not `240 + 0.15`.
//! * `50%` **alone** → `None` (not a calculation by itself).

/// Evaluate an expression, returning a formatted result string when the input
/// is a complete, valid arithmetic expression.
pub fn evaluate(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let trimmed = trimmed.strip_suffix('=').unwrap_or(trimmed).trim_end();
    if trimmed.is_empty() {
        return None;
    }
    // Require at least one digit.
    if !trimmed.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }

    // ── Unit conversion (e.g. "10 km in mi") ─────────────────────────────
    if let Some(result) = crate::units::try_convert(trimmed) {
        return Some(result);
    }

    // ── Percentage patterns ───────────────────────────────────────────────
    if let Some(result) = try_percentage(trimmed) {
        return Some(result);
    }

    // ── Arithmetic ────────────────────────────────────────────────────────
    // A comma is a decimal separator; mixed with points it is ambiguous
    // ("1.000,5" vs "1,000.5"), so give up rather than guess.
    let decimal_comma = trimmed.contains(',');
    if decimal_comma && trimmed.contains('.') {
        return None;
    }
    let mut normalized = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        normalized.push(match c {
            '×' | '·' => '*',
            '÷' => '/',
            ',' => '.',
            c if c.is_ascii_digit() || "+-*/%^(). \t".contains(c) => c,
            _ => return None,
        });
    }

    let mut parser = Parser {
        bytes: normalized.as_bytes(),
        pos: 0,
        operations: 0,
    };
    let value = parser.expr()?;
    parser.skip_ws();
    // A lone number ("7" while searching for 7-Zip) is not a calculation.
    if parser.pos != parser.bytes.len() || parser.operations == 0 || !value.is_finite() {
        return None;
    }

    let formatted = format_number(value);
    Some(if decimal_comma {
        formatted.replace('.', ",")
    } else {
        formatted
    })
}

// ── Percentage helpers ────────────────────────────────────────────────────────

/// Try to evaluate percentage expressions before the arithmetic parser runs.
/// Returns `None` if the input does not look like a percentage expression.
///
/// Three forms handled:
/// 1. `N% of M` / `N % of M` / `N% von M` – "N percent of M"
/// 2. `base + N%` – add N% of base to base
/// 3. `base - N%` – subtract N% of base from base
fn try_percentage(input: &str) -> Option<String> {
    if !input.contains('%') {
        return None;
    }

    // Detect decimal comma usage (same logic as arithmetic path).
    let decimal_comma = input.contains(',');
    if decimal_comma && input.contains('.') {
        return None;
    }

    let s_lower = input.to_ascii_lowercase();

    // Form 1: "N% of M" / "N % von M"
    // Scan for " of " / " von " after any '%'.
    for connector in &[" of ", " von "] {
        if let Some(conn_pos) = s_lower.find(connector) {
            let left = input[..conn_pos].trim();
            let right = input[conn_pos + connector.len()..].trim();

            // Right side must be a plain number (no letters, no operators other
            // than an optional unary minus – but we keep it simple: digits + decimal only).
            if right.is_empty()
                || right
                    .chars()
                    .any(|c| !c.is_ascii_digit() && c != '.' && c != ',' && c != '-')
            {
                continue;
            }

            // Left side must end with '%', with a number before it.
            let pct_str = left.trim_end_matches('%').trim();
            if !left.ends_with('%') || pct_str.is_empty() {
                continue;
            }
            // pct_str must be a plain number.
            if pct_str
                .chars()
                .any(|c| !c.is_ascii_digit() && c != '.' && c != ',')
            {
                continue;
            }

            let pct = parse_plain_number(pct_str, decimal_comma)?;
            let base = parse_plain_number(right, decimal_comma)?;
            let result = pct / 100.0 * base;
            if !result.is_finite() {
                return None;
            }
            let s = format_number(result);
            return Some(if decimal_comma {
                s.replace('.', ",")
            } else {
                s
            });
        }
    }

    // Forms 2 & 3: "base +/- N%"
    // The whole expression must end with '%'.
    let without_eq = input.trim_end_matches('=').trim_end();
    if let Some(body_with_ws) = without_eq.strip_suffix('%') {
        let body = body_with_ws.trim_end();
        // Find the rightmost top-level '+' or '-' that is a binary operator.
        // We scan backwards; the operator must be preceded by a digit, ')' or space.
        let bytes = body.as_bytes();
        let mut split: Option<usize> = None;
        let mut depth: i32 = 0;
        for i in (0..bytes.len()).rev() {
            match bytes[i] {
                b')' => depth += 1,
                b'(' => depth -= 1,
                b'+' | b'-' if depth == 0 && i > 0 => {
                    let prev = bytes[i - 1];
                    if prev.is_ascii_digit()
                        || prev == b' '
                        || prev == b')'
                        || prev == b','
                        || prev == b'.'
                    {
                        split = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(op_pos) = split {
            let op = bytes[op_pos] as char;
            let base_expr = input[..op_pos].trim();
            let pct_str = body[op_pos + 1..].trim();

            if base_expr.is_empty() || pct_str.is_empty() {
                return None;
            }
            // pct_str must be a plain number (digits + decimal only).
            if pct_str
                .chars()
                .any(|c| !c.is_ascii_digit() && c != '.' && c != ',')
            {
                return None;
            }

            let pct = parse_plain_number(pct_str, decimal_comma)?;
            let base = evaluate_arithmetic(base_expr, decimal_comma)?;
            let result = if op == '+' {
                base + base * pct / 100.0
            } else {
                base - base * pct / 100.0
            };
            if !result.is_finite() {
                return None;
            }
            let s = format_number(result);
            return Some(if decimal_comma {
                s.replace('.', ",")
            } else {
                s
            });
        }
    }

    None
}

/// Parse a plain decimal number string (no operators, no letters).
fn parse_plain_number(s: &str, decimal_comma: bool) -> Option<f64> {
    let norm = if decimal_comma {
        s.replace(',', ".")
    } else {
        s.to_string()
    };
    norm.parse().ok()
}

/// Evaluate a sub-expression through the arithmetic parser, returning the numeric
/// result.  Used to evaluate the base expression in trailing-percent forms.
fn evaluate_arithmetic(expr: &str, decimal_comma: bool) -> Option<f64> {
    if decimal_comma && expr.contains('.') {
        return None;
    }
    let mut normalized = String::with_capacity(expr.len());
    for c in expr.chars() {
        normalized.push(match c {
            '×' | '·' => '*',
            '÷' => '/',
            ',' => '.',
            c if c.is_ascii_digit() || "+-*/%^(). \t".contains(c) => c,
            _ => return None,
        });
    }
    let mut parser = Parser {
        bytes: normalized.as_bytes(),
        pos: 0,
        operations: 0,
    };
    let value = parser.expr()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() || !value.is_finite() {
        return None;
    }
    Some(value)
}

// ── Arithmetic parser ─────────────────────────────────────────────────────────

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Binary operators and parentheses seen; zero means a bare number.
    operations: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.pos += 1;
            self.operations += 1;
            let rhs = self.term()?;
            value = if op == b'+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        while let Some(op @ (b'*' | b'/' | b'%')) = self.peek() {
            self.pos += 1;
            self.operations += 1;
            let rhs = self.unary()?;
            value = match op {
                b'*' => value * rhs,
                b'/' => value / rhs,
                _ => value % rhs,
            };
        }
        Some(value)
    }

    /// Unary sign binds looser than `^`, so `-2^2` is `-(2^2)`.
    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            b'+' => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    /// `^` is right-associative: `2^3^2` is `2^(3^2)`.
    fn power(&mut self) -> Option<f64> {
        let base = self.factor()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            self.operations += 1;
            let exponent = self.unary()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn factor(&mut self) -> Option<f64> {
        if self.peek()? == b'(' {
            self.pos += 1;
            self.operations += 1;
            let value = self.expr()?;
            if self.peek()? != b')' {
                return None;
            }
            self.pos += 1;
            return Some(value);
        }
        self.number()
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.pos;
        while matches!(self.bytes.get(self.pos), Some(b) if b.is_ascii_digit() || *b == b'.') {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse()
            .ok()
    }
}

// ── Number formatting ─────────────────────────────────────────────────────────

/// Format a floating-point result: integers without a decimal point, fractional
/// values with up to six significant decimal digits (trailing zeros trimmed).
/// Exported so the `units` module can reuse the same formatting rules.
pub(crate) fn format_number(value: f64) -> String {
    if value == 0.0 {
        // Avoid printing "-0".
        return "0".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let formatted = format!("{value:.6}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_basic_arithmetic() {
        assert_eq!(evaluate("2 + 3 * 4"), Some("14".to_string()));
        assert_eq!(evaluate("(2 + 3) * 4"), Some("20".to_string()));
        assert_eq!(evaluate("10 / 4"), Some("2.5".to_string()));
        assert_eq!(evaluate("-5 + 2"), Some("-3".to_string()));
        assert_eq!(evaluate("10 % 3"), Some("1".to_string()));
    }

    #[test]
    fn rejects_non_expressions() {
        assert_eq!(evaluate("code"), None);
        assert_eq!(evaluate("firefox"), None);
        assert_eq!(evaluate(""), None);
        assert_eq!(evaluate("2 +"), None);
        assert_eq!(evaluate("(2 + 3"), None);
    }

    #[test]
    fn accepts_german_decimal_comma_and_answers_in_kind() {
        assert_eq!(evaluate("2,5 * 4"), Some("10".to_string()));
        assert_eq!(evaluate("1,5 + 1,25"), Some("2,75".to_string()));
        assert_eq!(evaluate("1.5 + 1.25"), Some("2.75".to_string()));
    }

    #[test]
    fn supports_powers_symbols_and_trailing_equals() {
        assert_eq!(evaluate("2^10"), Some("1024".to_string()));
        assert_eq!(evaluate("2^3^2"), Some("512".to_string()));
        assert_eq!(evaluate("-2^2"), Some("-4".to_string()));
        assert_eq!(evaluate("6 × 7"), Some("42".to_string()));
        assert_eq!(evaluate("9 ÷ 3"), Some("3".to_string()));
        assert_eq!(evaluate("5 + 5 ="), Some("10".to_string()));
    }

    #[test]
    fn a_lone_number_is_not_a_calculation() {
        assert_eq!(evaluate("7"), None);
        assert_eq!(evaluate("-7"), None);
        assert_eq!(evaluate("(7)"), Some("7".to_string()));
    }

    #[test]
    fn rejects_division_by_zero() {
        assert_eq!(evaluate("1 / 0"), None);
    }

    // ── Percentage tests ──────────────────────────────────────────────────

    #[test]
    fn percentage_of() {
        assert_eq!(evaluate("15% of 240"), Some("36".to_string()));
        assert_eq!(evaluate("15 % von 240"), Some("36".to_string()));
        assert_eq!(evaluate("50% of 200"), Some("100".to_string()));
        assert_eq!(evaluate("10% of 50"), Some("5".to_string()));
    }

    #[test]
    fn percentage_trailing_add_subtract() {
        assert_eq!(evaluate("240 + 15%"), Some("276".to_string()));
        assert_eq!(evaluate("240 - 15%"), Some("204".to_string()));
        assert_eq!(evaluate("100 + 10%"), Some("110".to_string()));
        assert_eq!(evaluate("100 - 10%"), Some("90".to_string()));
    }

    #[test]
    fn lone_percent_is_not_a_calculation() {
        assert_eq!(evaluate("50%"), None);
    }

    #[test]
    fn modulo_still_works() {
        assert_eq!(evaluate("10 % 3"), Some("1".to_string()));
        assert_eq!(evaluate("7 % 2"), Some("1".to_string()));
    }

    #[test]
    fn percentage_decimal_comma() {
        // German: "1,5 % von 200" → 3, and result should use comma
        // 1.5 % of 200 = 3 (integer, no comma needed in output)
        assert_eq!(evaluate("1,5 % von 200"), Some("3".to_string()));
    }

    // ── False-positive guards ─────────────────────────────────────────────

    #[test]
    fn app_search_false_positives_return_none() {
        assert_eq!(evaluate("7zip"), None);
        assert_eq!(evaluate("vlc"), None);
        assert_eq!(evaluate("2048"), None);
        assert_eq!(evaluate("h2o"), None);
        assert_eq!(evaluate("mp3 player"), None);
        assert_eq!(evaluate("in"), None);
        assert_eq!(evaluate("k3b"), None);
    }
}
