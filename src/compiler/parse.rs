use std::{
    mem,
    convert::TryFrom,
    collections::HashMap,
};

use crate::common::{
    span::{Span, Spanned},
    data::Data,
};

use crate::compiler::{lower::Lower, syntax::Syntax};

use crate::construct::{
    token::Token,
    tree::{AST, Base, Sugar, Lambda, Pattern, ArgPattern},
    symbol::SharedSymbol,
    module::{ThinModule, Module},
};

impl Lower for ThinModule<Vec<Spanned<Token>>> {
    type Out = Module<Spanned<AST>, usize>;

    /// Simple function that parses a token stream into an AST.
    /// Exposes the functionality of the `Parser`.
    fn lower(self) -> Result<Self::Out, Syntax> {
        let mut parser = Parser::new(self.repr);
        let ast = parser.body(Token::End)?;
        parser.consume(Token::End)?;
        return Ok(Module::new(
            Spanned::new(ast, Span::empty()),
            parser.symbols.len()
        ));
    }
}


/// We're using a Pratt parser, so this little enum
/// defines different precedence levels.
/// Each successive level is higher, so, for example,
/// `* > +`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    None = 0,
    Assign,
    Pair,
    Is,
    Lambda,

    Logic,

    AddSub,
    MulDiv,
    Pow,

    Compose, // TODO: where should this be, precedence-wise?
    Call,
    End,
}

impl Prec {
    /// Increments precedence level to cause the
    /// parser to associate infix operators to the left.
    /// For example, addition is left-associated:
    /// ```build
    /// Prec::Addition.left()
    /// ```
    /// `a + b + c` left-associated becomes `(a + b) + c`.
    /// By default, the parser accociates right.
    pub fn left(&self) -> Prec {
        if let Prec::End = self { panic!("Can not associate further left") }
        return unsafe { mem::transmute(self.clone() as u8 + 1) };
    }
}

/// Constructs an `AST` from a token stream.
/// Note that this struct should not be controlled manually,
/// use the `parse` function instead.
#[derive(Debug)]
pub struct Parser {
    tokens:  Vec<Spanned<Token>>,
    index:   usize,
    symbols: HashMap<String, SharedSymbol>,
}

impl Parser {
    /// Create a new `parser`.
    pub fn new(tokens: Vec<Spanned<Token>>) -> Parser {
        Parser { tokens, index: 0, symbols: HashMap::new() }
    }

    // Cookie Monster's Helper Functions:

    // NOTE: Maybe don't return bool?
    /// Consumes all seperator tokens, returning whether there were any.
    pub fn sep(&mut self) -> bool {
        if self.tokens[self.index].item != Token::Sep { false } else {
            while self.tokens[self.index].item == Token::Sep {
                self.index += 1;
            };
            true
        }
    }

    // TODO: merge with sep?
    /// Returns the next non-sep tokens,
    /// without advancing the parser.
    pub fn draw(&self) -> &Spanned<Token> {
        let mut offset = 0;

        while self.tokens[self.index + offset].item == Token::Sep {
            offset += 1;
        }

        return &self.tokens[self.index + offset];
    }

    /// Returns the current token then advances the parser.
    pub fn advance(&mut self) -> &Spanned<Token> {
        self.index += 1;
        &self.tokens[self.index - 1]
    }

    /// Returns the first token.
    pub fn current(&self) -> &Spanned<Token> {
        &self.tokens[self.index]
    }

    /// Returns the first non-Sep token.
    pub fn skip(&mut self) -> &Spanned<Token> {
        self.sep();
        self.current()
    }

    /// Throws an error if the next token is unexpected.
    /// I get one funny error message and this is it.
    /// The error message returned by this function will be changed frequently
    pub fn unexpected(&self) -> Syntax {
        let token = self.current();
        Syntax::error(
            &format!("Oopsie woopsie, what's {} doing here?", token.item),
            &token.span
        )
    }

