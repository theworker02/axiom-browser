//! ES module scripts: a module map keyed by URL, imports fetched by the host, and
//! top-level module jobs (fetch the graph, link, evaluate).
//!
//! Boa asks [`AxiomModuleLoader`] for each import. Modules already in the map complete
//! at once; new URLs are queued for the host ([`crate::JsContext::take_module_fetches`])
//! and complete when it delivers the source ([`crate::JsContext::module_fetched`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::{ModuleLoader, Referrer};
use boa_engine::object::builtins::JsPromise;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, Module,
    Source,
};

use crate::HostCell;

type FinishLoad = Box<dyn FnOnce(JsResult<Module>, &mut Context)>;

/// Start of the message of every module specifier resolution failure.
pub const RESOLVE_FAILURE: &str = "Failed to resolve module specifier";

/// A specifier HTML resolves as a URL: absolute, or starting with `/`, `./` or `../`.
/// Anything else is a bare specifier, which only an import map can resolve.
pub fn is_url_like_specifier(specifier: &str) -> bool {
    specifier.starts_with('/')
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier
            .split_once(':')
            .is_some_and(|(scheme, _)| is_scheme(scheme))
}

pub(crate) fn bare_specifier_error(specifier: &str) -> String {
    format!(
        "{RESOLVE_FAILURE} \"{specifier}\": relative references must start with \"/\", \"./\" or \"../\""
    )
}

/// Progress of a top-level module script job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleOutcome {
    /// The whole graph was fetched and parsed; the host runs it with
    /// [`crate::JsContext::run_module`] when its turn comes.
    Ready,
    /// The module graph evaluated (a thrown exception is `Error`).
    Evaluated,
    /// A module in the graph could not be fetched or resolved: the script element gets
    /// an `error` event.
    FetchFailed(String),
    /// Parsing, linking or evaluation threw; already reported to the window.
    Error(String),
}

#[derive(Default)]
pub(crate) struct ModuleMap {
    modules: HashMap<String, Module>,
    failed: HashMap<String, String>,
    waiting: HashMap<String, Vec<FinishLoad>>,
    fetches: Vec<String>,
}

pub(crate) struct AxiomModuleLoader {
    pub(crate) map: Rc<RefCell<ModuleMap>>,
    pub(crate) host: HostCell,
}

/// The URL a module was fetched from (its base for relative imports).
fn module_url(module: &Module) -> Option<String> {
    module.path().and_then(Path::to_str).map(str::to_string)
}

fn type_error(message: String) -> JsError {
    JsNativeError::typ().with_message(message).into()
}

impl AxiomModuleLoader {
    fn resolve(&self, base: Option<&str>, specifier: &str) -> Result<String, String> {
        self.host
            .borrow_mut()
            .resolve_module_specifier(base, specifier)
    }
}

fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

impl ModuleLoader for AxiomModuleLoader {
    fn load_imported_module(
        &self,
        referrer: Referrer,
        specifier: JsString,
        finish_load: FinishLoad,
        context: &mut Context,
    ) {
        let base = match &referrer {
            Referrer::Module(m) => module_url(m),
            _ => None,
        };
        let url = match self.resolve(base.as_deref(), &specifier.to_std_string_escaped()) {
            Ok(url) => url,
            Err(message) => return finish_load(Err(type_error(message)), context),
        };
        let mut map = self.map.borrow_mut();
        if let Some(module) = map.modules.get(&url).cloned() {
            drop(map);
            return finish_load(Ok(module), context);
        }
        if let Some(message) = map.failed.get(&url).cloned() {
            drop(map);
            return finish_load(Err(type_error(message)), context);
        }
        let map = &mut *map;
        let waiters = map.waiting.entry(url.clone()).or_default();
        if waiters.is_empty() {
            map.fetches.push(url);
        }
        waiters.push(finish_load);
    }

    fn init_import_meta(&self, import_meta: &JsObject, module: &Module, context: &mut Context) {
        if let Some(url) = module_url(module) {
            let _ = import_meta.set(
                js_string!("url"),
                JsValue::from(JsString::from(url.as_str())),
                false,
                context,
            );
        }
    }
}

enum Stage {
    Loading(JsPromise),
    /// Graph loaded; waiting for the host to run it.
    Loaded,
    Evaluating(JsPromise),
}

pub(crate) struct ModuleJob {
    pub(crate) id: u32,
    module: Module,
    stage: Stage,
}

impl ModuleJob {
    /// Still fetching its graph, or ready and waiting to run.
    pub(crate) fn is_pending(&self) -> bool {
        !matches!(self.stage, Stage::Evaluating(_))
    }
}

impl ModuleMap {
    pub(crate) fn take_fetches(&mut self) -> Vec<String> {
        std::mem::take(&mut self.fetches)
    }

    pub(crate) fn is_waiting(&self) -> bool {
        !self.waiting.is_empty() || !self.fetches.is_empty()
    }
}

