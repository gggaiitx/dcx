//! Style-variable expression evaluation.
//!
//! Faithful port of the reference mod's recursive-descent evaluator:
//!
//! * `IsValidStyleVariableIdentifier` (reference line 16263)
//! * `StyleVariableExpressionEvaluator` (reference line 16319)
//! * `ExpandStyleVariables` (reference line 16804)
//!
//! The grammar is kept as a structured chain of precedence levels (`ParseTernary`
//! -> `ParseEquality` -> `ParseRelational` -> `ParseAdditive` -> `ParseTerm` ->
//! `ParseFactor` -> `ParsePrimary`) so that additional operators can be slotted
//! in at the right level without disturbing the rest.
//!
//! Error model: the C++ evaluator reports failures with `std::runtime_error` and
//! an applied `skip()` with a `StyleVariableSkipRequested` exception. Here both
//! are `Err(EvalError)`; `expand_style_variables` maps them to `None`, which is
//! what the reference does at the `ResolveDynamicStyleValue` boundary (the
//! consuming style is then left unapplied). Nothing is logged from this crate.

/// Lookup for captured style variables (`ActualWidth=>containerGridWidth`, ...).
///
/// The reference keeps a separate numeric flag next to each captured value
/// (`StyleVariableValue::numeric`); `VarLookup` only exposes its text form, so a
/// captured value whose text parses as a number is treated as numeric. See
/// `Evaluator::lookup_variable` for the rationale.
pub trait VarLookup {
    fn get(&self, name: &str) -> Option<String>;
}

/// `IsValidStyleVariableIdentifier` (reference 16263): `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_valid_style_variable_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_ident_start(c) => chars.all(is_ident_cont),
        _ => false,
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_cont(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Value produced while evaluating a `{{ ... }}` expression: either a number or
/// a string, mirroring `StyleExpressionValue` (reference 16285).
#[derive(Debug, Clone, PartialEq)]
enum ExprValue {
    Number(f64),
    Text(String),
}

impl ExprValue {
    fn number(d: f64) -> Self {
        Self::Number(d)
    }

    fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }
}

/// `StyleVariableSkipRequested` or a `std::runtime_error` from the reference.
#[derive(Debug, Clone, PartialEq)]
enum EvalError {
    /// `skip()` was evaluated in a live branch: leave the style unapplied.
    Skip,
    /// Parse / evaluation failure.
    Failed(String),
}

impl EvalError {
    fn failed(msg: &str) -> Self {
        Self::Failed(msg.to_string())
    }
}

/// Format a numeric result. Mirrors `FormatDoubleInvariant` (reference 15795):
/// invariant decimal point, `NaN` / `Infinity` / `-Infinity` spellings, and the
/// shortest round-trip representation for finite values.
fn format_number(d: f64) -> String {
    if d.is_nan() {
        return "NaN".to_string();
    }
    if d.is_infinite() {
        return if d < 0.0 {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }
    // `Display` for f64 already yields the shortest round-trip form without a
    // trailing `.0` (e.g. 1000.0 -> "1000", 0.5 -> "0.5").
    format!("{}", d)
}

/// `ParseDoubleInvariant` (reference 15813): locale-independent full-string parse.
fn parse_double_invariant(s: &str) -> Option<f64> {
    if s.is_empty() || !s.is_ascii() {
        return None;
    }
    s.parse::<f64>().ok()
}

/// `RequireNumber` (reference 16475): unwrap a numeric operand. In a dead
/// ternary branch a string operand is tolerated (reported as 0) instead of
/// failing the whole expression.
fn require_number(v: &ExprValue, live: bool) -> Result<f64, EvalError> {
    match v {
        ExprValue::Number(d) => Ok(*d),
        ExprValue::Text(_) => {
            if live {
                Err(EvalError::failed(
                    "Non-numeric value used where a number is required in style \
                     variable expression",
                ))
            } else {
                Ok(0.0)
            }
        }
    }
}

/// `ExpandStyleVariables` (reference 16804).
///
/// Repeatedly substitutes the innermost `{{ ... }}`: the first `}}` is paired
/// with the *rightmost* `{{` preceding it, so `{{{x}}}` becomes `{` + value + `}`.
/// Substituted text is literal (it is not scanned for further `{{...}}`).
///
/// Returns `None` on a malformed expression, an unmatched `}}`, or a live
/// `skip()` -- in all of those cases the reference leaves the style unapplied.
pub fn expand_style_variables(input: &str, vars: &dyn VarLookup) -> Option<String> {
    let mut result = input.to_string();
    let mut scan_from = 0usize;

    loop {
        let close_pos = match result[scan_from..].find("}}") {
            Some(i) => scan_from + i,
            None => break,
        };

        let open_pos = match result[..close_pos].rfind("{{") {
            Some(i) => i,
            None => return None,
        };

        let expr_text = &result[open_pos + 2..close_pos];
        let expanded = evaluate_expression_text(expr_text, vars)?;

        let span_len = close_pos + 2 - open_pos;
        result.replace_range(open_pos..open_pos + span_len, &expanded);
        scan_from = open_pos + expanded.len();
    }

    Some(result)
}

/// `EvaluateStyleVariableExpression` (reference 16770): evaluate the text
/// between `{{` and `}}`.
fn evaluate_expression_text(expr_text: &str, vars: &dyn VarLookup) -> Option<String> {
    let trimmed = expr_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // A bare identifier substitutes the variable's text form directly (no
    // number formatting is applied to it).
    if is_valid_style_variable_identifier(trimmed) {
        return vars.get(trimmed);
    }

    let mut evaluator = Evaluator::new(trimmed, vars);
    evaluator.evaluate().ok()
}

/// The recursive-descent evaluator itself (reference `StyleVariableExpressionEvaluator`).
struct Evaluator<'a> {
    text: Vec<char>,
    vars: &'a dyn VarLookup,
    pos: usize,
    /// When false we are parsing (but discarding) the untaken branch of a
    /// ternary: value-level errors, `skip()` and variable lookups are suppressed.
    live: bool,
}

