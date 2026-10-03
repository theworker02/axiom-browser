//! WHATWG HTML tokenizer (§13.2.5), incremental.
//!
//! Input arrives through [`Tokenizer::feed`] and [`Tokenizer::finish`]; tokens come out
//! of [`Tokenizer::next_token`], which returns `None` when it needs more input (it never
//! guesses about a construct that is cut off at the end of the input received so far).
//! Newlines are normalized on input (CR LF and lone CR become LF). Parse errors are not
//! reported. The tree builder drives the content-model switches through
//! [`Tokenizer::set_state`] and [`Tokenizer::set_allow_cdata`].

use std::collections::VecDeque;

use crate::entities::{ENTITIES, LONGEST_NAME};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tag {
    pub name: String,
    pub attrs: Vec<Attribute>,
    pub self_closing: bool,
}

impl Tag {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub force_quirks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Doctype(Doctype),
    StartTag(Tag),
    EndTag(Tag),
    Comment(String),
    /// `<?target data>` (§13.2.5.72–76).
    ProcessingInstruction {
        target: String,
        data: String,
    },
    /// A run of character data (adjacent characters are merged).
    Character(String),
    Eof,
}

/// Tokenizer states (§13.2.5.1–80).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
    TagOpen,
    EndTagOpen,
    TagName,
    RcdataLessThanSign,
    RcdataEndTagOpen,
    RcdataEndTagName,
    RawtextLessThanSign,
    RawtextEndTagOpen,
    RawtextEndTagName,
    ScriptDataLessThanSign,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThanSign,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThanSign,
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThanSign,
    CommentLessThanSignBang,
    CommentLessThanSignBangDash,
    CommentLessThanSignBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicIdentifier,
    DoctypePublicIdentifierDoubleQuoted,
    DoctypePublicIdentifierSingleQuoted,
    AfterDoctypePublicIdentifier,
    BetweenDoctypePublicAndSystemIdentifiers,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemIdentifier,
    DoctypeSystemIdentifierDoubleQuoted,
    DoctypeSystemIdentifierSingleQuoted,
    AfterDoctypeSystemIdentifier,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexadecimalCharacterReferenceStart,
    DecimalCharacterReferenceStart,
    HexadecimalCharacterReference,
    DecimalCharacterReference,
    NumericCharacterReferenceEnd,
    ProcessingInstructionOpen,
    ProcessingInstructionTarget,
    AfterProcessingInstructionTarget,
    ProcessingInstructionData,
    ProcessingInstructionQuestionable,
}

enum Input {
    Char(char),
    Eof,
    NeedData,
}

/// Consumed input is dropped once this many bytes have been tokenized.
const COMPACT_AFTER: usize = 64 * 1024;
const REPLACEMENT: char = '\u{FFFD}';

#[derive(Debug)]
pub struct Tokenizer {
    input: String,
    pos: usize,
    consumed_total: usize,
    eof: bool,
    /// The last chunk ended with CR (already turned into LF): drop a leading LF next.
    pending_cr: bool,
    state: State,
    return_state: State,
    tokens: VecDeque<Token>,
    chars: String,
    tag: Tag,
    tag_is_end: bool,
    attr: Option<Attribute>,
    comment: String,
    pi_data: String,
    doctype: Doctype,
    temp: String,
    char_ref_code: u32,
    last_start_tag: Option<String>,
    allow_cdata: bool,
    emitted_eof: bool,
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokenize a complete input from the data state (without the final [`Token::Eof`]).
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut t = Tokenizer::new();
    t.feed(input);
    t.finish();
    let mut out = Vec::new();
    while let Some(tok) = t.next_token() {
        if tok != Token::Eof {
            out.push(tok);
        }
    }
    out
}

