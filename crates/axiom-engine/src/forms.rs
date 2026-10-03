//! HTML forms (HTML §4.10): control state (values, checkedness, selectedness), form
//! owners, constraint validation (`required`, `type=email` / `type=url`), the form data
//! set and its three encodings, and submission planning.
//!
//! The state lives in [`FormState`], shared by the engine, layout and the script host, so
//! typing, clicks and script all see one value per control. Attributes hold the defaults
//! (`value`, `checked`, `selected`, a textarea's text) and are never rewritten by user
//! input.

use std::collections::HashMap;

use axiom_dom::{Document, Namespace, NodeId};
use axiom_url::Url;

/// Per-document form control state.
#[derive(Debug, Default)]
pub struct FormState {
    /// What layout paints: the current value of text controls and the selected option's
    /// value for selects.
    pub values: HashMap<NodeId, String>,
    /// Checkedness of inputs that the user or script changed. Script may set it on any
    /// input (React sets `checked` on text fields), so it never touches `values`.
    pub checked: HashMap<NodeId, bool>,
    /// Selectedness of options that the user or script changed.
    selected: HashMap<NodeId, bool>,
    /// `setCustomValidity()` messages.
    custom_validity: HashMap<NodeId, String>,
    /// The planned navigation of the latest submission, taken by the browsing context.
    submission: Option<FormSubmission>,
    /// The document's frozen base URL, once the loader accepted a `<base href>` (the
    /// Content Security Policy's `base-uri` may reject one); else the document URL.
    base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormMethod {
    Get,
    Post,
}

/// A planned form navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormSubmission {
    pub url: String,
    pub method: FormMethod,
    /// The encoded entry list and its `Content-Type` (POST only).
    pub body: Option<(Vec<u8>, String)>,
    /// The submitting document (referrer and `Origin`).
    pub document_url: String,
}

/// One entry of a form data set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryValue {
    Text(String),
    /// A file control: Axiom has no file picker, so the file is always empty and unnamed.
    File {
        filename: String,
    },
}

/// Why a control fails constraint validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalidity {
    ValueMissing,
    TypeMismatch,
    CustomError,
}

impl Invalidity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValueMissing => "valueMissing",
            Self::TypeMismatch => "typeMismatch",
            Self::CustomError => "customError",
        }
    }
}

const TEXT_TYPES: &[&str] = &[
    "text",
    "search",
    "url",
    "tel",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
];