    /// Consumes a specific token then advances the parser.
    /// Can be used to consume Sep tokens, which are normally skipped.
    pub fn consume(&mut self, token: Token) -> Result<&Spanned<Token>, Syntax> {
        self.index += 1;
        let current = &self.tokens[self.index - 1];
        if current.item != token {
            self.index -= 1;
            Err(Syntax::error(&format!("Expected {}, found {}", token, current.item), &current.span))
        } else {
            Ok(current)
        }
    }

    pub fn intern_symbol(&mut self, name: String) -> SharedSymbol {
        if let Some(symbol) = self.symbols.get(&name) {
            *symbol
        } else {
            let symbol = SharedSymbol(self.symbols.len());
            self.symbols.insert(name.to_string(), symbol);
            symbol
        }
    }

    // Core Pratt Parser:

    /// Looks at the current token and parses an infix expression
    pub fn rule_prefix(&mut self) -> Result<Spanned<AST>, Syntax> {
        match self.skip().item {
            Token::End         => Ok(Spanned::new(AST::Base(Base::Block(vec![])), Span::empty())),

            Token::Syntax      => self.syntax(),
            Token::Type        => todo!(),
            Token::OpenParen   => self.group(),
            Token::OpenBracket => self.block(),
            Token::Symbol      => self.symbol(),
            Token::Magic       => self.magic(),
            Token::Label       => self.label(),
            Token::Keyword     => self.keyword(),
            Token::Sub         => self.neg(),

              Token::Unit
            | Token::Number(_)
            | Token::String(_)
            | Token::Boolean(_) => self.literal(),

            Token::Sep => unreachable!(),
            _          => Err(Syntax::error("Expected an expression", &self.current().span)),
        }
    }

    /// Looks at the current token and parses the right side of any infix expressions.
    pub fn rule_infix(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        match self.skip().item {
            Token::Assign  => self.assign(left),
            Token::Lambda  => self.lambda(left),
            Token::Pair    => self.pair(left),
            Token::Compose => self.compose(left),

            Token::Add => self.binop(Prec::AddSub.left(), "add", left),
            Token::Sub => self.binop(Prec::AddSub.left(), "sub", left),
            Token::Mul => self.binop(Prec::MulDiv.left(), "mul", left),
            Token::Div => self.binop(Prec::MulDiv.left(), "div", left),
            Token::Rem => self.binop(Prec::MulDiv.left(), "rem", left),
            Token::Equal => self.binop(Prec::Logic.left(), "equal", left),
            Token::Pow => self.binop(Prec::Pow, "pow", left),

            Token::End => Err(self.unexpected()),
            Token::Sep => unreachable!(),
            _          => self.call(left),
        }
    }

    /// Looks at the current operator token and determines the precedence
    pub fn prec(&mut self) -> Result<Prec, Syntax> {
        let next = self.draw().item.clone();
        let current = self.current().item.clone();
        let sep = next != current;

        let prec = match next {
            // infix
            Token::Assign  => Prec::Assign,
            Token::Lambda  => Prec::Lambda,
            Token::Pair    => Prec::Pair,
            Token::Is      => Prec::Is,
            Token::Compose => Prec::Compose,

            Token::Equal => Prec::Logic,

              Token::Add
            | Token::Sub => Prec::AddSub,

              Token::Mul
            | Token::Div
            | Token::Rem => Prec::MulDiv,

            Token::Pow => Prec::Pow,

            // postfix
              Token::End
            | Token::CloseParen
            | Token::CloseBracket => Prec::End,

            // prefix
              Token::OpenParen
            | Token::OpenBracket
            | Token::Unit
            | Token::Syntax // TODO: Syntax and Type in the right place?
            | Token::Type
            | Token::Magic
            | Token::Symbol
            | Token::Keyword
            | Token::Label
            | Token::Number(_)
            | Token::String(_)
            | Token::Boolean(_) => Prec::Call,

            Token::Sep => unreachable!(),
        };

        if sep && prec == Prec::Call {
            Ok(Prec::End)
        } else {
            Ok(prec)
        }
    }

