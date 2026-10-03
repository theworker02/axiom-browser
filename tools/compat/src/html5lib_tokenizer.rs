//! html5lib tokenizer tests (`tokenizer/*.test`, JSON).
//!
//! Each test runs once per entry in `initialStates` against
//! [`axiom_html::tokenizer::Tokenizer`], and the token stream (adjacent character tokens
//! merged) must equal `output`. Parse errors are not compared. `xmlViolationTests` are
//! excluded: they test XML-infoset coercion, not HTML tokenization. Tests whose
//! `doubleEscaped` input contains lone surrogates cannot be fed to a UTF-8 tokenizer and
//! run as failures.

use std::path::Path;

use axiom_html::tokenizer::{State, Token, Tokenizer};
use serde_json::{json, Value};

use crate::expectations::{Outcome, Status};
use crate::runner::{panic_message, parallel_map};

#[derive(Debug, Clone)]
pub struct TokenizerCase {
    pub file: String,
    pub index: usize,
    pub description: String,
    pub state: String,
    pub last_start_tag: Option<String>,
    pub input: Result<String, String>,
    pub output: Result<Vec<Value>, String>,
}

impl TokenizerCase {
    pub fn id(&self) -> String {
        format!("{}:{}@{}", self.file, self.index, self.state)
    }
}

/// Decode html5lib's `doubleEscaped` `\uXXXX` sequences.
pub fn unescape(s: &str) -> Result<String, String> {
    let mut units: Vec<u16> = Vec::new();
    let mut out = String::new();
    let flush = |units: &mut Vec<u16>, out: &mut String| -> Result<(), String> {
        if units.is_empty() {
            return Ok(());
        }
        let s = String::from_utf16(units).map_err(|_| "lone surrogate in input".to_string())?;
        out.push_str(&s);
        units.clear();
        Ok(())
    };
    let mut rest = s;
    while let Some(i) = rest.find("\\u") {
        let hex = rest.get(i + 2..i + 6);
        match hex.and_then(|h| u16::from_str_radix(h, 16).ok()) {
            Some(u) => {
                if i > 0 {
                    flush(&mut units, &mut out)?;
                    out.push_str(&rest[..i]);
                }
                units.push(u);
                rest = &rest[i + 6..];
            }
            None => {
                flush(&mut units, &mut out)?;
                out.push_str(&rest[..i + 2]);
                rest = &rest[i + 2..];
            }
        }
    }
    flush(&mut units, &mut out)?;
    out.push_str(rest);
    Ok(out)
}

fn unescape_value(v: &Value) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => Value::String(unescape(s)?),
        Value::Array(a) => Value::Array(a.iter().map(unescape_value).collect::<Result<_, _>>()?),
        Value::Object(o) => {
            let mut m = serde_json::Map::new();
            for (k, v) in o {
                m.insert(unescape(k)?, unescape_value(v)?);
            }
            Value::Object(m)
        }
        other => other.clone(),
    })
}

/// Merge adjacent `["Character", …]` tokens.
fn merge_characters(tokens: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for t in tokens {
        let is_char = t.get(0).and_then(Value::as_str) == Some("Character");
        if is_char {
            if let Some(last) = out.last_mut() {
                if last.get(0).and_then(Value::as_str) == Some("Character") {
                    let add = t[1].as_str().unwrap_or_default().to_string();
                    let merged = format!("{}{add}", last[1].as_str().unwrap_or_default());
                    last[1] = Value::String(merged);
                    continue;
                }
            }
        }
        out.push(t);
    }
    out
}

pub fn parse_file(file: &str, text: &str) -> anyhow::Result<Vec<TokenizerCase>> {
    let root: Value = serde_json::from_str(text)?;
    let Some(tests) = root.get("tests").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut cases = Vec::new();
    for (i, t) in tests.iter().enumerate() {
        let double = t.get("doubleEscaped").and_then(Value::as_bool) == Some(true);
        let raw_input = t.get("input").and_then(Value::as_str).unwrap_or_default();
        let input = if double {
            unescape(raw_input)
        } else {
            Ok(raw_input.to_string())
        };
        let raw_output = t.get("output").cloned().unwrap_or(Value::Array(Vec::new()));
        let output = if double {
            unescape_value(&raw_output)
        } else {
            Ok(raw_output)
        }
        .map(|v| merge_characters(v.as_array().cloned().unwrap_or_default()));
        let states: Vec<String> = t
            .get("initialStates")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_else(|| vec!["Data state".to_string()]);
        for state in states {
            cases.push(TokenizerCase {
                file: file.to_string(),
                index: i + 1,
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                state,
                last_start_tag: t
                    .get("lastStartTag")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input: input.clone(),
                output: output.clone(),
            });
        }
    }
    Ok(cases)
}