impl Tokenizer {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            pos: 0,
            consumed_total: 0,
            eof: false,
            pending_cr: false,
            state: State::Data,
            return_state: State::Data,
            tokens: VecDeque::new(),
            chars: String::new(),
            tag: Tag::default(),
            tag_is_end: false,
            attr: None,
            comment: String::new(),
            pi_data: String::new(),
            doctype: Doctype::default(),
            temp: String::new(),
            char_ref_code: 0,
            last_start_tag: None,
            allow_cdata: false,
            emitted_eof: false,
        }
    }

    /// Append input (newlines are normalized here).
    pub fn feed(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut s = text;
        if std::mem::take(&mut self.pending_cr) {
            s = s.strip_prefix('\n').unwrap_or(s);
        }
        if s.contains('\r') {
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' {
                    self.input.push('\n');
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                } else {
                    self.input.push(c);
                }
            }
            self.pending_cr = s.ends_with('\r');
        } else {
            self.input.push_str(s);
        }
    }

    /// No more input will arrive.
    pub fn finish(&mut self) {
        self.eof = true;
    }

    pub fn is_finished(&self) -> bool {
        self.eof
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Switch the content model (RCDATA, RAWTEXT, script data, PLAINTEXT), as the tree
    /// builder does after inserting such an element.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// The tag name an end tag must have to be "appropriate" (html5lib `lastStartTag`).
    pub fn set_last_start_tag(&mut self, name: Option<&str>) {
        self.last_start_tag = name.map(str::to_string);
    }

    /// Whether `<![CDATA[` opens a CDATA section (the adjusted current node is foreign).
    pub fn set_allow_cdata(&mut self, allow: bool) {
        self.allow_cdata = allow;
    }

    /// Input received but not tokenized yet (after newline normalization).
    pub fn unconsumed(&self) -> &str {
        &self.input[self.pos..]
    }

    /// Bytes of (normalized) input tokenized so far.
    pub fn consumed_bytes(&self) -> usize {
        self.consumed_total + self.pos
    }

    /// The next token, or `None` if more input is needed (or [`Token::Eof`] was returned).
    pub fn next_token(&mut self) -> Option<Token> {
        loop {
            if let Some(t) = self.tokens.pop_front() {
                return Some(t);
            }
            if self.emitted_eof {
                return None;
            }
            if !self.step() {
                self.compact();
                if !self.chars.is_empty() {
                    return Some(Token::Character(std::mem::take(&mut self.chars)));
                }
                return None;
            }
        }
    }

    fn compact(&mut self) {
        if self.pos >= COMPACT_AFTER {
            self.input.drain(..self.pos);
            self.consumed_total += self.pos;
            self.pos = 0;
        }
    }

    // ----- input -----

    fn getc(&mut self) -> Input {
        match self.input[self.pos..].chars().next() {
            Some(c) => {
                self.pos += c.len_utf8();
                Input::Char(c)
            }
            None if self.eof => Input::Eof,
            None => Input::NeedData,
        }
    }

    /// Un-consume `input` ("reconsume in the … state").
    fn unget(&mut self, input: &Input) {
        if let Input::Char(c) = input {
            self.pos -= c.len_utf8();
        }
    }

    fn reconsume(&mut self, input: &Input, state: State) {
        self.unget(input);
        self.state = state;
    }

    /// Whether the unconsumed input starts with `word` (optionally ASCII
    /// case-insensitively). `None`: not enough input to tell yet.
    fn lookahead(&self, word: &str, ignore_case: bool) -> Option<bool> {
        let rest = &self.input.as_bytes()[self.pos..];
        let n = word.len().min(rest.len());
        let prefix_matches = if ignore_case {
            rest[..n].eq_ignore_ascii_case(&word.as_bytes()[..n])
        } else {
            rest[..n] == word.as_bytes()[..n]
        };
        if !prefix_matches {
            return Some(false);
        }
        if rest.len() >= word.len() {
            return Some(true);
        }
        if self.eof {
            Some(false)
        } else {
            None
        }
    }

    // ----- emission -----

    fn emit_char(&mut self, c: char) {
        self.chars.push(c);
    }

    fn emit_str(&mut self, s: &str) {
        self.chars.push_str(s);
    }

    fn push_token(&mut self, t: Token) {
        if !self.chars.is_empty() {
            let chars = std::mem::take(&mut self.chars);
            self.tokens.push_back(Token::Character(chars));
        }
        self.tokens.push_back(t);
    }

    fn emit_eof(&mut self) {
        self.push_token(Token::Eof);
        self.emitted_eof = true;
    }

    fn new_tag(&mut self, end: bool) {
        self.tag = Tag::default();
        self.tag_is_end = end;
        self.attr = None;
    }

    fn start_attr(&mut self) {
        self.finish_attr();
        self.attr = Some(Attribute {
            name: String::new(),
            value: String::new(),
        });
    }

    /// Duplicate attributes are dropped (the first one wins).
    fn finish_attr(&mut self) {
        if let Some(a) = self.attr.take() {
            if !self.tag.attrs.iter().any(|x| x.name == a.name) {
                self.tag.attrs.push(a);
            }
        }
    }

    fn attr_name(&mut self) -> &mut String {
        &mut self
            .attr
            .get_or_insert_with(|| Attribute {
                name: String::new(),
                value: String::new(),
            })
            .name
    }

    fn attr_value(&mut self) -> &mut String {
        &mut self
            .attr
            .get_or_insert_with(|| Attribute {
                name: String::new(),
                value: String::new(),
            })
            .value
    }

    fn emit_tag(&mut self) {
        self.finish_attr();
        let tag = std::mem::take(&mut self.tag);
        if self.tag_is_end {
            self.push_token(Token::EndTag(tag));
        } else {
            self.last_start_tag = Some(tag.name.clone());
            self.push_token(Token::StartTag(tag));
        }
    }

    fn emit_comment(&mut self) {
        let c = std::mem::take(&mut self.comment);
        self.push_token(Token::Comment(c));
    }

    fn emit_processing_instruction(&mut self) {
        let target = std::mem::take(&mut self.temp);
        let data = std::mem::take(&mut self.pi_data);
        self.push_token(Token::ProcessingInstruction { target, data });
    }

    /// "Convert the temporary buffer to a comment" and reconsume in the bogus comment state.
    fn pi_to_bogus_comment(&mut self, input: &Input) {
        self.comment.clear();
        self.comment.push('?');
        let temp = std::mem::take(&mut self.temp);
        self.comment.push_str(&temp);
        self.reconsume(input, State::BogusComment);
    }

    fn emit_doctype(&mut self) {
        let d = std::mem::take(&mut self.doctype);
        self.push_token(Token::Doctype(d));
    }

    fn appropriate_end_tag(&self) -> bool {
        self.last_start_tag.as_deref() == Some(self.tag.name.as_str())
    }

    fn in_attribute(&self) -> bool {
        matches!(
            self.return_state,
            State::AttributeValueDoubleQuoted
                | State::AttributeValueSingleQuoted
                | State::AttributeValueUnquoted
        )
    }

    fn flush_char_ref(&mut self) {
        let temp = std::mem::take(&mut self.temp);
        if self.in_attribute() {
            self.attr_value().push_str(&temp);
        } else {
            self.emit_str(&temp);
        }
    }

    /// Copy ordinary characters up to the next byte in `stops` in one go.
    fn run_until(&mut self, stops: &[u8]) -> bool {
        let rest = &self.input.as_bytes()[self.pos..];
        let n = rest
            .iter()
            .position(|b| stops.contains(b))
            .unwrap_or(rest.len());
        if n == 0 {
            return false;
        }
        let end = self.pos + n;
        let s = &self.input[self.pos..end];
        self.chars.push_str(s);
        self.pos = end;
        true
    }

    // ----- the state machine -----

    /// Run one transition. Returns false when more input is needed.
    fn step(&mut self) -> bool {
        use State as S;
        let text_stops: Option<&[u8]> = match self.state {
            S::NumericCharacterReferenceEnd => {
                self.numeric_reference_end();
                return true;
            }
            S::NamedCharacterReference => return self.named_reference(),
            S::MarkupDeclarationOpen => return self.markup_declaration_open(),
            S::Data | S::Rcdata => Some(b"&<\0"),
            S::Rawtext | S::ScriptData => Some(b"<\0"),
            S::Plaintext => Some(b"\0"),
            _ => None,
        };
        if text_stops.is_some_and(|stops| self.run_until(stops)) {
            return true;
        }
        let input = self.getc();
        let c = match input {
            Input::NeedData => return false,
            Input::Eof => None,
            Input::Char(c) => Some(c),
        };
        match self.state {
            S::Data => match c {
                Some('&') => {
                    self.return_state = S::Data;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::TagOpen,
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::Rcdata => match c {
                Some('&') => {
                    self.return_state = S::Rcdata;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::RcdataLessThanSign,
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::Rawtext => match c {
                Some('<') => self.state = S::RawtextLessThanSign,
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::ScriptData => match c {
                Some('<') => self.state = S::ScriptDataLessThanSign,
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::Plaintext => match c {
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::TagOpen => match c {
                Some('!') => self.state = S::MarkupDeclarationOpen,
                Some('/') => self.state = S::EndTagOpen,
                Some(c) if c.is_ascii_alphabetic() => {
                    self.new_tag(false);
                    self.reconsume(&input, S::TagName);
                }
                Some('?') => {
                    self.temp.clear();
                    self.state = S::ProcessingInstructionOpen;
                }
                None => {
                    self.emit_char('<');
                    self.emit_eof();
                }
                Some(_) => {
                    self.emit_char('<');
                    self.reconsume(&input, S::Data);
                }
            },
            S::EndTagOpen => match c {
                Some(c) if c.is_ascii_alphabetic() => {
                    self.new_tag(true);
                    self.reconsume(&input, S::TagName);
                }
                Some('>') => self.state = S::Data,
                None => {
                    self.emit_str("</");
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.clear();
                    self.reconsume(&input, S::BogusComment);
                }
            },
            S::TagName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some('\0') => self.tag.name.push(REPLACEMENT),
                Some(c) => self.tag.name.push(c.to_ascii_lowercase()),
                None => self.emit_eof(),
            },
            S::RcdataLessThanSign => {
                self.text_less_than_sign(&input, S::Rcdata, S::RcdataEndTagOpen)
            }
            S::RcdataEndTagOpen => self.text_end_tag_open(&input, S::Rcdata, S::RcdataEndTagName),
            S::RcdataEndTagName => self.text_end_tag_name(&input, S::Rcdata),
            S::RawtextLessThanSign => {
                self.text_less_than_sign(&input, S::Rawtext, S::RawtextEndTagOpen)
            }
            S::RawtextEndTagOpen => {
                self.text_end_tag_open(&input, S::Rawtext, S::RawtextEndTagName)
            }
            S::RawtextEndTagName => self.text_end_tag_name(&input, S::Rawtext),
            S::ScriptDataLessThanSign => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEndTagOpen;
                }
                Some('!') => {
                    self.state = S::ScriptDataEscapeStart;
                    self.emit_str("<!");
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume(&input, S::ScriptData);
                }
            },
            S::ScriptDataEndTagOpen => {
                self.text_end_tag_open(&input, S::ScriptData, S::ScriptDataEndTagName)
            }
            S::ScriptDataEndTagName => self.text_end_tag_name(&input, S::ScriptData),
            S::ScriptDataEscapeStart => match c {
                Some('-') => {
                    self.state = S::ScriptDataEscapeStartDash;
                    self.emit_char('-');
                }
                _ => self.reconsume(&input, S::ScriptData),
            },
            S::ScriptDataEscapeStartDash => match c {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                _ => self.reconsume(&input, S::ScriptData),
            },
            S::ScriptDataEscaped => match c {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::ScriptDataEscapedDash => match c {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('\0') => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(REPLACEMENT);
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            S::ScriptDataEscapedDashDash => match c {
                Some('-') => self.emit_char('-'),
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(REPLACEMENT);
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            S::ScriptDataEscapedLessThanSign => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEscapedEndTagOpen;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.clear();
                    self.emit_char('<');
                    self.reconsume(&input, S::ScriptDataDoubleEscapeStart);
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume(&input, S::ScriptDataEscaped);
                }
            },
            S::ScriptDataEscapedEndTagOpen => {
                self.text_end_tag_open(&input, S::ScriptDataEscaped, S::ScriptDataEscapedEndTagName)
            }
            S::ScriptDataEscapedEndTagName => self.text_end_tag_name(&input, S::ScriptDataEscaped),
            S::ScriptDataDoubleEscapeStart => match c {
                Some(c @ ('\t' | '\n' | '\x0C' | ' ' | '/' | '>')) => {
                    self.state = if self.temp == "script" {
                        S::ScriptDataDoubleEscaped
                    } else {
                        S::ScriptDataEscaped
                    };
                    self.emit_char(c);
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.push(c.to_ascii_lowercase());
                    self.emit_char(c);
                }
                _ => self.reconsume(&input, S::ScriptDataEscaped),
            },
            S::ScriptDataDoubleEscaped => match c {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => self.emit_char(REPLACEMENT),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::ScriptDataDoubleEscapedDash => match c {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(REPLACEMENT);
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            S::ScriptDataDoubleEscapedDashDash => match c {
                Some('-') => self.emit_char('-'),
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(REPLACEMENT);
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            S::ScriptDataDoubleEscapedLessThanSign => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataDoubleEscapeEnd;
                    self.emit_char('/');
                }
                _ => self.reconsume(&input, S::ScriptDataDoubleEscaped),
            },
            S::ScriptDataDoubleEscapeEnd => match c {
                Some(c @ ('\t' | '\n' | '\x0C' | ' ' | '/' | '>')) => {
                    self.state = if self.temp == "script" {
                        S::ScriptDataEscaped
                    } else {
                        S::ScriptDataDoubleEscaped
                    };
                    self.emit_char(c);
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.push(c.to_ascii_lowercase());
                    self.emit_char(c);
                }
                _ => self.reconsume(&input, S::ScriptDataDoubleEscaped),
            },
            S::BeforeAttributeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('/' | '>') | None => self.reconsume(&input, S::AfterAttributeName),
                Some('=') => {
                    self.start_attr();
                    self.attr_name().push('=');
                    self.state = S::AttributeName;
                }
                Some(_) => {
                    self.start_attr();
                    self.reconsume(&input, S::AttributeName);
                }
            },
            S::AttributeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ' | '/' | '>') | None => {
                    self.reconsume(&input, S::AfterAttributeName)
                }
                Some('=') => self.state = S::BeforeAttributeValue,
                Some('\0') => self.attr_name().push(REPLACEMENT),
                Some(c) => self.attr_name().push(c.to_ascii_lowercase()),
            },
            S::AfterAttributeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('=') => self.state = S::BeforeAttributeValue,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                Some(_) => {
                    self.start_attr();
                    self.reconsume(&input, S::AttributeName);
                }
            },
            S::BeforeAttributeValue => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('"') => self.state = S::AttributeValueDoubleQuoted,
                Some('\'') => self.state = S::AttributeValueSingleQuoted,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                _ => self.reconsume(&input, S::AttributeValueUnquoted),
            },
            S::AttributeValueDoubleQuoted => self.quoted_attr_value(c, '"'),
            S::AttributeValueSingleQuoted => self.quoted_attr_value(c, '\''),
            S::AttributeValueUnquoted => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => self.state = S::BeforeAttributeName,
                Some('&') => {
                    self.return_state = S::AttributeValueUnquoted;
                    self.state = S::CharacterReference;
                }
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some('\0') => self.attr_value().push(REPLACEMENT),
                Some(c) => self.attr_value().push(c),
                None => self.emit_eof(),
            },
            S::AfterAttributeValueQuoted => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                Some(_) => self.reconsume(&input, S::BeforeAttributeName),
            },
            S::SelfClosingStartTag => match c {
                Some('>') => {
                    self.tag.self_closing = true;
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                Some(_) => self.reconsume(&input, S::BeforeAttributeName),
            },
            S::BogusComment => match c {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some('\0') => self.comment.push(REPLACEMENT),
                Some(c) => self.comment.push(c),
            },
            S::ProcessingInstructionOpen => match c {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    self.reconsume(&input, S::ProcessingInstructionTarget)
                }
                None => self.emit_eof(),
                Some(_) => self.pi_to_bogus_comment(&input),
            },
            S::ProcessingInstructionTarget => match c {
                Some('\t' | '\n' | '\x0C' | ' ' | '?' | '>') => {
                    if self.temp.eq_ignore_ascii_case("xml")
                        || self.temp.eq_ignore_ascii_case("xml-stylesheet")
                    {
                        self.pi_to_bogus_comment(&input);
                    } else {
                        self.pi_data.clear();
                        self.reconsume(&input, S::AfterProcessingInstructionTarget);
                    }
                }
                Some(c) if c.is_ascii_alphanumeric() || c == '-' || c == '_' => self.temp.push(c),
                None => self.emit_eof(),
                Some(_) => self.pi_to_bogus_comment(&input),
            },
            S::AfterProcessingInstructionTarget => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                _ => self.reconsume(&input, S::ProcessingInstructionData),
            },
            S::ProcessingInstructionData => match c {
                Some('?') => self.state = S::ProcessingInstructionQuestionable,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_processing_instruction();
                }
                None => self.emit_eof(),
                Some(c) => self.pi_data.push(c),
            },
            S::ProcessingInstructionQuestionable => match c {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_processing_instruction();
                }
                None => self.emit_eof(),
                Some(_) => {
                    self.pi_data.push('?');
                    self.reconsume(&input, S::ProcessingInstructionData);
                }
            },
            S::CommentStart => match c {
                Some('-') => self.state = S::CommentStartDash,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                _ => self.reconsume(&input, S::Comment),
            },
            S::CommentStartDash => match c {
                Some('-') => self.state = S::CommentEnd,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(&input, S::Comment);
                }
            },
            S::Comment => match c {
                Some('<') => {
                    self.comment.push('<');
                    self.state = S::CommentLessThanSign;
                }
                Some('-') => self.state = S::CommentEndDash,
                Some('\0') => self.comment.push(REPLACEMENT),
                Some(c) => self.comment.push(c),
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
            },
            S::CommentLessThanSign => match c {
                Some('!') => {
                    self.comment.push('!');
                    self.state = S::CommentLessThanSignBang;
                }
                Some('<') => self.comment.push('<'),
                _ => self.reconsume(&input, S::Comment),
            },
            S::CommentLessThanSignBang => match c {
                Some('-') => self.state = S::CommentLessThanSignBangDash,
                _ => self.reconsume(&input, S::Comment),
            },
            S::CommentLessThanSignBangDash => match c {
                Some('-') => self.state = S::CommentLessThanSignBangDashDash,
                _ => self.reconsume(&input, S::CommentEndDash),
            },
            S::CommentLessThanSignBangDashDash => self.reconsume(&input, S::CommentEnd),
            S::CommentEndDash => match c {
                Some('-') => self.state = S::CommentEnd,
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(&input, S::Comment);
                }
            },
            S::CommentEnd => match c {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                Some('!') => self.state = S::CommentEndBang,
                Some('-') => self.comment.push('-'),
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--");
                    self.reconsume(&input, S::Comment);
                }
            },
            S::CommentEndBang => match c {
                Some('-') => {
                    self.comment.push_str("--!");
                    self.state = S::CommentEndDash;
                }
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--!");
                    self.reconsume(&input, S::Comment);
                }
            },
            S::Doctype => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => self.state = S::BeforeDoctypeName,
                None => {
                    self.doctype = Doctype {
                        force_quirks: true,
                        ..Doctype::default()
                    };
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => self.reconsume(&input, S::BeforeDoctypeName),
            },
            S::BeforeDoctypeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('>') => {
                    self.doctype = Doctype {
                        force_quirks: true,
                        ..Doctype::default()
                    };
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype = Doctype {
                        force_quirks: true,
                        ..Doctype::default()
                    };
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(c) => {
                    let c = if c == '\0' {
                        REPLACEMENT
                    } else {
                        c.to_ascii_lowercase()
                    };
                    self.doctype = Doctype {
                        name: Some(c.to_string()),
                        ..Doctype::default()
                    };
                    self.state = S::DoctypeName;
                }
            },
            S::DoctypeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => self.state = S::AfterDoctypeName,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                Some(c) => {
                    let c = if c == '\0' {
                        REPLACEMENT
                    } else {
                        c.to_ascii_lowercase()
                    };
                    self.doctype.name.get_or_insert_with(String::new).push(c);
                }
            },
            S::AfterDoctypeName => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                Some(_) => {
                    self.unget(&input);
                    let public = self.lookahead("PUBLIC", true);
                    let system = self.lookahead("SYSTEM", true);
                    match (public, system) {
                        (Some(true), _) => {
                            self.pos += 6;
                            self.state = S::AfterDoctypePublicKeyword;
                        }
                        (_, Some(true)) => {
                            self.pos += 6;
                            self.state = S::AfterDoctypeSystemKeyword;
                        }
                        (None, _) | (_, None) => return false,
                        _ => {
                            self.doctype.force_quirks = true;
                            self.state = S::BogusDoctype;
                        }
                    }
                }
            },
            S::AfterDoctypePublicKeyword | S::BeforeDoctypePublicIdentifier => {
                let after_keyword = self.state == S::AfterDoctypePublicKeyword;
                match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        if after_keyword {
                            self.state = S::BeforeDoctypePublicIdentifier;
                        }
                    }
                    Some('"') => {
                        self.doctype.public_id = Some(String::new());
                        self.state = S::DoctypePublicIdentifierDoubleQuoted;
                    }
                    Some('\'') => {
                        self.doctype.public_id = Some(String::new());
                        self.state = S::DoctypePublicIdentifierSingleQuoted;
                    }
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = S::Data;
                        self.emit_doctype();
                    }
                    None => self.doctype_eof(),
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(&input, S::BogusDoctype);
                    }
                }
            }
            S::DoctypePublicIdentifierDoubleQuoted | S::DoctypePublicIdentifierSingleQuoted => {
                let quote = if self.state == S::DoctypePublicIdentifierDoubleQuoted {
                    '"'
                } else {
                    '\''
                };
                match c {
                    Some(q) if q == quote => self.state = S::AfterDoctypePublicIdentifier,
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = S::Data;
                        self.emit_doctype();
                    }
                    None => self.doctype_eof(),
                    Some(c) => {
                        let c = if c == '\0' { REPLACEMENT } else { c };
                        self.doctype
                            .public_id
                            .get_or_insert_with(String::new)
                            .push(c);
                    }
                }
            }
            S::AfterDoctypePublicIdentifier | S::BetweenDoctypePublicAndSystemIdentifiers => {
                let after_public = self.state == S::AfterDoctypePublicIdentifier;
                match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        if after_public {
                            self.state = S::BetweenDoctypePublicAndSystemIdentifiers;
                        }
                    }
                    Some('>') => {
                        self.state = S::Data;
                        self.emit_doctype();
                    }
                    Some('"') => {
                        self.doctype.system_id = Some(String::new());
                        self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    Some('\'') => {
                        self.doctype.system_id = Some(String::new());
                        self.state = S::DoctypeSystemIdentifierSingleQuoted;
                    }
                    None => self.doctype_eof(),
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(&input, S::BogusDoctype);
                    }
                }
            }
            S::AfterDoctypeSystemKeyword | S::BeforeDoctypeSystemIdentifier => {
                let after_keyword = self.state == S::AfterDoctypeSystemKeyword;
                match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        if after_keyword {
                            self.state = S::BeforeDoctypeSystemIdentifier;
                        }
                    }
                    Some('"') => {
                        self.doctype.system_id = Some(String::new());
                        self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    Some('\'') => {
                        self.doctype.system_id = Some(String::new());
                        self.state = S::DoctypeSystemIdentifierSingleQuoted;
                    }
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = S::Data;
                        self.emit_doctype();
                    }
                    None => self.doctype_eof(),
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(&input, S::BogusDoctype);
                    }
                }
            }
            S::DoctypeSystemIdentifierDoubleQuoted | S::DoctypeSystemIdentifierSingleQuoted => {
                let quote = if self.state == S::DoctypeSystemIdentifierDoubleQuoted {
                    '"'
                } else {
                    '\''
                };
                match c {
                    Some(q) if q == quote => self.state = S::AfterDoctypeSystemIdentifier,
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = S::Data;
                        self.emit_doctype();
                    }
                    None => self.doctype_eof(),
                    Some(c) => {
                        let c = if c == '\0' { REPLACEMENT } else { c };
                        self.doctype
                            .system_id
                            .get_or_insert_with(String::new)
                            .push(c);
                    }
                }
            }
            S::AfterDoctypeSystemIdentifier => match c {
                Some('\t' | '\n' | '\x0C' | ' ') => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                Some(_) => self.reconsume(&input, S::BogusDoctype),
            },
            S::BogusDoctype => match c {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => {
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {}
            },
            S::CdataSection => match c {
                Some(']') => self.state = S::CdataSectionBracket,
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            S::CdataSectionBracket => match c {
                Some(']') => self.state = S::CdataSectionEnd,
                _ => {
                    self.emit_char(']');
                    self.reconsume(&input, S::CdataSection);
                }
            },
            S::CdataSectionEnd => match c {
                Some(']') => self.emit_char(']'),
                Some('>') => self.state = S::Data,
                _ => {
                    self.emit_str("]]");
                    self.reconsume(&input, S::CdataSection);
                }
            },
            S::CharacterReference => {
                self.temp.clear();
                self.temp.push('&');
                match c {
                    Some(c) if c.is_ascii_alphanumeric() => {
                        self.reconsume(&input, S::NamedCharacterReference)
                    }
                    Some('#') => {
                        self.temp.push('#');
                        self.state = S::NumericCharacterReference;
                    }
                    _ => {
                        self.flush_char_ref();
                        let r = self.return_state;
                        self.reconsume(&input, r);
                    }
                }
            }
            S::AmbiguousAmpersand => match c {
                Some(c) if c.is_ascii_alphanumeric() => {
                    if self.in_attribute() {
                        self.attr_value().push(c);
                    } else {
                        self.emit_char(c);
                    }
                }
                _ => {
                    let r = self.return_state;
                    self.reconsume(&input, r);
                }
            },
            S::NumericCharacterReference => {
                self.char_ref_code = 0;
                match c {
                    Some(c @ ('x' | 'X')) => {
                        self.temp.push(c);
                        self.state = S::HexadecimalCharacterReferenceStart;
                    }
                    _ => self.reconsume(&input, S::DecimalCharacterReferenceStart),
                }
            }
            S::HexadecimalCharacterReferenceStart | S::DecimalCharacterReferenceStart => {
                let hex = self.state == S::HexadecimalCharacterReferenceStart;
                let digit = c.is_some_and(|c| {
                    if hex {
                        c.is_ascii_hexdigit()
                    } else {
                        c.is_ascii_digit()
                    }
                });
                if digit {
                    let next = if hex {
                        S::HexadecimalCharacterReference
                    } else {
                        S::DecimalCharacterReference
                    };
                    self.reconsume(&input, next);
                } else {
                    self.flush_char_ref();
                    let r = self.return_state;
                    self.reconsume(&input, r);
                }
            }
            S::HexadecimalCharacterReference | S::DecimalCharacterReference => {
                let radix = if self.state == S::HexadecimalCharacterReference {
                    16
                } else {
                    10
                };
                match c.and_then(|c| c.to_digit(radix)) {
                    Some(d) => {
                        self.char_ref_code = self
                            .char_ref_code
                            .saturating_mul(radix)
                            .saturating_add(d)
                            .min(0x11_0000);
                    }
                    None if c == Some(';') => self.state = S::NumericCharacterReferenceEnd,
                    None => self.reconsume(&input, S::NumericCharacterReferenceEnd),
                }
            }
            S::NumericCharacterReferenceEnd
            | S::NamedCharacterReference
            | S::MarkupDeclarationOpen => unreachable!("handled before consuming input"),
        }
        true
    }

    fn doctype_eof(&mut self) {
        self.doctype.force_quirks = true;
        self.emit_doctype();
        self.emit_eof();
    }

    fn quoted_attr_value(&mut self, c: Option<char>, quote: char) {
        match c {
            Some(q) if q == quote => self.state = State::AfterAttributeValueQuoted,
            Some('&') => {
                self.return_state = self.state;
                self.state = State::CharacterReference;
            }
            Some('\0') => self.attr_value().push(REPLACEMENT),
            Some(c) => {
                self.attr_value().push(c);
                let stops: &[u8] = if quote == '"' { b"\"&\0" } else { b"'&\0" };
                let rest = &self.input.as_bytes()[self.pos..];
                let n = rest
                    .iter()
                    .position(|b| stops.contains(b))
                    .unwrap_or(rest.len());
                if n > 0 {
                    let end = self.pos + n;
                    let s = self.input[self.pos..end].to_string();
                    self.attr_value().push_str(&s);
                    self.pos = end;
                }
            }
            None => self.emit_eof(),
        }
    }

    /// RCDATA / RAWTEXT / script data less-than sign state.
    fn text_less_than_sign(&mut self, input: &Input, text: State, end_tag_open: State) {
        if matches!(input, Input::Char('/')) {
            self.temp.clear();
            self.state = end_tag_open;
        } else {
            self.emit_char('<');
            self.reconsume(input, text);
        }
    }

    /// RCDATA / RAWTEXT / script data (escaped) end tag open state.
    fn text_end_tag_open(&mut self, input: &Input, text: State, end_tag_name: State) {
        match input {
            Input::Char(c) if c.is_ascii_alphabetic() => {
                self.new_tag(true);
                self.reconsume(input, end_tag_name);
            }
            _ => {
                self.emit_str("</");
                self.reconsume(input, text);
            }
        }
    }

    /// RCDATA / RAWTEXT / script data (escaped) end tag name state.
    fn text_end_tag_name(&mut self, input: &Input, text: State) {
        let c = match input {
            Input::Char(c) => Some(*c),
            _ => None,
        };
        match c {
            Some('\t' | '\n' | '\x0C' | ' ') if self.appropriate_end_tag() => {
                self.state = State::BeforeAttributeName;
                return;
            }
            Some('/') if self.appropriate_end_tag() => {
                self.state = State::SelfClosingStartTag;
                return;
            }
            Some('>') if self.appropriate_end_tag() => {
                self.state = State::Data;
                self.emit_tag();
                return;
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.tag.name.push(c.to_ascii_lowercase());
                self.temp.push(c);
                return;
            }
            _ => {}
        }
        self.emit_str("</");
        let temp = std::mem::take(&mut self.temp);
        self.emit_str(&temp);
        self.reconsume(input, text);
    }

    fn markup_declaration_open(&mut self) -> bool {
        match self.lookahead("--", false) {
            None => return false,
            Some(true) => {
                self.pos += 2;
                self.comment.clear();
                self.state = State::CommentStart;
                return true;
            }
            Some(false) => {}
        }
        match self.lookahead("DOCTYPE", true) {
            None => return false,
            Some(true) => {
                self.pos += 7;
                self.state = State::Doctype;
                return true;
            }
            Some(false) => {}
        }
        match self.lookahead("[CDATA[", false) {
            None => return false,
            Some(true) => {
                self.pos += 7;
                if self.allow_cdata {
                    self.state = State::CdataSection;
                } else {
                    self.comment = "[CDATA[".to_string();
                    self.state = State::BogusComment;
                }
                return true;
            }
            Some(false) => {}
        }
        self.comment.clear();
        self.state = State::BogusComment;
        true
    }

    fn named_reference(&mut self) -> bool {
        let rest = &self.input.as_bytes()[self.pos..];
        let alnum = rest
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        let run = alnum + usize::from(rest.get(alnum) == Some(&b';'));
        // The longest possible match, and the character after it, must have arrived.
        if !self.eof && alnum == rest.len() && alnum <= LONGEST_NAME {
            return false;
        }
        let best = (1..=run.min(LONGEST_NAME)).rev().find_map(|len| {
            let name = std::str::from_utf8(&rest[..len]).ok()?;
            ENTITIES
                .binary_search_by(|(n, _)| n.cmp(&name))
                .ok()
                .map(|i| (len, ENTITIES[i].1))
        });
        let Some((len, chars)) = best else {
            self.flush_char_ref();
            self.state = State::AmbiguousAmpersand;
            return true;
        };
        let ends_with_semicolon = rest[len - 1] == b';';
        let next = rest.get(len).copied();
        let matched = std::str::from_utf8(&rest[..len])
            .unwrap_or_default()
            .to_string();
        self.pos += len;
        if self.in_attribute()
            && !ends_with_semicolon
            && next.is_some_and(|b| b == b'=' || b.is_ascii_alphanumeric())
        {
            self.temp.push_str(&matched);
        } else {
            self.temp = chars.to_string();
        }
        self.flush_char_ref();
        self.state = self.return_state;
        true
    }

    fn numeric_reference_end(&mut self) {
        let code = self.char_ref_code;
        let c = match code {
            0 => REPLACEMENT,
            c if c > 0x10_FFFF => REPLACEMENT,
            0xD800..=0xDFFF => REPLACEMENT,
            0x80..=0x9F => windows_1252_c1(code).unwrap_or_else(|| char::from_u32(code).unwrap()),
            c => char::from_u32(c).unwrap_or(REPLACEMENT),
        };
        self.temp.clear();
        self.temp.push(c);
        self.flush_char_ref();
        self.state = self.return_state;
    }
}