    // TODO: factor out? only group uses the skip sep flag...
    /// Uses some pratt parser magic to parse an expression.
    /// It's essentially a fold-left over tokens
    /// based on the precedence and content.
    /// Cool stuff.
    pub fn expression(&mut self, prec: Prec, skip_sep: bool) -> Result<Spanned<AST>, Syntax> {
        let mut left = self.rule_prefix()?;

        // TODO: switch to this?
        // loop {
        //     if skip_sep { self.sep(); }
        //     let p = self.prec()?;
        //     if p < prec || p == Prec::End { break; }
        //     left = self.rule_infix(left)?;
        // }

        while {
            if skip_sep { self.sep(); }
            let p = self.prec()?;
            p >= prec && p != Prec::End
        } {
            left = self.rule_infix(left)?;
        }

        return Ok(left);
    }

    // Rule Definitions:

    // Prefix:

    /// Constructs an AST for a symbol.
    pub fn symbol(&mut self) -> Result<Spanned<AST>, Syntax> {
        let symbol = self.consume(Token::Symbol)?.clone();
        let index = self.intern_symbol(symbol.span.contents());

        // nothing is more permanant, but
        // temporary workaround until prelude
        let name = symbol.span.contents();
        if name == "println" {
            let var = self.intern_symbol("#println".to_string());

            return Ok(Spanned::new(
                AST::Lambda(Lambda::new(
                    Spanned::new(Pattern::Symbol(var), Span::empty()),
                    Spanned::new(AST::Base(Base::ffi(
                        "println",
                        Spanned::new(AST::Base(Base::Symbol(var)), Span::empty()),
                    )), Span::empty()),
                )),
                symbol.span.clone(),
            ));
        }

        Ok(Spanned::new(
            AST::Base(Base::Symbol(index)),
            symbol.span.clone(),
        ))
    }

    /// Parses a keyword.
    /// Note that this is wrapped in a Arg Pattern node.
    pub fn keyword(&mut self) -> Result<Spanned<AST>, Syntax> {
        let span = if let Spanned { item: Token::Keyword, span } = self.advance() {
            span.clone()
        } else {
            unreachable!("Expected a keyword");
        };

        // TODO: create a new symbol
        let wrapped = AST::Sugar(Sugar::ArgPattern(ArgPattern::Keyword(
            self.intern_symbol(span.contents()[1..].to_string())
        )));

        Ok(Spanned::new(wrapped, span.clone()))
    }

    /// Constructs the AST for a literal, such as a number or string.
    pub fn literal(&mut self) -> Result<Spanned<AST>, Syntax> {
        let Spanned { item: token, span } = self.advance();

        let leaf = match token {
            Token::Unit       => AST::Base(Base::Data(Data::Unit)),
            Token::Number(n)  => AST::Base(Base::Data(n.clone())),
            Token::String(s)  => AST::Base(Base::Data(s.clone())),
            Token::Boolean(b) => AST::Base(Base::Data(b.clone())),
            unexpected => return Err(Syntax::error(
                &format!("Expected a literal, found {}", unexpected),
                &span
            )),
        };

        Ok(Spanned::new(leaf, span.clone()))
    }

    /// Constructs the ast for a group,
    /// i.e. an expression between parenthesis.
    pub fn group(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start = self.consume(Token::OpenParen)?.span.clone();
        let ast   = self.expression(Prec::None.left(), true)?;
        let end   = self.consume(Token::CloseParen)?.span.clone();
        Ok(Spanned::new(
            AST::Sugar(Sugar::group(ast)),
            Span::combine(&start, &end),
        ))
    }

    /// Parses the body of a block.
    /// A block is one or more expressions, separated by separators.
    /// This is more of a helper function, as it serves as both the
    /// parser entrypoint while still being recursively nestable.
    pub fn body(&mut self, end: Token) -> Result<AST, Syntax> {
        let mut expressions = vec![];

        while self.skip().item != end {
            let ast = self.expression(Prec::None, false)?;
            expressions.push(ast);
            if let Err(_) = self.consume(Token::Sep) {
                break;
            }
        }

        return Ok(AST::Base(Base::Block(expressions)));
    }

