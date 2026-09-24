//! A tiny arithmetic evaluator for inline calculator results.
//!
//! Supports `+ - * / % ^`, parentheses, unary sign, decimal numbers with a
//! point or a (German) comma, the symbols `× · ÷`, and a trailing `=`. It is
//! deliberately conservative: anything that does not look like an arithmetic
//! expression returns `None` so it never competes with normal app search.

/// Evaluate an expression, returning a formatted result string when the input
/// is a complete, valid arithmetic expression.
pub fn evaluate(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let trimmed = trimmed.strip_suffix('=').unwrap_or(trimmed).trim_end();
    if trimmed.is_empty() {
        return None;
    }
    // Require at least one digit, otherwise plain words like "code" would be
    // treated as (failed) expressions on every keystroke.
    if !trimmed.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
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

fn format_number(value: f64) -> String {
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
}
