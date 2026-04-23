//! Text → Op stream. Full SAPF postfix grammar (M1).
//!
//! Handles: integer/float literals with suffixes (k M m u pi h), scientific
//! notation, infix fractions (5/4), strings, `[...]`, `#[...]`, `{:k v}`,
//! `\args [body]`, `` `quote ``, `'sym`, `,formget`, `.formapply`, `=bind`,
//! `=(...)`/`=[...]` multi-bind, `!` call, adverb suffixes (`+/` `+\` `+^`).

use std::sync::Arc;

use stax_core::{op::Adverb, Error, Op, Result, Value};

pub fn parse(source: &str) -> Result<Vec<Op>> {
    let mut p = Parser::new(source);
    p.parse_all()
}

// -------------------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Parser {
    fn new(src: &str) -> Self {
        Self { chars: src.chars().collect(), pos: 0, line: 1, col: 1 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' { self.line += 1; self.col = 1; } else { self.col += 1; }
        Some(c)
    }

    fn parse_err(&self, msg: impl Into<String>) -> Error {
        Error::Parse { line: self.line, col: self.col, msg: msg.into() }
    }

    fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => { self.advance(); }
                Some(';') => {
                    while matches!(self.peek(), Some(c) if c != '\n') { self.advance(); }
                }
                _ => break,
            }
        }
    }

    pub fn parse_all(&mut self) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        loop {
            self.skip_ws();
            if self.peek().is_none() { break; }
            let more = self.parse_one()?;
            ops.extend(more);
        }
        Ok(ops)
    }

    fn parse_one(&mut self) -> Result<Vec<Op>> {
        match self.peek() {
            None => Ok(vec![]),

            Some('"') => {
                let s = self.parse_string()?;
                Ok(vec![Op::Lit(Value::Str(Arc::from(s.as_str())))])
            }

            // #[ signal list
            Some('#') if self.peek2() == Some('[') => {
                self.advance(); self.advance();
                let body = self.parse_until(']')?;
                let mut ops = vec![Op::ListMark];
                ops.extend(body);
                ops.push(Op::MakeList { signal: true });
                Ok(ops)
            }

            Some('[') => {
                self.advance();
                let body = self.parse_until(']')?;
                let mut ops = vec![Op::ListMark];
                ops.extend(body);
                ops.push(Op::MakeList { signal: false });
                Ok(ops)
            }

            Some('{') => {
                self.advance();
                self.parse_form()
            }

            // ` quote
            Some('`') => {
                self.advance();
                let name = self.read_word();
                Ok(vec![Op::Quote(Arc::from(name.as_str()))])
            }

            // ' sym
            Some('\'') => {
                self.advance();
                let name = self.read_word();
                Ok(vec![Op::Sym(Arc::from(name.as_str()))])
            }

            // , form-get
            Some(',') => {
                self.advance();
                let name = self.read_word();
                Ok(vec![Op::FormGet(Arc::from(name.as_str()))])
            }

            // . form-apply
            Some('.') if self.peek2().is_some_and(is_word_start) => {
                self.advance();
                let name = self.read_word();
                Ok(vec![Op::FormApply(Arc::from(name.as_str()))])
            }

            // = bind
            Some('=') if self.peek2().is_some_and(|c| c != '=') => {
                self.advance();
                self.skip_ws();
                match self.peek() {
                    Some('(') => {
                        self.advance();
                        let names = self.parse_name_list(')')?;
                        Ok(vec![Op::BindMany { names: Arc::from(names), list_mode: false }])
                    }
                    Some('[') => {
                        self.advance();
                        let names = self.parse_name_list(']')?;
                        Ok(vec![Op::BindMany { names: Arc::from(names), list_mode: true }])
                    }
                    _ => {
                        let name = self.read_word();
                        if name.is_empty() {
                            return Err(self.parse_err("'=' must be followed by a name"));
                        }
                        Ok(vec![Op::Bind(Arc::from(name.as_str()))])
                    }
                }
            }

            Some('!') => {
                self.advance();
                Ok(vec![Op::Call])
            }

            // \ lambda
            Some('\\') => {
                self.advance();
                self.parse_lambda()
            }

            // Negative number
            Some('-') if self.peek2().is_some_and(|c| c.is_ascii_digit()) => {
                let n = self.parse_number()?;
                Ok(vec![Op::Lit(Value::Real(n))])
            }

            // Positive number
            Some(c) if c.is_ascii_digit() => {
                let n = self.parse_number()?;
                Ok(vec![Op::Lit(Value::Real(n))])
            }

            // Regular word (or adverb)
            Some(c) if is_word_start(c) || is_word_only(c) => {
                Ok(self.parse_word_or_adverb())
            }

            // Skip unknown
            _ => {
                self.advance();
                Ok(vec![])
            }
        }
    }

    fn parse_string(&mut self) -> Result<String> {
        self.advance(); // opening "
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err(self.parse_err("unterminated string")),
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some(c) => { s.push('\\'); s.push(c); }
                    None => return Err(self.parse_err("unterminated escape")),
                },
                Some(c) => s.push(c),
            }
        }
        Ok(s)
    }

    fn parse_until(&mut self, close: char) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.parse_err(format!("expected '{close}'"))),
                Some(c) if c == close => { self.advance(); break; }
                _ => ops.extend(self.parse_one()?),
            }
        }
        Ok(ops)
    }

    fn parse_form(&mut self) -> Result<Vec<Op>> {
        // { [:parent-expr] :key1 val1 :key2 val2 }
        let mut keys: Vec<Arc<str>> = Vec::new();
        let mut val_ops: Vec<Op> = Vec::new();
        let mut has_parent = false;

        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.parse_err("unclosed '{'")),
                Some('}') => { self.advance(); break; }
                Some(':') => {
                    self.advance();
                    let key = self.read_word();
                    keys.push(Arc::from(key.as_str()));
                    self.skip_ws();
                    val_ops.extend(self.parse_one()?);
                }
                _ => {
                    // parent-form expression before the first :key
                    val_ops.extend(self.parse_one()?);
                    has_parent = true;
                }
            }
        }
        let mut ops = val_ops;
        ops.push(Op::MakeForm {
            keys: Arc::from(keys),
            parent: has_parent,
        });
        Ok(ops)
    }

    fn parse_name_list(&mut self, close: char) -> Result<Vec<Arc<str>>> {
        let mut names = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.parse_err("unclosed name list")),
                Some(c) if c == close => { self.advance(); break; }
                _ => {
                    let name = self.read_word();
                    if !name.is_empty() {
                        names.push(Arc::from(name.as_str()));
                    }
                }
            }
        }
        Ok(names)
    }

    fn parse_lambda(&mut self) -> Result<Vec<Op>> {
        // \name1 name2 ... [body]  or  \[body]
        let mut params: Vec<Arc<str>> = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some('[') => {
                    self.advance();
                    let body_ops = self.parse_until(']')?;
                    let body: Arc<[Op]> = Arc::from(body_ops.as_slice());
                    return Ok(vec![Op::MakeFun {
                        params: Arc::from(params),
                        body,
                    }]);
                }
                Some(c) if is_word_start(c) || is_word_only(c) => {
                    let name = self.read_word();
                    if !name.is_empty() {
                        params.push(Arc::from(name.as_str()));
                    }
                }
                _ => return Err(self.parse_err("lambda: expected '[body]'")),
            }
        }
    }

    fn parse_number(&mut self) -> Result<f64> {
        let mut s = String::new();

        if self.peek() == Some('-') {
            s.push('-');
            self.advance();
        }

        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            s.push(self.advance().unwrap());
        }

        // Decimal part
        if self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit()) {
            s.push(self.advance().unwrap()); // '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                s.push(self.advance().unwrap());
            }
        }

        // Exponent
        if matches!(self.peek(), Some('e') | Some('E')) {
            s.push(self.advance().unwrap());
            if matches!(self.peek(), Some('+') | Some('-')) {
                s.push(self.advance().unwrap());
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                s.push(self.advance().unwrap());
            }
        }

        let base: f64 = s.parse().map_err(|_| self.parse_err(format!("bad number: {s}")))?;

        // Infix fraction: base/denom (no whitespace)
        if self.peek() == Some('/') && self.peek2().is_some_and(|c| c.is_ascii_digit()) {
            self.advance(); // '/'
            let mut denom = String::new();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                denom.push(self.advance().unwrap());
            }
            let d: f64 = denom.parse().unwrap();
            return Ok(base / d);
        }

        // Suffix (look ahead, only absorb if known)
        let suffix_start = self.pos;
        let mut suffix = String::new();
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            suffix.push(self.advance().unwrap());
            if matches!(suffix.as_str(), "k" | "M" | "m" | "u" | "pi" | "h" | "sr") {
                break;
            }
            if suffix.len() > 3 {
                // Not a known suffix, rewind
                self.pos = suffix_start;
                suffix.clear();
                break;
            }
        }

        let mult = match suffix.as_str() {
            "k"  => 1_000.0_f64,
            "M"  => 1_000_000.0,
            "m"  => 1e-3,
            "u"  => 1e-6,
            "pi" => std::f64::consts::PI,
            "h"  => std::f64::consts::TAU,
            "sr" => 44_100.0, // placeholder; real SR is runtime
            _ => {
                self.pos = suffix_start;
                1.0
            }
        };

        Ok(base * mult)
    }

    fn read_word(&mut self) -> String {
        let mut s = String::new();
        while self.peek().is_some_and(is_word_char) {
            s.push(self.advance().unwrap());
        }
        s
    }

    fn parse_word_or_adverb(&mut self) -> Vec<Op> {
        let mut word = self.read_word();
        if word.is_empty() { return vec![]; }

        // `<=` and `>=`: `=` is excluded from is_word_char (it triggers bind syntax),
        // so `<` and `>` stop there. Peek-ahead and consume the `=` if present.
        if (word == "<" || word == ">") && self.peek() == Some('=') {
            self.advance();
            word.push('=');
        }

        // Adverb suffixes
        if let Some(stem) = word.strip_suffix('/') {
            if !stem.is_empty() {
                return vec![Op::Adverb(Adverb::Reduce), Op::Word(Arc::from(stem))];
            }
        }
        if word.ends_with('\\') {
            let stem = word.trim_end_matches('\\');
            if !stem.is_empty() {
                return vec![Op::Adverb(Adverb::Scan), Op::Word(Arc::from(stem))];
            }
        }
        if let Some(stem) = word.strip_suffix('^') {
            if !stem.is_empty() {
                return vec![Op::Adverb(Adverb::Pairwise), Op::Word(Arc::from(stem))];
            }
        }

        vec![Op::Word(Arc::from(word.as_str()))]
    }
}

