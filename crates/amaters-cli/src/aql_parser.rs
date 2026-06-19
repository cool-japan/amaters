//! AQL (AmateRS Query Language) filter expression parser.
//!
//! BNF:
//!   expr       := or_expr
//!   or_expr    := and_expr ('OR' and_expr)*
//!   and_expr   := not_expr ('AND' not_expr)*
//!   not_expr   := 'NOT' not_expr | comparison
//!   comparison := '(' expr ')' | field op value
//!   op         := '==' | '!=' | '<' | '<=' | '>' | '>=' | 'CONTAINS'
//!   value      := string_lit | number_lit | bool_lit
//!   field      := identifier ('.' identifier)*

/// Comparison operator.
#[derive(Debug, Clone, PartialEq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
}

/// A literal value on the right-hand side of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum AqlValue {
    Str(String),
    Number(f64),
    Bool(bool),
}

/// AQL predicate tree.
#[derive(Debug, Clone)]
pub enum AqlPredicate {
    Comparison {
        field: String,
        op: CompareOp,
        value: AqlValue,
    },
    And(Box<AqlPredicate>, Box<AqlPredicate>),
    Or(Box<AqlPredicate>, Box<AqlPredicate>),
    Not(Box<AqlPredicate>),
}

/// A lexer token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Str(String),
    Number(f64),
    Bool(bool),
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    And,
    Or,
    Not,
    LParen,
    RParen,
}

/// Tokenise an AQL input string into a `Vec<Token>`.
pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        // String literal "..."
        if chars[i] == '"' {
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    match chars[i] {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        c => {
                            s.push('\\');
                            s.push(c);
                        }
                    }
                } else if chars[i] == '"' {
                    closed = true;
                    i += 1;
                    break;
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if !closed {
                return Err("Unterminated string literal".to_string());
            }
            tokens.push(Token::Str(s));
            continue;
        }

        // Two-character operators
        if i + 1 < chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            match two.as_str() {
                "==" => {
                    tokens.push(Token::Eq);
                    i += 2;
                    continue;
                }
                "!=" => {
                    tokens.push(Token::Ne);
                    i += 2;
                    continue;
                }
                "<=" => {
                    tokens.push(Token::Lte);
                    i += 2;
                    continue;
                }
                ">=" => {
                    tokens.push(Token::Gte);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Single-character operators / punctuation
        match chars[i] {
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
                continue;
            }
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
                continue;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
                continue;
            }
            _ => {}
        }

        // Number literal (leading '-' is NOT handled here; use unary negation outside)
        if chars[i].is_ascii_digit()
            || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            if chars[i] == '-' {
                i += 1;
            }
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            let n: f64 = num_str
                .parse()
                .map_err(|_| format!("Invalid number literal: '{num_str}'"))?;
            tokens.push(Token::Number(n));
            continue;
        }

        // Identifier or keyword
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let tok = match word.to_uppercase().as_str() {
                "AND" => Token::And,
                "OR" => Token::Or,
                "NOT" => Token::Not,
                "CONTAINS" => Token::Contains,
                "TRUE" => Token::Bool(true),
                "FALSE" => Token::Bool(false),
                _ => Token::Ident(word),
            };
            tokens.push(tok);
            continue;
        }

        return Err(format!("Unexpected character: '{}'", chars[i]));
    }

    Ok(tokens)
}

/// Recursive-descent AQL parser.
pub struct AqlParser {
    input: Vec<Token>,
    pos: usize,
}