/// Parse `source` as the module at `url`; fetched modules (not inline ones) go into the
/// module map so later imports of `url` share them.
pub(crate) fn parse_module(
    map: &Rc<RefCell<ModuleMap>>,
    url: &str,
    source: &str,
    register: bool,
    context: &mut Context,
) -> JsResult<Module> {
    let module = Module::parse(
        Source::from_bytes(source).with_path(Path::new(url)),
        None,
        context,
    )?;
    if register {
        map.borrow_mut()
            .modules
            .insert(url.to_string(), module.clone());
    }
    Ok(module)
}

pub(crate) fn start_job(id: u32, module: Module, context: &mut Context) -> ModuleJob {
    let stage = Stage::Loading(module.load(context));
    ModuleJob { id, module, stage }
}

/// The host delivered module `url` (`Ok((response_url, source))`) or failed to.
pub(crate) fn deliver(
    map: &Rc<RefCell<ModuleMap>>,
    url: &str,
    result: Result<(String, String), String>,
    context: &mut Context,
) {
    let waiters = map.borrow_mut().waiting.remove(url).unwrap_or_default();
    let outcome: Result<Module, String> = match result {
        Ok((response_url, source)) => {
            match parse_module(map, &response_url, &source, true, context) {
                Ok(module) => {
                    map.borrow_mut()
                        .modules
                        .insert(url.to_string(), module.clone());
                    Ok(module)
                }
                Err(e) => {
                    // A parse error is a script error: every importer's graph fails with it.
                    map.borrow_mut()
                        .failed
                        .insert(url.to_string(), e.to_string());
                    for finish in waiters {
                        finish(Err(e.clone()), context);
                    }
                    return;
                }
            }
        }
        Err(message) => {
            let message = format!("Failed to fetch module {url}: {message}");
            map.borrow_mut()
                .failed
                .insert(url.to_string(), message.clone());
            Err(message)
        }
    };
    for finish in waiters {
        match &outcome {
            Ok(module) => finish(Ok(module.clone()), context),
            Err(message) => finish(Err(type_error(message.clone())), context),
        }
    }
}

/// Advance `job`; `Some` when it has news (ready to run, or finished).
pub(crate) fn poll_job(job: &mut ModuleJob, context: &mut Context) -> Option<ModuleOutcome> {
    match &job.stage {
        Stage::Loading(p) => match p.state() {
            PromiseState::Pending => None,
            PromiseState::Rejected(err) => {
                let message = describe(&err, context);
                // Fetch failures fail the script element; syntax errors and
                // unresolvable specifiers in the graph are script errors.
                let script_error =
                    is_syntax_error(&err, context) || message.contains(RESOLVE_FAILURE);
                Some(if script_error {
                    report(&err, context);
                    ModuleOutcome::Error(format!("Uncaught {message}"))
                } else {
                    ModuleOutcome::FetchFailed(message)
                })
            }
            PromiseState::Fulfilled(_) => {
                job.stage = Stage::Loaded;
                Some(ModuleOutcome::Ready)
            }
        },
        Stage::Loaded => None,
        Stage::Evaluating(p) => match p.state() {
            PromiseState::Pending => None,
            PromiseState::Fulfilled(_) => Some(ModuleOutcome::Evaluated),
            PromiseState::Rejected(err) => {
                report(&err, context);
                Some(ModuleOutcome::Error(format!(
                    "Uncaught {}",
                    describe(&err, context)
                )))
            }
        },
    }
}

/// Link and evaluate a loaded job. `Some` when it ended at once (a link error).
pub(crate) fn run_job(job: &mut ModuleJob, context: &mut Context) -> Option<ModuleOutcome> {
    if !matches!(job.stage, Stage::Loaded) {
        return None;
    }
    if let Err(e) = job.module.link(context) {
        let err = e.to_opaque(context);
        report(&err, context);
        return Some(ModuleOutcome::Error(format!(
            "Uncaught {}",
            describe(&err, context)
        )));
    }
    job.stage = Stage::Evaluating(job.module.evaluate(context));
    None
}

fn describe(err: &JsValue, context: &mut Context) -> String {
    if let Some(obj) = err.as_object() {
        let name = obj
            .get(js_string!("name"), context)
            .ok()
            .and_then(|v| v.as_string().map(JsString::to_std_string_escaped));
        let message = obj
            .get(js_string!("message"), context)
            .ok()
            .and_then(|v| v.as_string().map(JsString::to_std_string_escaped));
        if let (Some(name), Some(message)) = (name, message) {
            return format!("{name}: {message}");
        }
    }
    err.to_string(context)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|_| "<value>".into())
}

fn is_syntax_error(err: &JsValue, context: &mut Context) -> bool {
    err.as_object()
        .and_then(|o| o.get(js_string!("name"), context).ok())
        .and_then(|v| v.as_string().map(JsString::to_std_string_escaped))
        .is_some_and(|n| n == "SyntaxError")
}

/// "Report the exception": an `error` event at the window.
fn report(err: &JsValue, context: &mut Context) {
    let global = context.global_object();
    if let Ok(f) = global.get(js_string!("__axiom_reportError"), context) {
        if let Some(f) = f.as_callable() {
            let _ = f.call(&JsValue::undefined(), std::slice::from_ref(err), context);
        }
    }
}