    /// Parse a block as an expression,
    /// Building the appropriate `AST`.
    /// Just a body between curlies.
    pub fn block(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start   = self.consume(Token::OpenBracket)?.span.clone();
        let mut ast = self.body(Token::CloseBracket)?;
        let end     = self.consume(Token::CloseBracket)?.span.clone();

        // construct a record if applicable
        // match ast {
        //     AST::Block(ref b) if b.len() == 0 => match b[0].item {
        //         AST::Tuple(ref t) => {
        //             ast = AST::Record(t.clone())
        //         },
        //         _ => (),
        //     },
        //     _ => (),
        // }

        return Ok(Spanned::new(ast, Span::combine(&start, &end)));
    }

    // TODO: unwrap from outside in to prevent nesting
    /// Parse a macro definition.
    /// `syntax`, followed by a pattern, followed by a `block`
    pub fn syntax(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start = self.consume(Token::Syntax)?.span.clone();
        let mut after = self.expression(Prec::Call, false)?;

        let mut form = match after.item {
            AST::Sugar(Sugar::Form(p)) => p,
            _ => return Err(Syntax::error(
                "Expected a pattern and a block after 'syntax'",
                &after.span,
            )),
        };

        let last = form.pop().unwrap();
        after.span = Spanned::build(&form);
        after.item = AST::Sugar(Sugar::Form(form));
        let block = match (last.item, last.span) {
            (b @ AST::Base(Base::Block(_)), s) => Spanned::new(b, s),
            _ => return Err(Syntax::error(
                "Expected a block after the syntax pattern",
                &after.span,
            )),
        };

        let arg_pat = Parser::arg_pat(after)?;
        let span = Span::join(vec![
            start,
            arg_pat.span.clone(),
            block.span.clone()
        ]);

        return Ok(Spanned::new(AST::Sugar(Sugar::syntax(arg_pat, block)), span));
    }

    // pub fn type_(&mut self) -> Result<Spanned<AST>, Syntax> {
    //     let start = self.consume(Token::Type)?.span.clone();
    //     let label = self.consume(Token::Label)?.span.clone();

    //     return Ok(Spanned::new(AST::type_(label), Span::combine(start, label)))
    // }

    /// Parse an `extern` statement.
    /// used for compiler magic and other glue.
    /// takes the form:
    /// ```ignore
    /// magic "FFI String Name" data_to_pass_out
    /// ```
    /// and evaluates to the value returned by the ffi function.
    pub fn magic(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start = self.consume(Token::Magic)?.span.clone();

        let Spanned { item: token, span } = self.advance();
        let name = match token {
            Token::String(Data::String(s))  => s.clone(),
            unexpected => return Err(Syntax::error(
                &format!("Expected a string, found {}", unexpected),
                &span
            )),
        };

        let ast = self.expression(Prec::End, false)?;
        let end = ast.span.clone();

        return Ok(Spanned::new(
            AST::Base(Base::ffi(&name, ast)),
            Span::combine(&start, &end),
        ));
    }

    /// Parse a label.
    /// A label takes the form of `<Label> <expression>`
    pub fn label(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start = self.consume(Token::Label)?.span.clone();
        let ast = self.expression(Prec::End, false)?;
        let end = ast.span.clone();
        return Ok(Spanned::new(
            AST::Base(Base::label(Spanned::new(
                self.intern_symbol(start.contents()),
                start,
            ), ast)),
            Span::combine(&start, &end),
        ));
    }

    pub fn neg(&mut self) -> Result<Spanned<AST>, Syntax> {
        let start = self.consume(Token::Sub)?.span.clone();
        let ast = self.expression(Prec::End, false)?;
        let end = ast.span.clone();

        return Ok(
            Spanned::new(AST::Base(Base::ffi("neg", ast)),
            Span::combine(&start, &end))
        );
    }