impl AqlParser {
    /// Create a new parser from a token stream.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            input: tokens,
            pos: 0,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.input.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.input.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        match self.advance() {
            Some(t) if t == expected => Ok(()),
            Some(t) => Err(format!("Expected {:?} but found {:?}", expected, t)),
            None => Err(format!("Expected {:?} but reached end of input", expected)),
        }
    }

    /// Parse the top-level expression.
    pub fn parse_expr(&mut self) -> Result<AqlPredicate, String> {
        let pred = self.parse_or()?;
        if self.pos < self.input.len() {
            return Err(format!(
                "Unexpected token at position {}: {:?}",
                self.pos, self.input[self.pos]
            ));
        }
        Ok(pred)
    }

    fn parse_or(&mut self) -> Result<AqlPredicate, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = AqlPredicate::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<AqlPredicate, String> {
        let mut left = self.parse_not()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_not()?;
            left = AqlPredicate::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<AqlPredicate, String> {
        if self.peek() == Some(&Token::Not) {
            self.advance();
            let inner = self.parse_not()?;
            return Ok(AqlPredicate::Not(Box::new(inner)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<AqlPredicate, String> {
        // Parenthesized group
        if self.peek() == Some(&Token::LParen) {
            self.advance();
            let inner = self.parse_or()?;
            self.expect(&Token::RParen)?;
            return Ok(inner);
        }

        // field op value
        let field = match self.advance() {
            Some(Token::Ident(name)) => name.clone(),
            Some(tok) => {
                return Err(format!("Expected field name but found {:?}", tok));
            }
            None => return Err("Expected field name but reached end of input".to_string()),
        };

        let op = match self.advance() {
            Some(Token::Eq) => CompareOp::Eq,
            Some(Token::Ne) => CompareOp::Ne,
            Some(Token::Lt) => CompareOp::Lt,
            Some(Token::Lte) => CompareOp::Lte,
            Some(Token::Gt) => CompareOp::Gt,
            Some(Token::Gte) => CompareOp::Gte,
            Some(Token::Contains) => CompareOp::Contains,
            Some(tok) => {
                return Err(format!("Expected comparison operator but found {:?}", tok));
            }
            None => return Err("Expected comparison operator but reached end of input".to_string()),
        };

        let value = match self.advance() {
            Some(Token::Str(s)) => AqlValue::Str(s.clone()),
            Some(Token::Number(n)) => AqlValue::Number(*n),
            Some(Token::Bool(b)) => AqlValue::Bool(*b),
            Some(tok) => {
                return Err(format!("Expected value literal but found {:?}", tok));
            }
            None => return Err("Expected value literal but reached end of input".to_string()),
        };

        Ok(AqlPredicate::Comparison { field, op, value })
    }
}

/// Top-level convenience: tokenize then parse.
pub fn parse_filter(input: &str) -> Result<AqlPredicate, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err("Empty filter expression".to_string());
    }
    let mut parser = AqlParser::new(tokens);
    parser.parse_expr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_equality() {
        let pred = parse_filter(r#"name == "Alice""#).expect("should parse");
        match pred {
            AqlPredicate::Comparison { field, op, value } => {
                assert_eq!(field, "name");
                assert_eq!(op, CompareOp::Eq);
                assert_eq!(value, AqlValue::Str("Alice".to_string()));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_numeric_comparison() {
        let pred = parse_filter("age > 30").expect("should parse");
        match pred {
            AqlPredicate::Comparison { field, op, value } => {
                assert_eq!(field, "age");
                assert_eq!(op, CompareOp::Gt);
                assert_eq!(value, AqlValue::Number(30.0));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_and_expression() {
        let pred = parse_filter(r#"age > 30 AND status == "active""#).expect("should parse");
        match pred {
            AqlPredicate::And(left, right) => {
                match *left {
                    AqlPredicate::Comparison { field, op, value } => {
                        assert_eq!(field, "age");
                        assert_eq!(op, CompareOp::Gt);
                        assert_eq!(value, AqlValue::Number(30.0));
                    }
                    _ => panic!("Expected left Comparison"),
                }
                match *right {
                    AqlPredicate::Comparison { field, op, value } => {
                        assert_eq!(field, "status");
                        assert_eq!(op, CompareOp::Eq);
                        assert_eq!(value, AqlValue::Str("active".to_string()));
                    }
                    _ => panic!("Expected right Comparison"),
                }
            }
            _ => panic!("Expected And"),
        }
    }

    #[test]
    fn test_or_expression() {
        let pred = parse_filter(r#"x == 1 OR y == 2"#).expect("should parse");
        match pred {
            AqlPredicate::Or(_, _) => {}
            _ => panic!("Expected Or"),
        }
    }

    #[test]
    fn test_not_expression() {
        let pred = parse_filter("NOT (deleted == true)").expect("should parse");
        match pred {
            AqlPredicate::Not(inner) => match *inner {
                AqlPredicate::Comparison { field, op, value } => {
                    assert_eq!(field, "deleted");
                    assert_eq!(op, CompareOp::Eq);
                    assert_eq!(value, AqlValue::Bool(true));
                }
                _ => panic!("Expected inner Comparison"),
            },
            _ => panic!("Expected Not"),
        }
    }

    #[test]
    fn test_nested_parens() {
        let pred = parse_filter(r#"(a == 1 OR b == 2) AND c == 3"#).expect("should parse");
        match pred {
            AqlPredicate::And(left, right) => {
                match *left {
                    AqlPredicate::Or(_, _) => {}
                    _ => panic!("Expected left Or"),
                }
                match *right {
                    AqlPredicate::Comparison { field, .. } => {
                        assert_eq!(field, "c");
                    }
                    _ => panic!("Expected right Comparison"),
                }
            }
            _ => panic!("Expected And"),
        }
    }

    #[test]
    fn test_contains_operator() {
        let pred = parse_filter(r#"name CONTAINS "Smith""#).expect("should parse");
        match pred {
            AqlPredicate::Comparison { op, .. } => {
                assert_eq!(op, CompareOp::Contains);
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_lte_gte_operators() {
        let pred = parse_filter("score >= 100").expect("should parse");
        match pred {
            AqlPredicate::Comparison { op, value, .. } => {
                assert_eq!(op, CompareOp::Gte);
                assert_eq!(value, AqlValue::Number(100.0));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_complex_range_filter() {
        let pred = parse_filter(r#"status == "active" OR (score >= 100 AND score <= 200)"#)
            .expect("should parse");
        match pred {
            AqlPredicate::Or(_, _) => {}
            _ => panic!("Expected Or at top level"),
        }
    }

    #[test]
    fn test_invalid_syntax_returns_error() {
        assert!(
            parse_filter("age >").is_err(),
            "Truncated expression should fail"
        );
    }

    #[test]
    fn test_unbalanced_parens_error() {
        assert!(
            parse_filter("(a == 1 AND b == 2").is_err(),
            "Unbalanced parens should fail"
        );
    }

    #[test]
    fn test_empty_filter_error() {
        assert!(parse_filter("").is_err(), "Empty filter should fail");
    }

    #[test]
    fn test_unknown_char_error() {
        assert!(
            parse_filter("age @ 30").is_err(),
            "Unknown char should fail"
        );
    }

    #[test]
    fn test_not_not_double_negation() {
        let pred = parse_filter("NOT NOT (active == true)").expect("should parse double NOT");
        match pred {
            AqlPredicate::Not(inner) => match *inner {
                AqlPredicate::Not(_) => {}
                _ => panic!("Expected inner Not"),
            },
            _ => panic!("Expected outer Not"),
        }
    }

    #[test]
    fn test_bool_false_value() {
        let pred = parse_filter("enabled == false").expect("should parse");
        match pred {
            AqlPredicate::Comparison { value, .. } => {
                assert_eq!(value, AqlValue::Bool(false));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_ne_operator() {
        let pred = parse_filter(r#"status != "deleted""#).expect("should parse");
        match pred {
            AqlPredicate::Comparison { op, .. } => {
                assert_eq!(op, CompareOp::Ne);
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_lt_operator() {
        let pred = parse_filter("age < 18").expect("should parse");
        match pred {
            AqlPredicate::Comparison { op, value, .. } => {
                assert_eq!(op, CompareOp::Lt);
                assert_eq!(value, AqlValue::Number(18.0));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_string_escape_sequences() {
        let pred = parse_filter(r#"msg == "hello \"world\"""#).expect("should parse");
        match pred {
            AqlPredicate::Comparison { value, .. } => {
                assert_eq!(value, AqlValue::Str("hello \"world\"".to_string()));
            }
            _ => panic!("Expected Comparison"),
        }
    }

    #[test]
    fn test_extra_tokens_after_expr() {
        // Trailing tokens that aren't part of the expression should error
        assert!(
            parse_filter("a == 1 garbage").is_err(),
            "Trailing garbage should fail"
        );
    }

    #[test]
    fn test_dotted_field_name() {
        // Field names with dots (e.g. user.name) should tokenize as a single Ident
        let pred = parse_filter(r#"user.name == "Bob""#).expect("should parse dotted field");
        match pred {
            AqlPredicate::Comparison { field, .. } => {
                assert_eq!(field, "user.name");
            }
            _ => panic!("Expected Comparison"),
        }
    }
}
