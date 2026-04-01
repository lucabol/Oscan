use crate::error::CompileError;
use crate::token::{Span, Token, TokenKind};

#[derive(Debug, Clone, Copy)]
enum LexerMode {
    StringContinuation,
    Interpolation { brace_depth: usize },
}

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    modes: Vec<LexerMode>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            modes: Vec::new(),
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, CompileError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_ahead(&self, offset: usize) -> Option<char> {
        self.source.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn span(&self) -> Span {
        Span::new(self.line, self.column)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            self.advance();
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), CompileError> {
        let start_span = self.span();
        // Already consumed '/*'
        loop {
            match self.advance() {
                Some('*') => {
                    if self.peek() == Some('/') {
                        self.advance();
                        return Ok(());
                    }
                }
                Some(_) => {}
                None => {
                    return Err(CompileError::new(start_span, "unterminated block comment"));
                }
            }
        }
    }

    fn read_string_escape(&mut self, s: &mut String, esc_span: Span) -> Result<(), CompileError> {
        match self.advance() {
            Some('n') => s.push('\n'),
            Some('t') => s.push('\t'),
            Some('r') => s.push('\r'),
            Some('\\') => s.push('\\'),
            Some('"') => s.push('"'),
            Some('0') => s.push('\0'),
            Some(c) => {
                return Err(CompileError::new(
                    esc_span,
                    format!("invalid escape sequence '\\{c}'"),
                ));
            }
            None => {
                return Err(CompileError::new(esc_span, "unterminated string literal"));
            }
        }
        Ok(())
    }

    fn read_string_segment(
        &mut self,
        start_span: Span,
        is_initial: bool,
    ) -> Result<Token, CompileError> {
        let mut s = String::new();
        loop {
            match self.advance() {
                Some('"') => {
                    if is_initial {
                        return Ok(Token::new(TokenKind::StringLit(s), start_span));
                    }
                    let mode = self.modes.pop();
                    debug_assert!(matches!(mode, Some(LexerMode::StringContinuation)));
                    return Ok(Token::new(TokenKind::InterpStringEnd(s), start_span));
                }
                Some('\\') => {
                    let esc_span = self.span();
                    self.read_string_escape(&mut s, esc_span)?;
                }
                Some('{') => {
                    if self.peek() == Some('{') {
                        self.advance();
                        s.push('{');
                        continue;
                    }
                    if is_initial {
                        self.modes.push(LexerMode::StringContinuation);
                    }
                    self.modes.push(LexerMode::Interpolation { brace_depth: 0 });
                    let kind = if is_initial {
                        TokenKind::InterpStringStart(s)
                    } else {
                        TokenKind::InterpStringMiddle(s)
                    };
                    return Ok(Token::new(kind, start_span));
                }
                Some('}') => {
                    if self.peek() == Some('}') {
                        self.advance();
                        s.push('}');
                    } else {
                        return Err(CompileError::new(
                            start_span,
                            "single '}' is not allowed in string literals; use '}}' for a literal brace",
                        ));
                    }
                }
                Some('\n') | None => {
                    return Err(CompileError::new(start_span, "unterminated string literal"));
                }
                Some(c) => s.push(c),
            }
        }
    }

    fn read_number(&mut self) -> Result<Token, CompileError> {
        let start_span = self.span();
        let mut num_str = String::new();
        let first_digit = self.advance().unwrap();
        num_str.push(first_digit);

        // Check for leading zeros (e.g. 007 is invalid, but 0 or 0.5 is fine)
        if first_digit == '0' {
            if let Some(next) = self.peek() {
                if next.is_ascii_digit() {
                    return Err(CompileError::new(
                        start_span,
                        "leading zeros are not allowed in integer literals",
                    ));
                }
            }
        }

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        // Check for float
        if self.peek() == Some('.') && self.peek_ahead(1).is_some_and(|c| c.is_ascii_digit()) {
            num_str.push('.');
            self.advance(); // consume '.'
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    num_str.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
            let value: f64 = num_str.parse().map_err(|_| {
                CompileError::new(start_span, format!("invalid float literal '{num_str}'"))
            })?;
            Ok(Token::new(TokenKind::FloatLit(value), start_span))
        } else {
            let value: i64 = num_str.parse().map_err(|_| {
                CompileError::new(start_span, format!("invalid integer literal '{num_str}'"))
            })?;
            Ok(Token::new(TokenKind::IntLit(value), start_span))
        }
    }

    fn read_identifier_or_keyword(&mut self) -> Token {
        let start_span = self.span();
        let mut ident = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ident.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        // Check for `fn!` — if we just read "fn" and the next char is '!'
        if ident == "fn" && self.peek() == Some('!') {
            self.advance(); // consume '!'
            return Token::new(TokenKind::FnBang, start_span);
        }

        let kind = match ident.as_str() {
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "match" => TokenKind::Match,
            "return" => TokenKind::Return,
            "try" => TokenKind::Try,
            "extern" => TokenKind::Extern,
            "as" => TokenKind::As,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "use" => TokenKind::Use,
            "_" => TokenKind::Underscore,
            _ => TokenKind::Ident(ident),
        };
        Token::new(kind, start_span)
    }

    fn lex_normal_token(&mut self) -> Result<Token, CompileError> {
        loop {
            self.skip_whitespace();
            let span = self.span();

            match self.peek() {
                None => return Ok(Token::new(TokenKind::Eof, span)),
                Some('/') => {
                    if self.peek_ahead(1) == Some('/') {
                        self.advance();
                        self.advance();
                        self.skip_line_comment();
                        continue;
                    } else if self.peek_ahead(1) == Some('*') {
                        self.advance();
                        self.advance();
                        self.skip_block_comment()?;
                        continue;
                    } else if self.peek_ahead(1) == Some('=') {
                        self.advance();
                        self.advance();
                        return Ok(Token::new(TokenKind::SlashEq, span));
                    } else {
                        self.advance();
                        return Ok(Token::new(TokenKind::Slash, span));
                    }
                }
                Some('"') => {
                    self.advance();
                    return self.read_string_segment(span, true);
                }
                Some(ch) if ch.is_ascii_digit() => {
                    return self.read_number();
                }
                Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {
                    return Ok(self.read_identifier_or_keyword());
                }
                Some('+') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::PlusEq, span));
                    }
                    return Ok(Token::new(TokenKind::Plus, span));
                }
                Some('-') => {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        return Ok(Token::new(TokenKind::Arrow, span));
                    }
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::MinusEq, span));
                    }
                    return Ok(Token::new(TokenKind::Minus, span));
                }
                Some('*') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::StarEq, span));
                    }
                    return Ok(Token::new(TokenKind::Star, span));
                }
                Some('%') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::PercentEq, span));
                    }
                    return Ok(Token::new(TokenKind::Percent, span));
                }
                Some('=') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::EqEq, span));
                    }
                    if self.peek() == Some('>') {
                        self.advance();
                        return Ok(Token::new(TokenKind::FatArrow, span));
                    }
                    return Ok(Token::new(TokenKind::Eq, span));
                }
                Some('!') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::BangEq, span));
                    }
                    return Err(CompileError::new(span, "unexpected character '!'"));
                }
                Some('<') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::LtEq, span));
                    }
                    return Ok(Token::new(TokenKind::Lt, span));
                }
                Some('>') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        return Ok(Token::new(TokenKind::GtEq, span));
                    }
                    return Ok(Token::new(TokenKind::Gt, span));
                }
                Some('{') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::LBrace, span));
                }
                Some('}') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::RBrace, span));
                }
                Some('(') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::LParen, span));
                }
                Some(')') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::RParen, span));
                }
                Some('[') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::LBracket, span));
                }
                Some(']') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::RBracket, span));
                }
                Some(',') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::Comma, span));
                }
                Some(':') => {
                    self.advance();
                    if self.peek() == Some(':') {
                        self.advance();
                        return Ok(Token::new(TokenKind::ColonColon, span));
                    }
                    return Ok(Token::new(TokenKind::Colon, span));
                }
                Some(';') => {
                    self.advance();
                    return Ok(Token::new(TokenKind::Semicolon, span));
                }
                Some('.') => {
                    self.advance();
                    if self.peek() == Some('.') {
                        self.advance();
                        return Ok(Token::new(TokenKind::DotDot, span));
                    }
                    return Ok(Token::new(TokenKind::Dot, span));
                }
                Some(ch) => {
                    self.advance();
                    return Err(CompileError::new(
                        span,
                        format!("unexpected character '{ch}'"),
                    ));
                }
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, CompileError> {
        match self.modes.last().copied() {
            Some(LexerMode::StringContinuation) => {
                let span = self.span();
                self.read_string_segment(span, false)
            }
            Some(LexerMode::Interpolation { brace_depth }) => {
                loop {
                    self.skip_whitespace();
                    if self.peek() == Some('/') && self.peek_ahead(1) == Some('/') {
                        self.advance();
                        self.advance();
                        self.skip_line_comment();
                        continue;
                    }
                    if self.peek() == Some('/') && self.peek_ahead(1) == Some('*') {
                        self.advance();
                        self.advance();
                        self.skip_block_comment()?;
                        continue;
                    }
                    break;
                }
                if brace_depth == 0 && self.peek() == Some('}') {
                    self.advance();
                    self.modes.pop();
                    self.next_token()
                } else if self.peek() == Some('{') {
                    let span = self.span();
                    self.advance();
                    if let Some(LexerMode::Interpolation { brace_depth }) = self.modes.last_mut() {
                        *brace_depth += 1;
                    }
                    Ok(Token::new(TokenKind::LBrace, span))
                } else if self.peek() == Some('}') {
                    let span = self.span();
                    self.advance();
                    if let Some(LexerMode::Interpolation { brace_depth }) = self.modes.last_mut() {
                        *brace_depth -= 1;
                    }
                    Ok(Token::new(TokenKind::RBrace, span))
                } else {
                    self.lex_normal_token()
                }
            }
            None => self.lex_normal_token(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<TokenKind> {
        let mut lexer = Lexer::new(input);
        lexer
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn lex_err(input: &str) -> String {
        let mut lexer = Lexer::new(input);
        lexer.tokenize().unwrap_err().message
    }

    #[test]
    fn test_keywords() {
        let tokens = lex("fn let mut struct enum if else while for in match return try extern as and or not true false");
        assert_eq!(tokens[0], TokenKind::Fn);
        assert_eq!(tokens[1], TokenKind::Let);
        assert_eq!(tokens[2], TokenKind::Mut);
        assert_eq!(tokens[3], TokenKind::Struct);
        assert_eq!(tokens[4], TokenKind::Enum);
        assert_eq!(tokens[5], TokenKind::If);
        assert_eq!(tokens[6], TokenKind::Else);
        assert_eq!(tokens[7], TokenKind::While);
        assert_eq!(tokens[8], TokenKind::For);
        assert_eq!(tokens[9], TokenKind::In);
        assert_eq!(tokens[10], TokenKind::Match);
        assert_eq!(tokens[11], TokenKind::Return);
        assert_eq!(tokens[12], TokenKind::Try);
        assert_eq!(tokens[13], TokenKind::Extern);
        assert_eq!(tokens[14], TokenKind::As);
        assert_eq!(tokens[15], TokenKind::And);
        assert_eq!(tokens[16], TokenKind::Or);
        assert_eq!(tokens[17], TokenKind::Not);
        assert_eq!(tokens[18], TokenKind::True);
        assert_eq!(tokens[19], TokenKind::False);
        assert_eq!(tokens[20], TokenKind::Eof);
    }

    #[test]
    fn test_fn_bang() {
        let tokens = lex("fn! main() { }");
        assert_eq!(tokens[0], TokenKind::FnBang);
        assert_eq!(tokens[1], TokenKind::Ident("main".into()));
    }

    #[test]
    fn test_identifiers() {
        let tokens = lex("foo _bar Point my_var123");
        assert_eq!(tokens[0], TokenKind::Ident("foo".into()));
        assert_eq!(tokens[1], TokenKind::Ident("_bar".into()));
        assert_eq!(tokens[2], TokenKind::Ident("Point".into()));
        assert_eq!(tokens[3], TokenKind::Ident("my_var123".into()));
    }

    #[test]
    fn test_underscore_wildcard() {
        let tokens = lex("_ => 42");
        assert_eq!(tokens[0], TokenKind::Underscore);
        assert_eq!(tokens[1], TokenKind::FatArrow);
        assert_eq!(tokens[2], TokenKind::IntLit(42));
    }

    #[test]
    fn test_integer_literals() {
        let tokens = lex("0 42 12345");
        assert_eq!(tokens[0], TokenKind::IntLit(0));
        assert_eq!(tokens[1], TokenKind::IntLit(42));
        assert_eq!(tokens[2], TokenKind::IntLit(12345));
    }

    #[test]
    fn test_float_literals() {
        let tokens = lex("3.14 0.5 100.0");
        assert_eq!(tokens[0], TokenKind::FloatLit(3.14));
        assert_eq!(tokens[1], TokenKind::FloatLit(0.5));
        assert_eq!(tokens[2], TokenKind::FloatLit(100.0));
    }

    #[test]
    fn test_string_literals() {
        let tokens = lex(r#""hello" "world\n" "tab\there" "esc\\""#);
        assert_eq!(tokens[0], TokenKind::StringLit("hello".into()));
        assert_eq!(tokens[1], TokenKind::StringLit("world\n".into()));
        assert_eq!(tokens[2], TokenKind::StringLit("tab\there".into()));
        assert_eq!(tokens[3], TokenKind::StringLit("esc\\".into()));
    }

    #[test]
    fn test_interpolated_string_tokens() {
        let tokens = lex(r#""hello {name}!""#);
        assert_eq!(tokens[0], TokenKind::InterpStringStart("hello ".into()));
        assert_eq!(tokens[1], TokenKind::Ident("name".into()));
        assert_eq!(tokens[2], TokenKind::InterpStringEnd("!".into()));
    }

    #[test]
    fn test_interpolated_string_literal_braces() {
        let tokens = lex(r#""{{value}} {n}""#);
        assert_eq!(tokens[0], TokenKind::InterpStringStart("{value} ".into()));
        assert_eq!(tokens[1], TokenKind::Ident("n".into()));
        assert_eq!(tokens[2], TokenKind::InterpStringEnd("".into()));
    }

    #[test]
    fn test_interpolated_string_rejects_single_closing_brace() {
        let msg = lex_err(r#""oops }""#);
        assert!(msg.contains("single '}' is not allowed"), "got: {msg}");
    }

    #[test]
    fn test_operators() {
        let tokens = lex("+ - * / % == != < > <= >= = => -> ::");
        assert_eq!(tokens[0], TokenKind::Plus);
        assert_eq!(tokens[1], TokenKind::Minus);
        assert_eq!(tokens[2], TokenKind::Star);
        assert_eq!(tokens[3], TokenKind::Slash);
        assert_eq!(tokens[4], TokenKind::Percent);
        assert_eq!(tokens[5], TokenKind::EqEq);
        assert_eq!(tokens[6], TokenKind::BangEq);
        assert_eq!(tokens[7], TokenKind::Lt);
        assert_eq!(tokens[8], TokenKind::Gt);
        assert_eq!(tokens[9], TokenKind::LtEq);
        assert_eq!(tokens[10], TokenKind::GtEq);
        assert_eq!(tokens[11], TokenKind::Eq);
        assert_eq!(tokens[12], TokenKind::FatArrow);
        assert_eq!(tokens[13], TokenKind::Arrow);
        assert_eq!(tokens[14], TokenKind::ColonColon);
    }

    #[test]
    fn test_punctuation() {
        let tokens = lex("{ } ( ) [ ] , : ; . ..");
        assert_eq!(tokens[0], TokenKind::LBrace);
        assert_eq!(tokens[1], TokenKind::RBrace);
        assert_eq!(tokens[2], TokenKind::LParen);
        assert_eq!(tokens[3], TokenKind::RParen);
        assert_eq!(tokens[4], TokenKind::LBracket);
        assert_eq!(tokens[5], TokenKind::RBracket);
        assert_eq!(tokens[6], TokenKind::Comma);
        assert_eq!(tokens[7], TokenKind::Colon);
        assert_eq!(tokens[8], TokenKind::Semicolon);
        assert_eq!(tokens[9], TokenKind::Dot);
        assert_eq!(tokens[10], TokenKind::DotDot);
    }

    #[test]
    fn test_line_comment() {
        let tokens = lex("42 // this is a comment\n43");
        assert_eq!(tokens[0], TokenKind::IntLit(42));
        assert_eq!(tokens[1], TokenKind::IntLit(43));
        assert_eq!(tokens[2], TokenKind::Eof);
    }

    #[test]
    fn test_block_comment() {
        let tokens = lex("42 /* block */ 43");
        assert_eq!(tokens[0], TokenKind::IntLit(42));
        assert_eq!(tokens[1], TokenKind::IntLit(43));
        assert_eq!(tokens[2], TokenKind::Eof);
    }

    #[test]
    fn test_unterminated_string() {
        let msg = lex_err("\"hello");
        assert!(msg.contains("unterminated"), "got: {msg}");
    }

    #[test]
    fn test_unterminated_block_comment() {
        let msg = lex_err("/* never closed");
        assert!(msg.contains("unterminated"), "got: {msg}");
    }

    #[test]
    fn test_leading_zeros_error() {
        let msg = lex_err("007");
        assert!(msg.contains("leading zeros"), "got: {msg}");
    }

    #[test]
    fn test_span_tracking() {
        let mut lexer = Lexer::new("fn\n  main");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0].span, Span::new(1, 1));
        assert_eq!(tokens[1].span, Span::new(2, 3));
    }

    #[test]
    fn test_full_fn_decl() {
        let mut lexer = Lexer::new("fn! main() { let x: i32 = 42; }");
        let tokens = lexer.tokenize().unwrap();
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(kinds[0], &TokenKind::FnBang);
        assert_eq!(kinds[1], &TokenKind::Ident("main".into()));
        assert_eq!(kinds[2], &TokenKind::LParen);
        assert_eq!(kinds[3], &TokenKind::RParen);
        assert_eq!(kinds[4], &TokenKind::LBrace);
        assert_eq!(kinds[5], &TokenKind::Let);
        assert_eq!(kinds[6], &TokenKind::Ident("x".into()));
        assert_eq!(kinds[7], &TokenKind::Colon);
        assert_eq!(kinds[8], &TokenKind::Ident("i32".into()));
        assert_eq!(kinds[9], &TokenKind::Eq);
        assert_eq!(kinds[10], &TokenKind::IntLit(42));
        assert_eq!(kinds[11], &TokenKind::Semicolon);
        assert_eq!(kinds[12], &TokenKind::RBrace);
    }
}