    // Infix:

    /// Parses an assignment, associates right.
    pub fn assign(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        let left_span = left.span.clone();
        let pattern = left.map(Pattern::try_from)
            .map_err(|e| Syntax::error(&e, &left_span))?;

        self.consume(Token::Assign)?;
        let expression = self.expression(Prec::Assign, false)?;
        let combined   = Span::combine(&pattern.span, &expression.span);
        Ok(Spanned::new(AST::Base(Base::assign(pattern, expression)), combined))
    }

    /// Parses a lambda definition, associates right.
    pub fn lambda(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        let left_span = left.span.clone();
        let pattern = left.map(Pattern::try_from)
            .map_err(|e| Syntax::error(&e, &left_span))?;

        self.consume(Token::Lambda)?;
        let expression = self.expression(Prec::Lambda, false)?;
        let combined   = Span::combine(&pattern.span, &expression.span);
        Ok(Spanned::new(AST::Lambda(Lambda::new(pattern, expression)), combined))
    }

    // TODO: trailing comma must be grouped
    /// Parses a pair operator, i.e. the comma used to build tuples: `a, b, c`.
    pub fn pair(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        let left_span = left.span.clone();
        self.consume(Token::Pair)?;

        let mut tuple = match left.item {
            AST::Base(Base::Tuple(t)) => t,
            _ => vec![left],
        };

        let index = self.index;
        let span = if let Ok(item) = self.expression(Prec::Pair.left(), false) {
            let combined = Span::combine(&left_span, &item.span);
            tuple.push(item);
            combined
        } else {
            // restore parser to location right after trailing comma
            self.index = index;
            left_span
        };

        return Ok(Spanned::new(AST::Base(Base::Tuple(tuple)), span));
    }

    /// Parses a function comp, i.e. `a . b`
    pub fn compose(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        self.consume(Token::Compose)?;
        let right = self.expression(Prec::Compose.left(), false)?;
        let combined = Span::combine(&left.span, &right.span);
        return Ok(Spanned::new(AST::Sugar(Sugar::comp(left, right)), combined));
    }

    // TODO: names must be full qualified paths.

    /// Parses a left-associative binary operator.
    fn binop(
        &mut self,
        // op: Token,
        prec: Prec,
        name: &str,
        left: Spanned<AST>
    ) -> Result<Spanned<AST>, Syntax> {
        // self.consume(op)?;
        self.advance();
        let right = self.expression(prec, false)?;
        let combined = Span::combine(&left.span, &right.span);

        let arguments = Spanned::new(AST::Base(Base::Tuple(vec![left, right])), combined.clone());
        return Ok(Spanned::new(AST::Base(Base::ffi(name, arguments)), combined));
    }

    /// Parses a function call.
    /// Function calls are a bit magical,
    /// because they're just a series of expressions.
    /// There's a bit of magic involved -
    /// we interpret anything that isn't an operator as a function call operator.
    /// Then pull a fast one and not parse it like an operator at all.
    pub fn call(&mut self, left: Spanned<AST>) -> Result<Spanned<AST>, Syntax> {
        let argument = self.expression(Prec::Call.left(), false)?;
        let combined = Span::combine(&left.span, &argument.span);

        let mut form = match left.item {
            AST::Sugar(Sugar::Form(f)) => f,
            _ => vec![left],
        };

        form.push(argument);
        return Ok(Spanned::new(AST::Sugar(Sugar::Form(form)), combined));
    }
}

#[cfg(test)]
mod test {
    use crate::common::source::Source;
    use super::*;

    #[test]
    pub fn empty() {
        let source = Source::source("");
        let ast = ThinModule::thin(source).lower().unwrap().lower();
        let result = Module::new(Spanned::new(AST::Base(Base::Block(vec![])), Span::empty()), 0);
        assert_eq!(ast, Ok(result));
    }

    // TODO: fuzzing
}