const INPUT_TYPES: &[&str] = &[
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

fn is_html(doc: &Document, node: NodeId) -> bool {
    doc.namespace(node) == Some(Namespace::Html)
}

fn html_tag(doc: &Document, node: NodeId) -> Option<&str> {
    if is_html(doc, node) {
        doc.tag_name(node)
    } else {
        None
    }
}

/// The input's type keyword (`text` for a missing or unknown type).
pub fn input_type(doc: &Document, node: NodeId) -> String {
    let ty = doc
        .attr(node, "type")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if INPUT_TYPES.contains(&ty.as_str()) {
        ty
    } else {
        "text".into()
    }
}

fn is_input_of(doc: &Document, node: NodeId, types: &[&str]) -> bool {
    html_tag(doc, node) == Some("input") && types.contains(&input_type(doc, node).as_str())
}

/// A text-like control whose value the user types (inputs of a text type, textareas).
pub fn is_text_control(doc: &Document, node: NodeId) -> bool {
    html_tag(doc, node) == Some("textarea") || is_input_of(doc, node, TEXT_TYPES)
}

pub fn is_checkable(doc: &Document, node: NodeId) -> bool {
    is_input_of(doc, node, &["checkbox", "radio"])
}

/// Listed elements (HTML §4.10.2): the controls a form owns.
fn is_listed(doc: &Document, node: NodeId) -> bool {
    matches!(
        html_tag(doc, node),
        Some("button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea")
    )
}

/// Submit buttons: `<button>` without a type or of type submit, `<input type=submit|image>`.
pub fn is_submit_button(doc: &Document, node: NodeId) -> bool {
    match html_tag(doc, node) {
        Some("button") => {
            let ty = doc.attr(node, "type").unwrap_or("").trim();
            !ty.eq_ignore_ascii_case("reset") && !ty.eq_ignore_ascii_case("button")
        }
        Some("input") => matches!(input_type(doc, node).as_str(), "submit" | "image"),
        _ => false,
    }
}

fn is_button(doc: &Document, node: NodeId) -> bool {
    html_tag(doc, node) == Some("button")
        || is_input_of(doc, node, &["submit", "image", "reset", "button"])
}

fn tree_root(doc: &Document, node: NodeId) -> NodeId {
    let mut cur = node;
    while let Some(p) = doc.get(cur).parent {
        cur = p;
    }
    cur
}

fn is_connected(doc: &Document, node: NodeId) -> bool {
    doc.document_id
        .is_some_and(|root| tree_root(doc, node) == root)
}

/// The control's form owner: the form its `form` attribute names, else its nearest form
/// ancestor.
pub fn form_owner(doc: &Document, node: NodeId) -> Option<NodeId> {
    if !is_listed(doc, node) {
        return None;
    }
    if let Some(id) = doc.attr(node, "form") {
        let root = tree_root(doc, node);
        return doc
            .descendants(root)
            .into_iter()
            .find(|&n| doc.is_element(n) && doc.attr(n, "id") == Some(id))
            .filter(|&n| html_tag(doc, n) == Some("form"));
    }
    doc.parent_chain(node)
        .into_iter()
        .rev()
        .skip(1)
        .find(|&n| html_tag(doc, n) == Some("form"))
}

/// The form's listed elements in tree order.
pub fn form_controls(doc: &Document, form: NodeId) -> Vec<NodeId> {
    doc.descendants(tree_root(doc, form))
        .into_iter()
        .filter(|&n| is_listed(doc, n) && form_owner(doc, n) == Some(form))
        .collect()
}

/// A label's labeled control: the element its `for` attribute names, else its first
/// labelable descendant.
fn label_control(doc: &Document, label: NodeId) -> Option<NodeId> {
    let labelable = |n: NodeId| {
        matches!(
            html_tag(doc, n),
            Some("button" | "meter" | "output" | "progress" | "select" | "textarea")
        ) || (html_tag(doc, n) == Some("input") && input_type(doc, n) != "hidden")
    };
    match doc.attr(label, "for") {
        Some(id) => doc
            .descendants(tree_root(doc, label))
            .into_iter()
            .find(|&n| doc.is_element(n) && doc.attr(n, "id") == Some(id))
            .filter(|&n| labelable(n)),
        None => doc
            .descendants(label)
            .into_iter()
            .find(|&n| n != label && labelable(n)),
    }
}

/// The form's default button: its first submit button in tree order.
pub fn default_button(doc: &Document, form: NodeId) -> Option<NodeId> {
    form_controls(doc, form)
        .into_iter()
        .find(|&n| is_submit_button(doc, n))
}

/// Disabled: the `disabled` attribute, or inside a disabled fieldset but not in its first
/// legend.
pub fn is_disabled(doc: &Document, node: NodeId) -> bool {
    if matches!(
        html_tag(doc, node),
        Some("button" | "input" | "select" | "textarea" | "fieldset" | "optgroup" | "option")
    ) && doc.has_attr(node, "disabled")
    {
        return true;
    }
    if html_tag(doc, node) == Some("option") {
        if let Some(p) = doc.get(node).parent {
            if html_tag(doc, p) == Some("optgroup") && doc.has_attr(p, "disabled") {
                return true;
            }
        }
        return false;
    }
    let chain = doc.parent_chain(node);
    for (i, &fs) in chain.iter().enumerate().rev().skip(1) {
        if html_tag(doc, fs) != Some("fieldset") || !doc.has_attr(fs, "disabled") {
            continue;
        }
        let first_legend = doc
            .get(fs)
            .children
            .iter()
            .copied()
            .find(|&c| html_tag(doc, c) == Some("legend"));
        let in_legend = first_legend.is_some_and(|l| chain.get(i + 1) == Some(&l));
        if !in_legend {
            return true;
        }
    }
    false
}

fn collapse_ws(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

/// An option's value: its `value` attribute, else its text with whitespace collapsed.
pub fn option_value(doc: &Document, option: NodeId) -> String {
    doc.attr(option, "value")
        .map(str::to_string)
        .unwrap_or_else(|| collapse_ws(&doc.text_content(option)))
}

/// The options of a select, in tree order (children and optgroup children).
pub fn select_options(doc: &Document, select: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    for &c in &doc.get(select).children {
        match html_tag(doc, c) {
            Some("option") => out.push(c),
            Some("optgroup") => out.extend(
                doc.get(c)
                    .children
                    .iter()
                    .copied()
                    .filter(|&o| html_tag(doc, o) == Some("option")),
            ),
            _ => {}
        }
    }
    out
}

/// The select an option belongs to.
pub fn option_select(doc: &Document, option: NodeId) -> Option<NodeId> {
    let parent = doc.get(option).parent?;
    match html_tag(doc, parent) {
        Some("select") => Some(parent),
        Some("optgroup") => doc
            .get(parent)
            .parent
            .filter(|&s| html_tag(doc, s) == Some("select")),
        _ => None,
    }
}

fn is_multiple(doc: &Document, select: NodeId) -> bool {
    doc.has_attr(select, "multiple")
}

fn display_size(doc: &Document, select: NodeId) -> u32 {
    doc.attr(select, "size")
        .and_then(|s| s.trim().parse().ok())
        .filter(|&s| s > 0)
        .unwrap_or(if is_multiple(doc, select) { 4 } else { 1 })
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

impl FormState {
    /// A control's current value (HTML "value" IDL attribute semantics).
    pub fn value(&self, doc: &Document, node: NodeId) -> String {
        match html_tag(doc, node) {
            Some("input") => match input_type(doc, node).as_str() {
                "checkbox" | "radio" => doc.attr(node, "value").unwrap_or("on").to_string(),
                "hidden" | "submit" | "image" | "reset" | "button" => {
                    doc.attr(node, "value").unwrap_or("").to_string()
                }
                "file" => String::new(),
                "color" => self.values.get(&node).cloned().unwrap_or_else(|| {
                    let v = doc.attr(node, "value").unwrap_or("").to_ascii_lowercase();
                    let valid = v.len() == 7
                        && v.starts_with('#')
                        && v[1..].chars().all(|c| c.is_ascii_hexdigit());
                    if valid {
                        v
                    } else {
                        "#000000".into()
                    }
                }),
                _ => self.values.get(&node).cloned().unwrap_or_else(|| {
                    doc.attr(node, "value")
                        .unwrap_or("")
                        .replace(['\r', '\n'], "")
                }),
            },
            Some("textarea") => self
                .values
                .get(&node)
                .cloned()
                .unwrap_or_else(|| normalize_newlines(&textarea_default(doc, node))),
            Some("select") => self
                .selected_options(doc, node)
                .first()
                .map(|&o| option_value(doc, o))
                .unwrap_or_default(),
            Some("option") => option_value(doc, node),
            Some("button") => doc.attr(node, "value").unwrap_or("").to_string(),
            Some("output") => doc.text_content(node),
            _ => String::new(),
        }
    }

    /// Set a control's value as script's `value` setter does. Returns whether the
    /// document needs repainting.
    pub fn set_value(&mut self, doc: &mut Document, node: NodeId, value: &str) -> bool {
        match html_tag(doc, node) {
            Some("input") => match input_type(doc, node).as_str() {
                "checkbox" | "radio" | "hidden" | "submit" | "image" | "reset" | "button" => {
                    doc.set_attr(node, "value", value);
                    true
                }
                "file" => false,
                _ => {
                    self.values.insert(node, value.replace(['\r', '\n'], ""));
                    true
                }
            },
            Some("textarea") => {
                self.values.insert(node, normalize_newlines(value));
                true
            }
            Some("select") => {
                let options = select_options(doc, node);
                let mut found = false;
                for o in options {
                    let hit = !found && option_value(doc, o) == value;
                    found |= hit;
                    self.selected.insert(o, hit);
                }
                self.sync_select(doc, node);
                true
            }
            Some("button" | "option") => {
                doc.set_attr(node, "value", value);
                true
            }
            _ => false,
        }
    }

    /// Checkedness of a checkbox or radio button.
    pub fn checked(&self, doc: &Document, node: NodeId) -> bool {
        self.checked
            .get(&node)
            .copied()
            .unwrap_or_else(|| doc.has_attr(node, "checked"))
    }

    /// Set checkedness; checking a radio button unchecks the rest of its group.
    pub fn set_checked(&mut self, doc: &Document, node: NodeId, checked: bool) {
        self.checked.insert(node, checked);
        if checked && is_input_of(doc, node, &["radio"]) {
            for other in radio_group(doc, node) {
                if other != node {
                    self.checked.insert(other, false);
                }
            }
        }
    }

    /// Selectedness of an option, after the select's selectedness rules.
    pub fn option_selected(&self, doc: &Document, option: NodeId) -> bool {
        match option_select(doc, option) {
            Some(select) => self.selected_options(doc, select).contains(&option),
            None => self
                .selected
                .get(&option)
                .copied()
                .unwrap_or_else(|| doc.has_attr(option, "selected")),
        }
    }

    pub fn set_option_selected(&mut self, doc: &Document, option: NodeId, selected: bool) {
        if let Some(select) = option_select(doc, option) {
            if selected && !is_multiple(doc, select) {
                for o in select_options(doc, select) {
                    self.selected.insert(o, false);
                }
            }
            self.selected.insert(option, selected);
            self.sync_select(doc, select);
        } else {
            self.selected.insert(option, selected);
        }
    }

    /// The select's selected options in tree order. A single select keeps only the last
    /// selected option and, when shown as a drop-down whose selectedness nobody set, falls
    /// back to its first enabled option (the reset that inserting options runs); after
    /// `selectedIndex = -1` it stays empty.
    pub fn selected_options(&self, doc: &Document, select: NodeId) -> Vec<NodeId> {
        let options = select_options(doc, select);
        let mut flags: Vec<bool> = options
            .iter()
            .map(|o| {
                self.selected
                    .get(o)
                    .copied()
                    .unwrap_or_else(|| doc.has_attr(*o, "selected"))
            })
            .collect();
        if !is_multiple(doc, select) {
            match flags.iter().rposition(|&f| f) {
                Some(last) => {
                    for (i, f) in flags.iter_mut().enumerate() {
                        *f = i == last;
                    }
                }
                None if display_size(doc, select) == 1
                    && !options.iter().any(|o| self.selected.contains_key(o)) =>
                {
                    if let Some(i) = options.iter().position(|&o| !is_disabled(doc, o)) {
                        flags[i] = true;
                    }
                }
                None => {}
            }
        }
        options
            .into_iter()
            .zip(flags)
            .filter_map(|(o, f)| f.then_some(o))
            .collect()
    }

    /// The selected option index, or -1.
    pub fn selected_index(&self, doc: &Document, select: NodeId) -> i32 {
        let options = select_options(doc, select);
        self.selected_options(doc, select)
            .first()
            .and_then(|s| options.iter().position(|o| o == s))
            .map_or(-1, |i| i as i32)
    }

    fn sync_select(&mut self, doc: &Document, select: NodeId) {
        let value = self
            .selected_options(doc, select)
            .first()
            .map(|&o| option_value(doc, o))
            .unwrap_or_default();
        self.values.insert(select, value);
    }

    /// The user typed into a text control: `Backspace` or one character.
    pub fn edit_text(&mut self, doc: &Document, node: NodeId, key: &str) -> bool {
        if doc.has_attr(node, "readonly") || is_disabled(doc, node) {
            return false;
        }
        let mut value = self.value(doc, node);
        match key {
            "Backspace" => {
                if value.pop().is_none() {
                    return false;
                }
            }
            k if k.chars().count() == 1 => {
                if html_tag(doc, node) == Some("input") && (k == "\n" || k == "\r") {
                    return false;
                }
                let limit = doc
                    .attr(node, "maxlength")
                    .and_then(|m| m.trim().parse::<usize>().ok());
                if limit.is_some_and(|l| value.encode_utf16().count() >= l) {
                    return false;
                }
                value.push_str(k);
            }
            _ => return false,
        }
        self.values.insert(node, value);
        true
    }

    pub fn set_custom_validity(&mut self, node: NodeId, message: &str) {
        if message.is_empty() {
            self.custom_validity.remove(&node);
        } else {
            self.custom_validity.insert(node, message.to_string());
        }
    }

    /// Restore every control of `form` to its default state.
    pub fn reset(&mut self, doc: &Document, form: NodeId) {
        for c in form_controls(doc, form) {
            self.values.remove(&c);
            self.checked.remove(&c);
            self.custom_validity.remove(&c);
            if html_tag(doc, c) == Some("select") {
                for o in select_options(doc, c) {
                    self.selected.remove(&o);
                }
            }
        }
    }

    /// A control is a candidate for constraint validation.
    pub fn will_validate(&self, doc: &Document, node: NodeId) -> bool {
        let submittable = matches!(
            html_tag(doc, node),
            Some("button" | "input" | "select" | "textarea")
        );
        if !submittable || is_disabled(doc, node) {
            return false;
        }
        if doc
            .parent_chain(node)
            .iter()
            .any(|&a| html_tag(doc, a) == Some("datalist"))
        {
            return false;
        }
        match html_tag(doc, node) {
            Some("input") => {
                let ty = input_type(doc, node);
                if matches!(ty.as_str(), "hidden" | "reset" | "button") {
                    return false;
                }
                !(TEXT_TYPES.contains(&ty.as_str()) && doc.has_attr(node, "readonly"))
            }
            Some("button") => is_submit_button(doc, node),
            Some("textarea") => !doc.has_attr(node, "readonly"),
            _ => true,
        }
    }

    /// Why the control is invalid, if it is.
    pub fn invalidity(&self, doc: &Document, node: NodeId) -> Option<Invalidity> {
        if !self.will_validate(doc, node) {
            return None;
        }
        if self.custom_validity.contains_key(&node) {
            return Some(Invalidity::CustomError);
        }
        let required = doc.has_attr(node, "required");
        match html_tag(doc, node) {
            Some("input") => {
                let ty = input_type(doc, node);
                match ty.as_str() {
                    "checkbox" if required && !self.checked(doc, node) => {
                        Some(Invalidity::ValueMissing)
                    }
                    "radio" => {
                        let group = radio_group(doc, node);
                        let any_required = group.iter().any(|&r| doc.has_attr(r, "required"));
                        let any_checked = group.iter().any(|&r| self.checked(doc, r));
                        (any_required && !any_checked).then_some(Invalidity::ValueMissing)
                    }
                    "file" if required => Some(Invalidity::ValueMissing),
                    t if TEXT_TYPES.contains(&t) => {
                        let value = self.value(doc, node);
                        if value.is_empty() {
                            return required.then_some(Invalidity::ValueMissing);
                        }
                        let mismatch = match t {
                            "email" => {
                                let valid = |v: &str| {
                                    v.split_once('@').is_some_and(|(l, d)| {
                                        !l.is_empty()
                                            && !d.is_empty()
                                            && !v.contains(char::is_whitespace)
                                            && !d.contains('@')
                                    })
                                };
                                if doc.has_attr(node, "multiple") {
                                    !value.split(',').all(|v| valid(v.trim()))
                                } else {
                                    !valid(&value)
                                }
                            }
                            "url" => Url::parse(&value).is_err(),
                            _ => false,
                        };
                        mismatch.then_some(Invalidity::TypeMismatch)
                    }
                    _ => None,
                }
            }
            Some("textarea") if required && self.value(doc, node).is_empty() => {
                Some(Invalidity::ValueMissing)
            }
            Some("select") if required => {
                let selected = self.selected_options(doc, node);
                let placeholder = placeholder_option(doc, node);
                let missing = selected.is_empty()
                    || (selected.len() == 1 && placeholder.is_some_and(|p| selected[0] == p));
                missing.then_some(Invalidity::ValueMissing)
            }
            _ => None,
        }
    }

    /// The message a browser would show for an invalid control.
    pub fn validation_message(&self, doc: &Document, node: NodeId) -> String {
        match self.invalidity(doc, node) {
            None => String::new(),
            Some(Invalidity::CustomError) => {
                self.custom_validity.get(&node).cloned().unwrap_or_default()
            }
            Some(Invalidity::ValueMissing) => {
                if is_input_of(doc, node, &["checkbox"]) {
                    "Please check this box if you want to proceed.".into()
                } else if is_input_of(doc, node, &["radio"]) {
                    "Please select one of these options.".into()
                } else if html_tag(doc, node) == Some("select") {
                    "Please select an item in the list.".into()
                } else if is_input_of(doc, node, &["file"]) {
                    "Please select a file.".into()
                } else {
                    "Please fill out this field.".into()
                }
            }
            Some(Invalidity::TypeMismatch) => {
                if is_input_of(doc, node, &["email"]) {
                    "Please enter an email address.".into()
                } else {
                    "Please enter a URL.".into()
                }
            }
        }
    }

    /// The form's controls that fail constraint validation, in tree order.
    pub fn invalid_controls(&self, doc: &Document, form: NodeId) -> Vec<NodeId> {
        form_controls(doc, form)
            .into_iter()
            .filter(|&c| self.invalidity(doc, c).is_some())
            .collect()
    }

    /// The form data set (HTML "constructing the entry list").
    pub fn entry_list(
        &self,
        doc: &Document,
        form: NodeId,
        submitter: Option<NodeId>,
    ) -> Vec<(String, EntryValue)> {
        let mut out = Vec::new();
        for c in form_controls(doc, form) {
            let tag = html_tag(doc, c).unwrap_or("");
            if !matches!(tag, "button" | "input" | "select" | "textarea") {
                continue;
            }
            if doc
                .parent_chain(c)
                .iter()
                .any(|&a| html_tag(doc, a) == Some("datalist"))
                || is_disabled(doc, c)
                || (is_button(doc, c) && Some(c) != submitter)
            {
                continue;
            }
            let ty = if tag == "input" {
                input_type(doc, c)
            } else {
                String::new()
            };
            if matches!(ty.as_str(), "checkbox" | "radio") && !self.checked(doc, c) {
                continue;
            }
            if ty == "image" {
                let name = doc.attr(c, "name").unwrap_or("");
                let prefix = if name.is_empty() {
                    String::new()
                } else {
                    format!("{name}.")
                };
                out.push((format!("{prefix}x"), EntryValue::Text("0".into())));
                out.push((format!("{prefix}y"), EntryValue::Text("0".into())));
                continue;
            }
            let name = match doc.attr(c, "name") {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            match tag {
                "select" => {
                    for o in self.selected_options(doc, c) {
                        if !is_disabled(doc, o) {
                            out.push((name.clone(), EntryValue::Text(option_value(doc, o))));
                        }
                    }
                }
                "input" if ty == "file" => out.push((
                    name.clone(),
                    EntryValue::File {
                        filename: String::new(),
                    },
                )),
                "input"
                    if ty == "hidden"
                        && name.eq_ignore_ascii_case("_charset_")
                        && !doc.has_attr(c, "value") =>
                {
                    out.push((name.clone(), EntryValue::Text("UTF-8".into())));
                }
                _ => out.push((name.clone(), EntryValue::Text(self.value(doc, c)))),
            }
            let dirname = doc.attr(c, "dirname").filter(|d| !d.is_empty());
            if let Some(dirname) = dirname {
                if tag == "textarea" || matches!(ty.as_str(), "text" | "search") {
                    out.push((dirname.to_string(), EntryValue::Text("ltr".into())));
                }
            }
        }
        out
    }

    /// Plan the navigation for submitting `form` (the entry list is captured now, as the
    /// HTML submit algorithm does). `Err` explains why nothing will navigate.
    pub fn plan_submission(
        &self,
        doc: &mut Document,
        form: NodeId,
        submitter: Option<NodeId>,
        document_url: &str,
    ) -> Result<Option<FormSubmission>, String> {
        if !is_connected(doc, form) {
            return Err("the form is not connected".into());
        }
        let submitter = submitter.filter(|&s| is_submit_button(doc, s));
        let from_submitter = |attr: &str| submitter.and_then(|s| doc.attr(s, attr));
        let method = from_submitter("formmethod")
            .or_else(|| doc.attr(form, "method"))
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if method == "dialog" {
            let dialog = doc
                .parent_chain(form)
                .into_iter()
                .rev()
                .find(|&a| html_tag(doc, a) == Some("dialog"));
            if let Some(dialog) = dialog {
                doc.remove_attr(dialog, "open");
            }
            return Ok(None);
        }
        let method = if method == "post" {
            FormMethod::Post
        } else {
            FormMethod::Get
        };
        let enctype = from_submitter("formenctype")
            .or_else(|| doc.attr(form, "enctype"))
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let action = from_submitter("formaction")
            .or_else(|| doc.attr(form, "action"))
            .unwrap_or("")
            .to_string();
        if let Some(scheme) = url_scheme(&action) {
            if !matches!(scheme.as_str(), "http" | "https") {
                return Err(format!(
                    "submitting a form to a {scheme}: URL is not supported"
                ));
            }
        }
        let base = self.base_url.as_deref().unwrap_or(document_url);
        let resolved = if action.trim().is_empty() {
            Url::parse(document_url)
        } else {
            Url::parse(base).and_then(|b| b.join(&action))
        };
        let mut url =
            resolved.map_err(|_| format!("the form action {action:?} is not a valid URL"))?;
        if !matches!(url.scheme.as_str(), "http" | "https") {
            return Err(format!(
                "submitting a form to a {}: URL is not supported",
                url.scheme
            ));
        }
        let entries = self.entry_list(doc, form, submitter);
        let body = match method {
            FormMethod::Get => {
                url.query = Some(urlencode(&entries));
                None
            }
            FormMethod::Post => Some(match enctype.as_str() {
                "multipart/form-data" => {
                    let boundary = multipart_boundary();
                    (
                        multipart(&entries, &boundary),
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                }
                "text/plain" => (text_plain(&entries).into_bytes(), "text/plain".to_string()),
                _ => (
                    urlencode(&entries).into_bytes(),
                    "application/x-www-form-urlencoded".to_string(),
                ),
            }),
        };
        Ok(Some(FormSubmission {
            url: url.as_str(),
            method,
            body,
            document_url: document_url.to_string(),
        }))
    }

    /// Activation behavior for a click along `path` (root first) when the page has no
    /// script realm: toggle checkboxes, check radio buttons, follow labels, reset forms and
    /// queue submissions — blocked while a control is invalid, as interactive validation
    /// would. Returns whether control state changed or a submission was queued.
    pub fn activate_without_script(
        &mut self,
        doc: &mut Document,
        path: &[NodeId],
        document_url: &str,
    ) -> bool {
        let Some(mut el) = path.iter().rev().copied().find(|&n| {
            is_checkable(doc, n) || is_button(doc, n) || html_tag(doc, n) == Some("label")
        }) else {
            return false;
        };
        if html_tag(doc, el) == Some("label") {
            match label_control(doc, el) {
                Some(control) => el = control,
                None => return false,
            }
        }
        if is_disabled(doc, el) {
            return false;
        }
        if is_checkable(doc, el) {
            let checked = input_type(doc, el) == "radio" || !self.checked(doc, el);
            self.set_checked(doc, el, checked);
            return true;
        }
        let Some(form) = form_owner(doc, el) else {
            return false;
        };
        if is_input_of(doc, el, &["reset"])
            || (html_tag(doc, el) == Some("button")
                && doc
                    .attr(el, "type")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("reset")))
        {
            self.reset(doc, form);
            return true;
        }
        if !is_submit_button(doc, el) {
            return false;
        }
        self.submit_without_script(doc, form, Some(el), document_url)
    }

    /// Implicit submission (Enter in a field) when the page has no script realm: activate
    /// the form's default button, or submit a form without one when at most one field
    /// blocks implicit submission.
    pub fn implicit_submit_without_script(
        &mut self,
        doc: &mut Document,
        node: NodeId,
        document_url: &str,
    ) -> bool {
        let Some(form) = form_owner(doc, node) else {
            return false;
        };
        if let Some(button) = default_button(doc, form) {
            let path = doc.parent_chain(button);
            return self.activate_without_script(doc, &path, document_url);
        }
        let blocking = form_controls(doc, form)
            .into_iter()
            .filter(|&c| is_input_of(doc, c, TEXT_TYPES))
            .count();
        blocking <= 1 && self.submit_without_script(doc, form, None, document_url)
    }

    fn submit_without_script(
        &mut self,
        doc: &mut Document,
        form: NodeId,
        submitter: Option<NodeId>,
        document_url: &str,
    ) -> bool {
        let novalidate = doc.has_attr(form, "novalidate")
            || submitter.is_some_and(|s| doc.has_attr(s, "formnovalidate"));
        if !novalidate && !self.invalid_controls(doc, form).is_empty() {
            log::info!("form not submitted: a control is invalid");
            return false;
        }
        match self.plan_submission(doc, form, submitter, document_url) {
            Ok(Some(submission)) => {
                self.queue_submission(submission);
                true
            }
            Ok(None) => true,
            Err(e) => {
                log::info!("form not submitted: {e}");
                false
            }
        }
    }

    /// Queue a planned navigation; a later submission replaces an earlier one.
    pub fn queue_submission(&mut self, submission: FormSubmission) {
        self.submission = Some(submission);
    }

    /// Record the base URL the loader froze from the document's `<base href>`.
    pub fn set_base_url(&mut self, base: String) {
        self.base_url = Some(base);
    }

    pub fn take_submission(&mut self) -> Option<FormSubmission> {
        self.submission.take()
    }

    pub fn has_pending_submission(&self) -> bool {
        self.submission.is_some()
    }
}

/// The lowercased scheme of an absolute URL string, if it has one.
fn url_scheme(s: &str) -> Option<String> {
    let (scheme, _) = s.trim().split_once(':')?;
    let valid = scheme.chars().next()?.is_ascii_alphabetic()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    valid.then(|| scheme.to_ascii_lowercase())
}

fn textarea_default(doc: &Document, node: NodeId) -> String {
    doc.get(node)
        .children
        .iter()
        .filter(|&&c| !doc.is_element(c))
        .map(|&c| doc.text_content(c))
        .collect()
}

/// The select's placeholder label option: a required, single, drop-down select whose
/// first option is a direct child with an empty value.
fn placeholder_option(doc: &Document, select: NodeId) -> Option<NodeId> {
    if is_multiple(doc, select) || display_size(doc, select) != 1 {
        return None;
    }
    let first = select_options(doc, select).into_iter().next()?;
    (doc.get(first).parent == Some(select) && option_value(doc, first).is_empty()).then_some(first)
}

/// The radio buttons in `radio`'s group: same tree, same form owner, same non-empty name.
fn radio_group(doc: &Document, radio: NodeId) -> Vec<NodeId> {
    let name = match doc.attr(radio, "name") {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return vec![radio],
    };
    let owner = form_owner(doc, radio);
    doc.descendants(tree_root(doc, radio))
        .into_iter()
        .filter(|&n| {
            is_input_of(doc, n, &["radio"])
                && doc.attr(n, "name") == Some(name.as_str())
                && form_owner(doc, n) == owner
        })
        .collect()
}

fn entry_text(value: &EntryValue) -> &str {
    match value {
        EntryValue::Text(t) => t,
        EntryValue::File { filename } => filename,
    }
}

fn crlf(s: &str) -> String {
    normalize_newlines(s).replace('\n', "\r\n")
}

/// `application/x-www-form-urlencoded` serialization (UTF-8).
pub fn urlencode(entries: &[(String, EntryValue)]) -> String {
    fn encode(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'*' | b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (name, value)) in entries.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode(&crlf(name), &mut out);
        out.push('=');
        encode(&crlf(entry_text(value)), &mut out);
    }
    out
}

/// `text/plain` serialization.
pub fn text_plain(entries: &[(String, EntryValue)]) -> String {
    entries
        .iter()
        .map(|(n, v)| format!("{}={}\r\n", crlf(n), crlf(entry_text(v))))
        .collect()
}

fn multipart_boundary() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 12];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        bytes.copy_from_slice(&nanos.to_le_bytes()[..12]);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("----AxiomFormBoundary{hex}")
}

/// `multipart/form-data` serialization with `boundary`.
pub fn multipart(entries: &[(String, EntryValue)], boundary: &str) -> Vec<u8> {
    fn escape(s: &str) -> String {
        s.replace('\n', "%0A")
            .replace('\r', "%0D")
            .replace('"', "%22")
    }
    let mut out = String::new();
    for (name, value) in entries {
        out.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
            escape(&crlf(name))
        ));
        match value {
            EntryValue::Text(t) => out.push_str(&format!("\r\n\r\n{}\r\n", crlf(t))),
            EntryValue::File { filename } => out.push_str(&format!(
                "; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n",
                escape(filename)
            )),
        }
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(html: &str) -> Document {
        let mut doc = Document::new();
        let mut parser = axiom_html::HtmlParser::new();
        parser.feed(html);
        parser.finish();
        while !matches!(
            parser.step(&mut doc),
            axiom_html::ParseStep::NeedData | axiom_html::ParseStep::Done
        ) {}
        doc
    }

    fn by_id(doc: &Document, id: &str) -> NodeId {
        doc.get_element_by_id(id).expect("element")
    }

    fn texts(entries: &[(String, EntryValue)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(n, v)| (n.clone(), entry_text(v).to_string()))
            .collect()
    }

    #[test]
    fn entry_list_follows_the_html_rules() {
        let doc = parse(
            "<form id=f>\
             <input name=q value='a b'>\
             <input name=nameless-skipped value=x disabled>\
             <input value=no-name>\
             <input type=checkbox name=c1 checked><input type=checkbox name=c2 value=v2>\
             <input type=radio name=r value=1><input type=radio name=r value=2 checked>\
             <select name=s><option>one<option selected value=2>two</select>\
             <select name=m multiple><option selected>x<option selected disabled>y<option selected>z</select>\
             <textarea name=t>line1\r\nline2</textarea>\
             <fieldset disabled><input name=in-disabled-fieldset value=1>\
             <legend><input name=in-legend value=ok></legend></fieldset>\
             <input type=hidden name=_charset_>\
             <button name=b value=clicked id=b>Go</button><input type=submit name=other value=no>\
             </form><input form=f name=outside value=yes>",
        );
        let st = FormState::default();
        let f = by_id(&doc, "f");
        let b = by_id(&doc, "b");
        assert_eq!(
            texts(&st.entry_list(&doc, f, Some(b))),
            [
                ("q", "a b"),
                ("c1", "on"),
                ("r", "2"),
                ("s", "2"),
                ("m", "x"),
                ("m", "z"),
                ("t", "line1\nline2"),
                ("in-legend", "ok"),
                ("_charset_", "UTF-8"),
                ("b", "clicked"),
                ("outside", "yes"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }

    #[test]
    fn a_fieldset_s_first_legend_is_not_disabled() {
        let doc = parse(
            "<fieldset disabled><legend><input id=a></legend><legend><input id=b></legend>\
             <input id=c></fieldset>",
        );
        assert!(!is_disabled(&doc, by_id(&doc, "a")));
        assert!(is_disabled(&doc, by_id(&doc, "b")));
        assert!(is_disabled(&doc, by_id(&doc, "c")));
    }

    #[test]
    fn encodings() {
        let entries = vec![
            ("a b".to_string(), EntryValue::Text("x&y=z\n~é".into())),
            (
                "f\"".to_string(),
                EntryValue::File {
                    filename: String::new(),
                },
            ),
        ];
        assert_eq!(urlencode(&entries), "a+b=x%26y%3Dz%0D%0A%7E%C3%A9&f%22=");
        assert_eq!(text_plain(&entries), "a b=x&y=z\r\n~é\r\nf\"=\r\n");
        let body = String::from_utf8(multipart(&entries, "BB")).unwrap();
        assert_eq!(
            body,
            "--BB\r\nContent-Disposition: form-data; name=\"a b\"\r\n\r\nx&y=z\r\n~é\r\n\
             --BB\r\nContent-Disposition: form-data; name=\"f%22\"; filename=\"\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\r\n--BB--\r\n"
        );
    }

    #[test]
    fn radios_in_a_group_are_exclusive_and_selects_keep_one_option() {
        let doc = parse(
            "<form><input type=radio name=r id=r1 checked><input type=radio name=r id=r2></form>\
             <input type=radio name=r id=r3 checked>\
             <select id=s><option id=o1>a<option id=o2 value=b>b</select>",
        );
        let mut st = FormState::default();
        let (r1, r2, r3) = (by_id(&doc, "r1"), by_id(&doc, "r2"), by_id(&doc, "r3"));
        st.set_checked(&doc, r2, true);
        assert!(!st.checked(&doc, r1) && st.checked(&doc, r2));
        assert!(
            st.checked(&doc, r3),
            "a radio outside the form is another group"
        );
        let (s, o1, o2) = (by_id(&doc, "s"), by_id(&doc, "o1"), by_id(&doc, "o2"));
        assert_eq!(st.selected_options(&doc, s), [o1]);
        st.set_option_selected(&doc, o2, true);
        assert_eq!(st.selected_options(&doc, s), [o2]);
        assert_eq!(st.value(&doc, s), "b");
        assert_eq!(st.values.get(&s).map(String::as_str), Some("b"));
    }

    #[test]
    fn validation_reports_missing_and_mismatched_values() {
        let doc = parse(
            "<form id=f><input id=t required><input id=e type=email value=nope>\
             <input id=ok type=email value=a@b.test><input type=checkbox id=c required>\
             <select id=s required><option value=''>Pick<option>x</select>\
             <input id=ro required readonly><input id=d required disabled></form>",
        );
        let st = FormState::default();
        let f = by_id(&doc, "f");
        let ids: Vec<NodeId> = ["t", "e", "c", "s"]
            .iter()
            .map(|i| by_id(&doc, i))
            .collect();
        assert_eq!(st.invalid_controls(&doc, f), ids);
        assert_eq!(
            st.validation_message(&doc, by_id(&doc, "e")),
            "Please enter an email address."
        );
    }

    #[test]
    fn plans_get_and_post_navigations() {
        let mut doc = parse(
            "<form id=f action='search?old=1#frag'><input name=q value='a b'>\
             <button id=post formmethod=post formaction='/submit' formenctype=text/plain>p</button></form>",
        );
        let mut st = FormState::default();
        st.set_base_url("https://site.test/app/".into());
        let f = by_id(&doc, "f");
        let get = st
            .plan_submission(&mut doc, f, None, "https://site.test/page")
            .unwrap()
            .unwrap();
        assert_eq!(get.url, "https://site.test/app/search?q=a+b#frag");
        assert_eq!((get.method, get.body.is_none()), (FormMethod::Get, true));
        let post_button = by_id(&doc, "post");
        let post = st
            .plan_submission(&mut doc, f, Some(post_button), "https://site.test/page")
            .unwrap()
            .unwrap();
        assert_eq!(post.url, "https://site.test/submit");
        assert_eq!(post.method, FormMethod::Post);
        assert_eq!(
            post.body,
            Some((b"q=a b\r\n".to_vec(), "text/plain".to_string()))
        );
    }

    #[test]
    fn javascript_actions_never_navigate() {
        let mut doc = parse("<form id=f action='javascript:alert(1)'><input name=q></form>");
        let f = by_id(&doc, "f");
        let st = FormState::default();
        assert!(st
            .plan_submission(&mut doc, f, None, "https://site.test/")
            .is_err());
    }
}
