//! Evaluation of node `when:` conditions.
//!
//! A `when:` expression gates whether a node runs (after its `trigger_rule` is
//! already satisfied). The grammar is deliberately small — enough for the
//! routing/branching patterns workflows actually use, no more:
//!
//! ```text
//! expr := or
//! or   := and ('||' and)*
//! and  := cmp ('&&' cmp)*
//! cmp  := value (('=='|'!=') value)?
//! value := $node.output[.field…] | $HARNESS_VAR | 'literal' | "literal" | bareword
//! ```
//!
//! A lone `value` (no operator) is **truthy** when it resolves to a non-empty
//! string other than `false`, `0`, or `null`. Comparisons are string equality
//! after resolution (operand values are compared as their rendered strings).
//! Operators bind looser than comparison: `||` < `&&` < `==`/`!=`.
//!
//! Resolution is lenient (a missing node ref → empty string), matching
//! [`crate::vars`]. A structurally invalid expression (e.g. a dangling operator)
//! is a hard [`DagError::InvalidCondition`] so authoring mistakes fail loud
//! rather than silently skipping or running a node.

use crate::error::DagError;
use crate::vars::VarContext;

/// Evaluate a `when:` expression against the current variable context.
pub fn eval_when(expr: &str, ctx: &VarContext) -> Result<bool, DagError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(invalid(expr, "expression is empty"));
    }
    eval_or(trimmed, ctx, expr)
}

/// Validate a `when:` expression's *structure* without resolving any values —
/// used at parse time so malformed conditions fail loud before a run starts.
/// Catches empty expressions, dangling/empty operands around `&&`/`||`, and
/// comparisons missing an operand.
pub fn validate_syntax(expr: &str) -> Result<(), DagError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(invalid(expr, "expression is empty"));
    }
    for or_part in split_top(trimmed, "||") {
        for and_part in split_top(or_part, "&&") {
            let cmp = and_part.trim();
            if cmp.is_empty() {
                return Err(invalid(expr, "empty operand around a boolean operator"));
            }
            if let Some((lhs, _, rhs)) = split_comparison(cmp) {
                if lhs.trim().is_empty() || rhs.trim().is_empty() {
                    return Err(invalid(expr, "comparison is missing an operand"));
                }
            }
        }
    }
    Ok(())
}

fn eval_or(s: &str, ctx: &VarContext, full: &str) -> Result<bool, DagError> {
    for part in split_top(s, "||") {
        if eval_and(part, ctx, full)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn eval_and(s: &str, ctx: &VarContext, full: &str) -> Result<bool, DagError> {
    for part in split_top(s, "&&") {
        if !eval_cmp(part, ctx, full)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn eval_cmp(s: &str, ctx: &VarContext, full: &str) -> Result<bool, DagError> {
    // Look for a top-level comparison operator (outside quotes).
    if let Some((lhs, op, rhs)) = split_comparison(s) {
        let l = resolve(lhs.trim(), ctx, full)?;
        let r = resolve(rhs.trim(), ctx, full)?;
        return Ok(match op {
            Op::Eq => l == r,
            Op::Ne => l != r,
        });
    }
    // No operator: a lone operand is truthy unless it's a falsey value.
    let v = resolve(s.trim(), ctx, full)?;
    Ok(!matches!(v.trim(), "" | "false" | "0" | "null"))
}

#[derive(Clone, Copy)]
enum Op {
    Eq,
    Ne,
}

/// Find a top-level (not quote-enclosed) `==` or `!=`, returning the operands.
fn split_comparison(s: &str) -> Option<(&str, Op, &str)> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'\'' || c == b'"' {
                    quote = Some(c);
                } else if c == b'=' && bytes[i + 1] == b'=' {
                    return Some((&s[..i], Op::Eq, &s[i + 2..]));
                } else if c == b'!' && bytes[i + 1] == b'=' {
                    return Some((&s[..i], Op::Ne, &s[i + 2..]));
                }
            }
        }
        i += 1;
    }
    None
}

/// Split `s` on a top-level (not quote-enclosed) `sep` (`"&&"` or `"||"`).
fn split_top<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    let bytes = s.as_bytes();
    let sb = sep.as_bytes();
    let mut parts = Vec::new();
    let mut quote: Option<u8> = None;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                i += 1;
            }
            None => {
                if c == b'\'' || c == b'"' {
                    quote = Some(c);
                    i += 1;
                } else if i + sb.len() <= bytes.len() && &bytes[i..i + sb.len()] == sb {
                    parts.push(&s[start..i]);
                    i += sb.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Resolve a single operand token to its string value.
fn resolve(token: &str, ctx: &VarContext, full: &str) -> Result<String, DagError> {
    if token.is_empty() {
        return Err(invalid(full, "missing operand"));
    }
    // Quoted string literal.
    if (token.starts_with('\'') && token.ends_with('\'') && token.len() >= 2)
        || (token.starts_with('"') && token.ends_with('"') && token.len() >= 2)
    {
        return Ok(token[1..token.len() - 1].to_string());
    }
    // Variable / node-output reference: substitute reuses the same grammar and
    // resolution rules as prompt bodies (node refs lenient, harness vars strict).
    if token.starts_with('$') {
        return crate::vars::substitute(token, ctx).map_err(|e| invalid(full, &e.to_string()));
    }
    // Bareword (true / false / number / unquoted enum) — taken literally.
    Ok(token.to_string())
}

fn invalid(expr: &str, why: &str) -> DagError {
    DagError::InvalidCondition {
        expr: expr.trim().to_string(),
        reason: why.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> VarContext {
        VarContext::new()
            .set_node_output("classify", r#"{"type":"BUG","proceed":true,"count":0}"#)
            .set_node_output("verdict", "SAFE")
            .set("ARGUMENTS", "hello")
    }

    fn ev(expr: &str) -> bool {
        eval_when(expr, &ctx()).unwrap()
    }

    #[test]
    fn equality_on_json_field() {
        assert!(ev("$classify.output.type == 'BUG'"));
        assert!(!ev("$classify.output.type == 'FEATURE'"));
    }

    #[test]
    fn inequality_on_raw_output() {
        assert!(ev("$verdict.output != 'UNSAFE'"));
        assert!(!ev("$verdict.output != 'SAFE'"));
    }

    #[test]
    fn boolean_field_truthiness() {
        assert!(ev("$classify.output.proceed")); // "true"
        assert!(!ev("$classify.output.count")); // "0" is falsey
        assert!(!ev("$classify.output.missing")); // absent → "" falsey
    }

    #[test]
    fn and_or_precedence() {
        assert!(ev(
            "$classify.output.type == 'BUG' && $verdict.output == 'SAFE'"
        ));
        assert!(!ev(
            "$classify.output.type == 'BUG' && $verdict.output == 'UNSAFE'"
        ));
        assert!(ev(
            "$classify.output.type == 'FEATURE' || $verdict.output == 'SAFE'"
        ));
    }

    #[test]
    fn quoted_literal_containing_operator_chars() {
        assert!(ev("$verdict.output == 'SAFE'"));
        assert!(ev("'a&&b' == 'a&&b'"));
    }

    #[test]
    fn empty_expression_is_invalid() {
        assert!(eval_when("   ", &ctx()).is_err());
    }

    #[test]
    fn dangling_operator_is_invalid() {
        assert!(eval_when("$verdict.output ==", &ctx()).is_err());
    }
}