// Characters that can START a word (excludes digits which are parsed separately)
fn is_word_start(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '_' | '@' | '+' | '-' | '*' | '<' | '>' | '?' | '~' | '%' | '&' | '|' | '^' | '$')
}

// Characters that can ONLY appear in a word (not start one under the current rules,
// but we treat them uniformly via is_word_char)
fn is_word_only(c: char) -> bool {
    matches!(c, '/' | '\\')
}

fn is_word_char(c: char) -> bool {
    !matches!(c,
        ' ' | '\t' | '\n' | '\r'
        | ';' | '"'
        | '[' | ']' | '{' | '}' | '(' | ')'
        | '\'' | '`' | ',' | '='
        | '!' | ':' | '#'
    )
}

// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use stax_core::Error;
    use super::*;

    #[test]
    fn tokens_and_numbers() {
        let ops = parse("2 3 +").unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], Op::Lit(Value::Real(n)) if n == 2.0));
        assert!(matches!(ops[1], Op::Lit(Value::Real(n)) if n == 3.0));
        assert!(matches!(ops[2], Op::Word(_)));
    }

    #[test]
    fn fraction() {
        let ops = parse("5/4").unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Op::Lit(Value::Real(n)) if (n - 1.25).abs() < 1e-9));
    }

    #[test]
    fn string_literal() {
        let ops = parse("\"hello\"").unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Op::Lit(Value::Str(s)) if s.as_ref() == "hello"));
    }

    #[test]
    fn list_literal() {
        let ops = parse("[1 2 3]").unwrap();
        // ListMark, Lit(1), Lit(2), Lit(3), MakeList
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[0], Op::ListMark));
        assert!(matches!(ops[4], Op::MakeList { signal: false }));
    }

    #[test]
    fn signal_list() {
        let ops = parse("#[1 2]").unwrap();
        assert_eq!(ops.len(), 4); // ListMark, Lit(1), Lit(2), MakeList{signal:true}
        assert!(matches!(ops[3], Op::MakeList { signal: true }));
    }

    #[test]
    fn lambda() {
        let ops = parse("\\a b [a b +]").unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Op::MakeFun { params, body } if params.len() == 2 && body.len() == 3));
    }

    #[test]
    fn sym_and_quote() {
        let ops = parse("'foo `bar").unwrap();
        assert!(matches!(&ops[0], Op::Sym(s) if s.as_ref() == "foo"));
        assert!(matches!(&ops[1], Op::Quote(s) if s.as_ref() == "bar"));
    }

    #[test]
    fn adverb_suffix() {
        let ops = parse("+/ +\\ +^").unwrap();
        assert!(matches!(ops[0], Op::Adverb(Adverb::Reduce)));
        assert!(matches!(&ops[1], Op::Word(s) if s.as_ref() == "+"));
        assert!(matches!(ops[2], Op::Adverb(Adverb::Scan)));
        assert!(matches!(ops[4], Op::Adverb(Adverb::Pairwise)));
    }

    #[test]
    fn bind() {
        let ops = parse("42 = x").unwrap();
        assert!(matches!(&ops[1], Op::Bind(n) if n.as_ref() == "x"));
    }

    #[test]
    fn suffix_k() {
        let ops = parse("2k").unwrap();
        assert!(matches!(ops[0], Op::Lit(Value::Real(n)) if n == 2000.0));
    }

    #[test]
    fn comment() {
        let ops = parse("1 ; ignored\n2").unwrap();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn lte_gte_tokenise_correctly() {
        // Regression: `=` was excluded from is_word_char (to support `= bind` syntax),
        // so `<` and `>` stopped there and `=` was silently dropped.
        // Fixed by peeking after `<`/`>` in parse_word_or_adverb.
        let lte = parse("3 3 <=").unwrap();
        assert_eq!(lte.len(), 3);
        assert!(matches!(&lte[2], Op::Word(w) if w.as_ref() == "<="),
            "expected Word(\"<=\"), got {:?}", lte[2]);

        let gte = parse("5 3 >=").unwrap();
        assert_eq!(gte.len(), 3);
        assert!(matches!(&gte[2], Op::Word(w) if w.as_ref() == ">="),
            "expected Word(\">=\"), got {:?}", gte[2]);

        // Ensure plain `<` and `>` still parse correctly (no accidental merge).
        let lt = parse("3 5 <").unwrap();
        assert!(matches!(&lt[2], Op::Word(w) if w.as_ref() == "<"));

        let gt = parse("5 3 >").unwrap();
        assert!(matches!(&gt[2], Op::Word(w) if w.as_ref() == ">"));

        // Ensure `= bind` still works after `<` or `>`.
        let bind = parse("3 5 < = result").unwrap();
        assert!(matches!(&bind[3], Op::Bind(n) if n.as_ref() == "result"));
    }

    #[test]
    fn _placeholder_for_error_kind() {
        let _ = Error::Other("parser not fully implemented".into());
    }
}