/// The numeric character reference end state's C1 replacement table.
fn windows_1252_c1(code: u32) -> Option<char> {
    let c = match code {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        _ => return None,
    };
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(name: &str, attrs: &[(&str, &str)]) -> Token {
        Token::StartTag(Tag {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(n, v)| Attribute {
                    name: (*n).into(),
                    value: (*v).into(),
                })
                .collect(),
            self_closing: false,
        })
    }

    #[test]
    fn tags_attributes_text_and_references() {
        let toks = tokenize("<P Class=a id='b' x=\"&amp;c\" x=dup>&lt;hi&notit; &#x41;&#128;</p>");
        assert_eq!(
            toks,
            vec![
                start("p", &[("class", "a"), ("id", "b"), ("x", "&c")]),
                Token::Character("<hi\u{AC}it; A\u{20AC}".into()),
                Token::EndTag(Tag {
                    name: "p".into(),
                    ..Tag::default()
                }),
            ]
        );
    }

    #[test]
    fn legacy_reference_in_attribute_followed_by_equals_is_literal() {
        let toks = tokenize("<a href='?a=1&copy=2&copy;'>");
        assert_eq!(toks, vec![start("a", &[("href", "?a=1&copy=2\u{A9}")])]);
    }

    #[test]
    fn processing_instructions() {
        let pi = |target: &str, data: &str| Token::ProcessingInstruction {
            target: target.into(),
            data: data.into(),
        };
        assert_eq!(tokenize("<?target  a ?b?>"), vec![pi("target", "a ?b")]);
        assert_eq!(tokenize("<?x-y_1>"), vec![pi("x-y_1", "")]);
        assert_eq!(
            tokenize("<?XML version='1.0'?>"),
            vec![Token::Comment("?XML version='1.0'?".into())]
        );
        assert_eq!(tokenize("<?1>"), vec![Token::Comment("?1".into())]);
        assert_eq!(tokenize("<?t data"), vec![]);
        assert_eq!(tokenize("<?"), vec![]);
    }

    #[test]
    fn doctype_with_identifiers() {
        let toks = tokenize("<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\" 'x'>");
        assert_eq!(
            toks,
            vec![Token::Doctype(Doctype {
                name: Some("html".into()),
                public_id: Some("-//W3C//DTD HTML 4.01//EN".into()),
                system_id: Some("x".into()),
                force_quirks: false,
            })]
        );
    }

    #[test]
    fn script_data_needs_an_appropriate_end_tag() {
        let mut t = Tokenizer::new();
        t.feed("<script>");
        assert_eq!(t.next_token(), Some(start("script", &[])));
        t.set_state(State::ScriptData);
        t.feed("a</b></scriptx></script>");
        t.finish();
        assert_eq!(
            t.next_token(),
            Some(Token::Character("a</b></scriptx>".into()))
        );
        assert!(matches!(t.next_token(), Some(Token::EndTag(tag)) if tag.name == "script"));
        assert_eq!(t.next_token(), Some(Token::Eof));
        assert_eq!(t.next_token(), None);
    }

    #[test]
    fn every_chunking_gives_the_same_tokens() {
        let html = "<!DOCTYPE html><p a=\"x&amp;y\" b=c>t\r\nu &notin; &noti; &#65<!-- c -- -->\r</p><![CDATA[x]]>";
        let whole = tokenize(html);
        for size in 1..8 {
            let mut t = Tokenizer::new();
            let mut out = Vec::new();
            let chars: Vec<char> = html.chars().collect();
            for chunk in chars.chunks(size) {
                t.feed(&chunk.iter().collect::<String>());
                while let Some(tok) = t.next_token() {
                    out.push(tok);
                }
            }
            t.finish();
            while let Some(tok) = t.next_token() {
                if tok != Token::Eof {
                    out.push(tok);
                }
            }
            let mut merged: Vec<Token> = Vec::new();
            for tok in out {
                match (merged.last_mut(), tok) {
                    (Some(Token::Character(a)), Token::Character(b)) => a.push_str(&b),
                    (_, tok) => merged.push(tok),
                }
            }
            assert_eq!(merged, whole, "chunk size {size}");
        }
    }

    #[test]
    fn newlines_are_normalized_across_chunks() {
        let mut t = Tokenizer::new();
        t.feed("a\r");
        t.feed("\nb\rc");
        t.finish();
        assert_eq!(t.next_token(), Some(Token::Character("a\nb\nc".into())));
    }
}