impl<'a> Evaluator<'a> {
    fn new(text: &str, vars: &'a dyn VarLookup) -> Self {
        Self {
            text: text.chars().collect(),
            vars,
            pos: 0,
            live: true,
        }
    }

    /// `Evaluate()`: parse the whole expression and format the result as text.
    fn evaluate(&mut self) -> Result<String, EvalError> {
        self.pos = 0;
        self.skip_whitespace();
        let value = self.parse_expression()?;
        self.skip_whitespace();
        if self.pos != self.text.len() {
            return Err(EvalError::failed(
                "Unexpected trailing characters in style variable expression",
            ));
        }
        Ok(match value {
            ExprValue::Number(d) => format_number(d),
            ExprValue::Text(s) => s,
        })
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.text.len() && matches!(self.text[self.pos], ' ' | '\t' | '\r' | '\n')
        {
            self.pos += 1;
        }
    }

    fn consume_char(&mut self, c: char) -> bool {
        self.skip_whitespace();
        if self.pos < self.text.len() && self.text[self.pos] == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Consume the multi-character operator `op` at the current position (after
    /// leading whitespace). The operator must match exactly, with no embedded
    /// whitespace.
    fn consume_operator(&mut self, op: &str) -> bool {
        self.skip_whitespace();
        let op_chars: Vec<char> = op.chars().collect();
        if self.text.len() - self.pos < op_chars.len() {
            return false;
        }
        let matches = self.text[self.pos..self.pos + op_chars.len()] == op_chars[..];
        if matches {
            self.pos += op_chars.len();
        }
        matches
    }

    /// Equality for `==` / `!=`: two numbers compare numerically, two strings by
    /// content, a number/string mismatch is unequal (not an error).
    fn values_equal(a: &ExprValue, b: &ExprValue) -> bool {
        match (a, b) {
            (ExprValue::Number(x), ExprValue::Number(y)) => x == y,
            (ExprValue::Text(x), ExprValue::Text(y)) => x == y,
            _ => false,
        }
    }

    fn parse_expression(&mut self) -> Result<ExprValue, EvalError> {
        self.parse_ternary()
    }

    /// `cond ? thenVal : elseVal`, right-associative, short-circuiting.
    fn parse_ternary(&mut self) -> Result<ExprValue, EvalError> {
        let cond = self.parse_equality()?;
        if !self.consume_char('?') {
            return Ok(cond);
        }

        let cond_true = require_number(&cond, self.live)? != 0.0;
        let prev_live = self.live;

        self.live = prev_live && cond_true;
        let then_val = self.parse_expression()?;
        self.live = prev_live;

        if !self.consume_char(':') {
            return Err(EvalError::failed(
                "Missing ':' for '?' in style variable expression",
            ));
        }

        self.live = prev_live && !cond_true;
        let else_val = self.parse_ternary()?;
        self.live = prev_live;

        Ok(if cond_true { then_val } else { else_val })
    }

    fn parse_equality(&mut self) -> Result<ExprValue, EvalError> {
        let mut v = self.parse_relational()?;
        loop {
            if self.consume_operator("==") {
                let eq = Self::values_equal(&v, &self.parse_relational()?);
                v = ExprValue::number(if eq { 1.0 } else { 0.0 });
            } else if self.consume_operator("!=") {
                let eq = Self::values_equal(&v, &self.parse_relational()?);
                v = ExprValue::number(if eq { 0.0 } else { 1.0 });
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn parse_relational(&mut self) -> Result<ExprValue, EvalError> {
        let mut v = self.parse_additive()?;
        loop {
            // Two-character operators before their single-character prefixes.
            if self.consume_operator("<=") {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_additive()?, self.live)?;
                v = ExprValue::number(if lhs <= rhs { 1.0 } else { 0.0 });
            } else if self.consume_operator(">=") {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_additive()?, self.live)?;
                v = ExprValue::number(if lhs >= rhs { 1.0 } else { 0.0 });
            } else if self.consume_operator("<") {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_additive()?, self.live)?;
                v = ExprValue::number(if lhs < rhs { 1.0 } else { 0.0 });
            } else if self.consume_operator(">") {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_additive()?, self.live)?;
                v = ExprValue::number(if lhs > rhs { 1.0 } else { 0.0 });
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn parse_additive(&mut self) -> Result<ExprValue, EvalError> {
        let mut v = self.parse_term()?;
        loop {
            self.skip_whitespace();
            if self.consume_char('+') {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_term()?, self.live)?;
                v = ExprValue::number(lhs + rhs);
            } else if self.consume_char('-') {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_term()?, self.live)?;
                v = ExprValue::number(lhs - rhs);
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn parse_term(&mut self) -> Result<ExprValue, EvalError> {
        let mut v = self.parse_factor()?;
        loop {
            self.skip_whitespace();
            if self.consume_char('*') {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_factor()?, self.live)?;
                v = ExprValue::number(lhs * rhs);
            } else if self.consume_char('/') {
                let lhs = require_number(&v, self.live)?;
                let rhs = require_number(&self.parse_factor()?, self.live)?;
                if rhs == 0.0 {
                    if self.live {
                        return Err(EvalError::failed(
                            "Division by zero in style variable expression",
                        ));
                    }
                    // Dead ternary branch: result discarded, skip the divide.
                    v = ExprValue::number(lhs);
                } else {
                    v = ExprValue::number(lhs / rhs);
                }
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn parse_factor(&mut self) -> Result<ExprValue, EvalError> {
        self.skip_whitespace();
        if self.consume_char('+') {
            let d = require_number(&self.parse_factor()?, self.live)?;
            return Ok(ExprValue::number(d));
        }
        if self.consume_char('-') {
            let d = require_number(&self.parse_factor()?, self.live)?;
            return Ok(ExprValue::number(-d));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<ExprValue, EvalError> {
        self.skip_whitespace();
        let c = match self.text.get(self.pos) {
            Some(&c) => c,
            None => {
                return Err(EvalError::failed(
                    "Unexpected end of style variable expression",
                ))
            }
        };

        if c == '(' {
            self.pos += 1;
            let v = self.parse_expression()?;
            self.skip_whitespace();
            if !self.consume_char(')') {
                return Err(EvalError::failed(
                    "Missing ')' in style variable expression",
                ));
            }
            return Ok(v);
        }

        if c == '`' {
            return self.parse_string_literal();
        }

        if c.is_ascii_digit() || c == '.' {
            return Ok(ExprValue::number(self.parse_number_literal()?));
        }

        if is_ident_start(c) {
            return self.parse_identifier_or_call();
        }

        Err(EvalError::failed(
            "Unexpected character in style variable expression",
        ))
    }

    /// Backtick-delimited string literal; a doubled backtick is one literal
    /// backtick and every other character is taken verbatim.
    fn parse_string_literal(&mut self) -> Result<ExprValue, EvalError> {
        self.pos += 1; // opening backtick
        let mut out = String::new();
        while self.pos < self.text.len() {
            let c = self.text[self.pos];
            if c == '`' {
                if self.pos + 1 < self.text.len() && self.text[self.pos + 1] == '`' {
                    out.push('`');
                    self.pos += 2;
                    continue;
                }
                self.pos += 1;
                return Ok(ExprValue::text(out));
            }
            out.push(c);
            self.pos += 1;
        }
        Err(EvalError::failed(
            "Unterminated string literal in style variable expression",
        ))
    }

    fn parse_number_literal(&mut self) -> Result<f64, EvalError> {
        let start = self.pos;
        let mut saw_digit = false;
        let mut saw_dot = false;
        while self.pos < self.text.len() {
            let c = self.text[self.pos];
            if c.is_ascii_digit() {
                saw_digit = true;
                self.pos += 1;
            } else if c == '.' && !saw_dot {
                saw_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos < self.text.len() && matches!(self.text[self.pos], 'e' | 'E') {
            self.pos += 1;
            if self.pos < self.text.len() && matches!(self.text[self.pos], '+' | '-') {
                self.pos += 1;
            }
            while self.pos < self.text.len() && self.text[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if !saw_digit {
            return Err(EvalError::failed(
                "Bad number literal in style variable expression",
            ));
        }
        let literal: String = self.text[start..self.pos].iter().collect();
        parse_double_invariant(&literal)
            .ok_or_else(|| EvalError::failed("Bad number literal in style variable expression"))
    }

    fn parse_identifier_or_call(&mut self) -> Result<ExprValue, EvalError> {
        let start = self.pos;
        while self.pos < self.text.len() && is_ident_cont(self.text[self.pos]) {
            self.pos += 1;
        }
        let ident: String = self.text[start..self.pos].iter().collect();
        self.skip_whitespace();

        if self.pos < self.text.len() && self.text[self.pos] == '(' {
            self.pos += 1;

            if ident == "skip" {
                if !self.consume_char(')') {
                    return Err(EvalError::failed(
                        "skip() takes no arguments in style variable expression",
                    ));
                }
                if self.live {
                    return Err(EvalError::Skip);
                }
                // Dead ternary branch: value discarded.
                return Ok(ExprValue::number(0.0));
            }

            // Note: the reference parses both arguments before checking the
            // function name, so an unknown function is still syntax-checked.
            let a = require_number(&self.parse_expression()?, self.live)?;
            if !self.consume_char(',') {
                return Err(EvalError::failed(
                    "Expected ',' in min/max style variable call",
                ));
            }
            let b = require_number(&self.parse_expression()?, self.live)?;
            if !self.consume_char(')') {
                return Err(EvalError::failed(
                    "Missing ')' after min/max style variable call",
                ));
            }

            return match ident.as_str() {
                "min" => Ok(ExprValue::number(if a < b { a } else { b })),
                "max" => Ok(ExprValue::number(if a > b { a } else { b })),
                _ => {
                    if self.live {
                        Err(EvalError::failed(
                            "Unknown function in style variable expression",
                        ))
                    } else {
                        Ok(ExprValue::number(0.0))
                    }
                }
            };
        }

        self.lookup_variable(&ident)
    }

    fn lookup_variable(&mut self, name: &str) -> Result<ExprValue, EvalError> {
        // Dead branch: value discarded, so skip lookup and value-level errors.
        if !self.live {
            return Ok(ExprValue::text(""));
        }

        // The reference keeps a numeric flag per captured value and returns a
        // Number for numeric captures / a String for substitutable string
        // captures / an error for opaque captures. `VarLookup` exposes text only,
        // so a value whose text is a valid number is treated as numeric. For the
        // values this theme captures (`ActualWidth` -> "1234", `Tag` ->
        // "vertical") that is equivalent to the reference.
        match self.vars.get(name) {
            // Undefined reads as the empty string sentinel, so a fallback can be
            // written as `{{var==``?default:var}}`.
            None => Ok(ExprValue::text("")),
            Some(text) => match parse_double_invariant(&text) {
                Some(n) => Ok(ExprValue::number(n)),
                None => Ok(ExprValue::text(text)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Vars(HashMap<String, String>);

    impl Vars {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        }
    }

    impl VarLookup for Vars {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn expand(input: &str, pairs: &[(&str, &str)]) -> Option<String> {
        expand_style_variables(input, &Vars::new(pairs))
    }

    #[test]
    fn identifier_validation() {
        for ok in ["a", "A", "_", "_x1", "abcDEF_09"] {
            assert!(is_valid_style_variable_identifier(ok), "{ok}");
        }
        for bad in ["", "1a", "-x", "a-b", "a.b", "a b", "é"] {
            assert!(!is_valid_style_variable_identifier(bad), "{bad}");
        }
    }

    #[test]
    fn number_literals_and_formatting() {
        assert_eq!(expand("{{1}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{1.0}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{0.5}}", &[]).as_deref(), Some("0.5"));
        assert_eq!(expand("{{.5}}", &[]).as_deref(), Some("0.5"));
        assert_eq!(expand("{{2.5e3}}", &[]).as_deref(), Some("2500"));
        assert_eq!(expand("{{-3}}", &[]).as_deref(), Some("-3"));
        assert_eq!(expand("{{+3}}", &[]).as_deref(), Some("3"));
        assert_eq!(expand("{{1000}}", &[]).as_deref(), Some("1000"));
        assert_eq!(expand("{{1/2}}", &[]).as_deref(), Some("0.5"));
        assert_eq!(expand("{{1/0}}", &[]), None);
        assert_eq!(expand("{{.}}", &[]), None);
        assert_eq!(expand("{{1+}}", &[]), None);
    }

    #[test]
    fn arithmetic_precedence() {
        assert_eq!(expand("{{1+2*3}}", &[]).as_deref(), Some("7"));
        assert_eq!(expand("{{(1+2)*3}}", &[]).as_deref(), Some("9"));
        assert_eq!(expand("{{10-2-3}}", &[]).as_deref(), Some("5"));
        assert_eq!(expand("{{8/2/2}}", &[]).as_deref(), Some("2"));
        assert_eq!(expand("{{-2*3}}", &[]).as_deref(), Some("-6"));
        assert_eq!(expand("{{2*-3}}", &[]).as_deref(), Some("-6"));
    }

    #[test]
    fn comparisons() {
        assert_eq!(expand("{{1<2}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{2<=2}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{2>2}}", &[]).as_deref(), Some("0"));
        assert_eq!(expand("{{3>=2}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{1==1}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{1!=1}}", &[]).as_deref(), Some("0"));
        assert_eq!(expand("{{1==1.0}}", &[]).as_deref(), Some("1"));
        // Two-character operators are matched before their prefixes.
        assert_eq!(expand("{{1<=1}}", &[]).as_deref(), Some("1"));
        assert_eq!(expand("{{1>=1}}", &[]).as_deref(), Some("1"));
    }

    #[test]
    fn string_literals_and_comparison() {
        assert_eq!(expand("{{`hello`}}", &[]).as_deref(), Some("hello"));
        assert_eq!(expand("{{`a``b`}}", &[]).as_deref(), Some("a`b"));
        assert_eq!(
            expand(
                "{{taskbarDock==`vertical`}}",
                &[("taskbarDock", "vertical")]
            )
            .as_deref(),
            Some("1")
        );
        assert_eq!(
            expand(
                "{{taskbarDock==`vertical`}}",
                &[("taskbarDock", "horizontal")]
            )
            .as_deref(),
            Some("0")
        );
        // A numeric variable compared with a string literal: number/string
        // mismatch is unequal, not an error (the reference compares the two
        // unboxed variants and a mismatch is false).
        assert_eq!(expand("{{x==`7`}}", &[("x", "7")]).as_deref(), Some("0"));
        assert_eq!(expand("{{`unterminated}}", &[]), None);
    }

    #[test]
    fn undefined_variable_reads_as_empty_string() {
        // A bare identifier that is undefined is refused (style skipped) ...
        assert_eq!(expand("{{missing}}", &[]), None);
        // ... but comparing it to the empty backtick string works.
        assert_eq!(expand("{{missing==``}}", &[]).as_deref(), Some("1"));
        // Arithmetic on it fails, it is not silently zero.
        assert_eq!(expand("{{missing+1}}", &[]), None);
    }

    #[test]
    fn variables_are_numeric_when_they_parse_as_numbers() {
        assert_eq!(expand("{{w+1}}", &[("w", "10")]).as_deref(), Some("11"));
        assert_eq!(
            expand("{{w>0?`wide`:`narrow`}}", &[("w", "10")]).as_deref(),
            Some("wide")
        );
        assert_eq!(
            expand("{{w>0?`wide`:`narrow`}}", &[("w", "0")]).as_deref(),
            Some("narrow")
        );
    }

    #[test]
    fn min_max_and_ternary() {
        assert_eq!(expand("{{min(3,5)}}", &[]).as_deref(), Some("3"));
        assert_eq!(expand("{{max(3,5)}}", &[]).as_deref(), Some("5"));
        assert_eq!(expand("{{min(3,5)+max(1,2)}}", &[]).as_deref(), Some("5"));
        assert_eq!(expand("{{1?2:3}}", &[]).as_deref(), Some("2"));
        assert_eq!(expand("{{0?2:3}}", &[]).as_deref(), Some("3"));
        // Right-associative conditional.
        assert_eq!(expand("{{0?1:1?2:3}}", &[]).as_deref(), Some("2"));
        // Dead branches are not evaluated: division by zero and skip() are fine.
        assert_eq!(expand("{{1?5:1/0}}", &[]).as_deref(), Some("5"));
        assert_eq!(expand("{{1?5:skip()}}", &[]).as_deref(), Some("5"));
        // A live skip() means the style must be left unapplied.
        assert_eq!(expand("{{1?skip():5}}", &[]), None);
        // Unknown function is an error in a live branch, tolerated when dead.
        assert_eq!(expand("{{1?2:foo(1,2)}}", &[]).as_deref(), Some("2"));
        assert_eq!(expand("{{foo(1,2)}}", &[]), None);
    }

    #[test]
    fn theme_max_width_expression() {
        // After ApplyStyleConstants has replaced $TaskbarFrameMaxWidth with 1895.
        let expr = "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}";
        assert_eq!(
            expand(expr, &[("containerGridWidth", "1000")]).as_deref(),
            Some("1000")
        );
        assert_eq!(
            expand(expr, &[("containerGridWidth", "2000")]).as_deref(),
            Some("1895")
        );
        assert_eq!(
            expand(expr, &[("containerGridWidth", "0")]).as_deref(),
            Some("1895")
        );
        assert_eq!(expand(expr, &[]), None);
    }

    #[test]
    fn theme_width_expression_skip_or_auto() {
        let expr = "{{taskbarDock==`vertical`?skip():`Auto`}}";
        assert_eq!(expand(expr, &[("taskbarDock", "vertical")]), None);
        assert_eq!(
            expand(expr, &[("taskbarDock", "horizontal")]).as_deref(),
            Some("Auto")
        );
        // An undefined variable reads as the empty string, which is unequal to
        // `vertical`, so the else branch is taken.
        assert_eq!(expand(expr, &[]).as_deref(), Some("Auto"));
    }

    #[test]
    fn expansion_is_innermost_and_output_not_rescanned() {
        assert_eq!(expand("a{{1+1}}b{{2*2}}c", &[]).as_deref(), Some("a2b4c"));
        // {{{x}}} -> '{' + value + '}'
        assert_eq!(expand("{{{x}}}", &[("x", "7")]).as_deref(), Some("{7}"));
        // Substituted output is literal.
        assert_eq!(expand("{{a}}", &[("a", "{{b}}")]).as_deref(), Some("{{b}}"));
        // Unmatched '}}' is a failure.
        assert_eq!(expand("x}}y", &[]), None);
        assert_eq!(expand("plain text", &[]).as_deref(), Some("plain text"));
    }

    #[test]
    fn expressions_with_whitespace() {
        assert_eq!(
            expand(
                "{{ containerGridWidth > 0 ? 1 : 2 }}",
                &[("containerGridWidth", "5")]
            )
            .as_deref(),
            Some("1")
        );
        // Whole expression text is trimmed before evaluation.
        assert_eq!(expand("{{ 42 }}", &[]).as_deref(), Some("42"));
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        assert_eq!(expand("{{1 2}}", &[]), None);
        assert_eq!(expand("{{1)}}", &[]), None);
        assert_eq!(expand("{{(1}}", &[]), None);
    }
}
