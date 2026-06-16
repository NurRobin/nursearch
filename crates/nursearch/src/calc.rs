//! A tiny arithmetic evaluator for inline calculator results.
//!
//! Supports `+ - * / %`, parentheses, unary sign, and decimal numbers. It is
//! deliberately conservative: anything that does not look like an arithmetic
//! expression returns `None` so it never competes with normal app search.

/// Evaluate an expression, returning a formatted result string when the input
/// is a complete, valid arithmetic expression.
pub fn evaluate(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Require at least one digit and an operator or paren, otherwise plain words
    // like "code" would be treated as (failed) expressions on every keystroke.
    if !trimmed.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_digit() || b"+-*/%(). \t".contains(&b))
    {
        return None;
    }

    let mut parser = Parser {
        bytes: trimmed.as_bytes(),
        pos: 0,
    };
    let value = parser.expr()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() || !value.is_finite() {
        return None;
    }

    Some(format_number(value))
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
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
            let rhs = self.term()?;
            value = if op == b'+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.factor()?;
        while let Some(op @ (b'*' | b'/' | b'%')) = self.peek() {
            self.pos += 1;
            let rhs = self.factor()?;
            value = match op {
                b'*' => value * rhs,
                b'/' => value / rhs,
                _ => value % rhs,
            };
        }
        Some(value)
    }

    fn factor(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.pos += 1;
                Some(-self.factor()?)
            }
            b'+' => {
                self.pos += 1;
                self.factor()
            }
            b'(' => {
                self.pos += 1;
                let value = self.expr()?;
                if self.peek()? != b')' {
                    return None;
                }
                self.pos += 1;
                Some(value)
            }
            _ => self.number(),
        }
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
    fn rejects_division_by_zero() {
        assert_eq!(evaluate("1 / 0"), None);
    }
}