fn initial_state(name: &str) -> Option<State> {
    Some(match name {
        "Data state" => State::Data,
        "PLAINTEXT state" => State::Plaintext,
        "RCDATA state" => State::Rcdata,
        "RAWTEXT state" => State::Rawtext,
        "Script data state" => State::ScriptData,
        "CDATA section state" => State::CdataSection,
        _ => return None,
    })
}

/// Axiom's token stream in html5lib's JSON shape.
pub fn tokens_as_json(input: &str, state: State, last_start_tag: Option<&str>) -> Vec<Value> {
    let mut t = Tokenizer::new();
    t.set_state(state);
    t.set_last_start_tag(last_start_tag);
    t.feed(input);
    t.finish();
    let mut out = Vec::new();
    while let Some(tok) = t.next_token() {
        out.push(match tok {
            Token::Doctype(d) => {
                json!(["DOCTYPE", d.name, d.public_id, d.system_id, !d.force_quirks])
            }
            Token::StartTag(tag) => {
                let attrs: serde_json::Map<String, Value> = tag
                    .attrs
                    .into_iter()
                    .map(|a| (a.name, Value::String(a.value)))
                    .collect();
                if tag.self_closing {
                    json!(["StartTag", tag.name, attrs, true])
                } else {
                    json!(["StartTag", tag.name, attrs])
                }
            }
            Token::EndTag(tag) => json!(["EndTag", tag.name]),
            Token::Comment(c) => json!(["Comment", c]),
            Token::ProcessingInstruction { target, data } => {
                json!(["ProcessingInstruction", target, data])
            }
            Token::Character(c) => json!(["Character", c]),
            Token::Eof => break,
        });
    }
    merge_characters(out)
}

pub fn run_one(case: &TokenizerCase) -> Outcome {
    let outcome = |status| Outcome::new(case.id(), case.file.clone(), status);
    let (input, expected) = match (&case.input, &case.output) {
        (Ok(i), Ok(o)) => (i, o),
        (Err(e), _) | (_, Err(e)) => return outcome(Status::Fail).with_message(e.clone()),
    };
    let Some(state) = initial_state(&case.state) else {
        return outcome(Status::Fail).with_message(format!("unknown initial state {}", case.state));
    };
    let last = case.last_start_tag.as_deref();
    let got = match std::panic::catch_unwind(|| tokens_as_json(input, state, last)) {
        Ok(g) => g,
        Err(p) => return outcome(Status::Crash).with_message(panic_message(&*p)),
    };
    if &got == expected {
        return outcome(Status::Pass);
    }
    let i = got
        .iter()
        .zip(expected.iter())
        .position(|(g, e)| g != e)
        .unwrap_or(got.len().min(expected.len()));
    let show = |v: Option<&Value>| v.map_or("<end>".to_string(), Value::to_string);
    outcome(Status::Fail).with_message(format!(
        "{}: token {}: expected {}, got {}",
        case.description,
        i + 1,
        show(expected.get(i)),
        show(got.get(i))
    ))
}

pub fn load(root: &Path) -> anyhow::Result<Vec<TokenizerCase>> {
    let dir = root.join("tests/html5lib-tests/tokenizer");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "test"))
        .collect();
    files.sort();
    let mut cases = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path)?;
        cases.extend(parse_file(&name, &text).map_err(|e| anyhow::anyhow!("{name}: {e}"))?);
    }
    Ok(cases)
}

pub fn run(root: &Path, jobs: usize, filter: Option<&str>) -> anyhow::Result<Vec<Outcome>> {
    let cases: Vec<_> = load(root)?
        .into_iter()
        .filter(|c| filter.is_none_or(|f| c.id().contains(f)))
        .collect();
    Ok(parallel_map(&cases, jobs, run_one))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_escaped_strings_decode_and_lone_surrogates_are_reported() {
        assert_eq!(
            unescape("a\\u0000b\\uD83D\\uDE00").unwrap(),
            "a\0b\u{1F600}"
        );
        assert!(unescape("\\uDC00").is_err());
    }

    #[test]
    fn cases_expand_per_initial_state_and_compare_merged_characters() {
        let file = r#"{"tests": [
            {"description": "d", "input": "a<b x=1>c&amp;", "output": [["Character", "a"], ["StartTag", "b", {"x": "1"}], ["Character", "c"], ["Character", "&"]]},
            {"description": "e", "initialStates": ["RCDATA state", "RAWTEXT state"], "lastStartTag": "xmp", "input": "&lt;</xmp>", "output": [["Character", "&lt;"], ["EndTag", "xmp"]]}
        ]}"#;
        let cases = parse_file("f.test", file).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].id(), "f.test:1@Data state");
        assert_eq!(run_one(&cases[0]).status, Status::Pass);
        assert_eq!(
            run_one(&cases[1]).status,
            Status::Fail,
            "RCDATA decodes &lt;"
        );
        assert_eq!(run_one(&cases[2]).status, Status::Pass);
    }
}
