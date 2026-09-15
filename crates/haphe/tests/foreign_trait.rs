//! `#[script(foreign)]` on traits: descriptor generation through the
//! `ScriptForeign` trait, and runtime dispatch through a `ForeignCaller`.

#![cfg(feature = "macros")]

use std::cell::RefCell;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use haphe::{
    ForeignCaller, ForeignError, ForeignErrorKind, ForeignHandle, ForeignInterfaceDescriptor,
    FunctionDescriptor, Ownership, ParamDescriptor, PrimitiveType, Receiver, ScriptForeign,
    ScriptValue, ThreadSafety, TypeDescriptor, TypeId, script,
};

/// Host-side hooks.
#[script(foreign, thread_safety = none)]
pub trait HostHooks {
    /// Logs a message on the host.
    fn log(&self, message: String);

    fn add(&self, a: i32, b: i32) -> i32;

    #[script(rename = "get_env", error_kind = "EnvError")]
    fn env(&self, key: String) -> Result<String, HooksError>;

    async fn fetch(&self, url: String) -> Result<String, HooksError>;
}

#[derive(Debug, PartialEq)]
pub enum HooksError {
    Foreign(String),
}

impl From<ForeignError> for HooksError {
    fn from(e: ForeignError) -> Self {
        Self::Foreign(e.to_string())
    }
}

// ── Expected descriptor

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);

static EXPECTED_ADD: FunctionDescriptor<'static> = FunctionDescriptor {
    name: "add",
    doc: None,
    receiver: Some(Receiver::Ref),
    params: &[
        ParamDescriptor {
            name: "a",
            ty: &I32,
            ownership: Ownership::Owned,
        },
        ParamDescriptor {
            name: "b",
            ty: &I32,
            ownership: Ownership::Owned,
        },
    ],
    return_type: &I32,
    return_ownership: Ownership::Owned,
    is_async: false,
    error_kind: None,
};

/// Descriptors are usable in static initializers (`registry!` relies on this).
static IN_STATIC: ForeignInterfaceDescriptor<'static> =
    <HostHooksHandle as ScriptForeign>::DESCRIPTOR;

#[test]
fn descriptor_matches_hand_written() {
    let desc = IN_STATIC;
    assert!(desc.id == TypeId::new(concat!(module_path!(), "::HostHooks")));
    assert_eq!(desc.name, "HostHooks");
    assert_eq!(desc.doc, Some("Host-side hooks."));
    assert_eq!(desc.thread_safety, ThreadSafety::NONE);
    assert_eq!(desc.functions.len(), 4);

    assert_eq!(desc.functions[1], EXPECTED_ADD);

    let log = &desc.functions[0];
    assert_eq!(log.name, "log");
    assert_eq!(log.doc, Some("Logs a message on the host."));
    assert_eq!(*log.return_type, TypeDescriptor::Unit);

    let env = &desc.functions[2];
    assert_eq!(env.name, "get_env");
    assert_eq!(env.error_kind, Some("EnvError"));
    assert!(!env.is_async);

    let fetch = &desc.functions[3];
    assert_eq!(fetch.name, "fetch");
    assert!(fetch.is_async);
}

// ── Runtime dispatch through a mock caller

/// Records calls and replies from a canned response table.
struct MockCaller {
    calls: RefCell<Vec<(String, Vec<ScriptValue>)>>,
    respond: fn(&str, &[ScriptValue]) -> Result<ScriptValue, ForeignError>,
}

impl ForeignCaller for MockCaller {
    fn call(
        &self,
        function: &'static str,
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        self.calls
            .borrow_mut()
            .push((function.to_string(), args.to_vec()));
        (self.respond)(function, args)
    }

    fn call_async<'a>(
        &'a self,
        function: &'static str,
        args: &'a [ScriptValue],
    ) -> Pin<Box<dyn Future<Output = Result<ScriptValue, ForeignError>> + 'a>> {
        Box::pin(std::future::ready(self.call(function, args)))
    }
}

fn host_error(function: &'static str, message: &str) -> ForeignError {
    ForeignError {
        function,
        kind: ForeignErrorKind::Call(message.to_string().into()),
    }
}

fn handle(
    respond: fn(&str, &[ScriptValue]) -> Result<ScriptValue, ForeignError>,
) -> HostHooksHandle {
    HostHooksHandle::from_caller(Box::new(MockCaller {
        calls: RefCell::new(Vec::new()),
        respond,
    }))
}

fn block_on<F: Future>(mut fut: F) -> F::Output {
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

#[test]
fn sync_calls_dispatch_with_converted_args() {
    let hooks = handle(|function, args| match function {
        "log" => Ok(ScriptValue::Unit),
        "add" => {
            let (ScriptValue::I64(a), ScriptValue::I64(b)) = (&args[0], &args[1]) else {
                panic!("expected integer args");
            };
            Ok(ScriptValue::I64(a + b))
        }
        other => Err(host_error("add", other)),
    });
    hooks.log("hello".to_string());
    assert_eq!(hooks.add(2, 3), 5);
}

#[test]
fn result_method_surfaces_host_errors() {
    let hooks = handle(|function, _| match function {
        "get_env" => Err(host_error("get_env", "no such key")),
        _ => Ok(ScriptValue::Unit),
    });
    let err = hooks.env("MISSING".to_string()).unwrap_err();
    let HooksError::Foreign(message) = err;
    assert!(message.contains("get_env"));
    assert!(message.contains("no such key"));
}

#[test]
fn result_method_surfaces_conversion_errors() {
    let hooks = handle(|_, _| Ok(ScriptValue::I64(7)));
    let err = hooks.env("HOME".to_string()).unwrap_err();
    let HooksError::Foreign(message) = err;
    assert!(message.contains("unexpected value"));
}

#[test]
#[should_panic(expected = "foreign function `add` failed")]
fn non_result_method_panics_on_host_error() {
    let hooks = handle(|_, _| Err(host_error("add", "boom")));
    hooks.add(1, 1);
}

#[test]
#[should_panic(expected = "unexpected value")]
fn non_result_method_panics_on_conversion_error() {
    let hooks = handle(|_, _| Ok(ScriptValue::String("not a number".to_string())));
    hooks.add(1, 1);
}

#[test]
fn async_method_dispatches_through_call_async() {
    let hooks = handle(|function, args| match function {
        "fetch" => {
            let ScriptValue::String(url) = &args[0] else {
                panic!("expected string arg");
            };
            Ok(ScriptValue::String(format!("body of {url}")))
        }
        other => Err(host_error("fetch", other)),
    });
    let body = block_on(hooks.fetch("http://example".to_string())).unwrap();
    assert_eq!(body, "body of http://example");
}

#[test]
fn default_call_async_reports_async_unsupported() {
    struct SyncOnly;
    impl ForeignCaller for SyncOnly {
        fn call(
            &self,
            _function: &'static str,
            _args: &[ScriptValue],
        ) -> Result<ScriptValue, ForeignError> {
            Ok(ScriptValue::Unit)
        }
    }
    let hooks = HostHooksHandle::from_caller(Box::new(SyncOnly));
    let err = block_on(hooks.fetch("x".to_string())).unwrap_err();
    let HooksError::Foreign(message) = err;
    assert!(message.contains("async"));
}
