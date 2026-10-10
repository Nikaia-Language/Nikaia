// --- `nikaia bind node` (ADR-284 D26, D28, D29) ---------------------------------
//
// **An N-API module in C over the library**, written from the same reading of
// the package as the header and the Python binding, which the host's toolchain
// compiles (`node-gyp` with the `binding.gyp` beside it) - never a `napi`
// dependency in the emitted crate, which would be a second artifact (D28).
//
// What it makes of the boundary: a status other than `OK` is an `Error` whose
// `code` is the header's name for it (`CALC_E_NEGATIVE`) and whose `status` is
// the number, with what `<prefix>_last_error` said; text is a `string`, bytes a
// `Buffer` (a `Uint8Array` or an `ArrayBuffer` in); a run of numbers an array; a
// `scalar` a one-character `string`; an `i64` a `number` where it is a safe
// integer and a `bigint` beyond (either in); a handle a class with its methods,
// a getter per `pub` field, `close()`, and a finalizer that frees it; an `extern`
// struct a plain object, its methods functions of `<Struct>` taking it first; an
// `enum` a frozen object of numbers; `null` and `undefined` are `NULL`; a
// callback a JavaScript function called on the calling thread, whose exception
// the call throws once it has returned. A function that may pause is called in
// its blocking form, and as `<name>_async`: a Promise settled on the main
// thread, whose `cancel()` is the ticket (D9, D19). Only the entry points (D29).

use super::{
    ByValue, Entry, Handled, In, LEDGER_DIGEST, Lent, Out, Part, Plain, Record, records_in_order,
    result_of, taken,
};

/// The N-API module of one library, gathered entry by entry.
#[derive(Default)]
pub(super) struct Node {
    /// Each entry point's `static napi_value` function.
    functions: Vec<String>,
    /// The trampolines a callback parameter is handed as.
    trampolines: Vec<String>,
    /// `exports.<name>`: the property name and the C function.
    exported: Vec<(String, String)>,
    /// Per handle: its instance members, as `napi_property_descriptor`s.
    members: std::collections::BTreeMap<String, Vec<String>>,
    /// Per handle, the function its constructor calls: `<prefix>_<T>_new`'s.
    constructors: std::collections::BTreeMap<String, String>,
    /// Per `extern` struct: its methods, as `(name, function)`.
    methods: std::collections::BTreeMap<String, Vec<(String, String)>>,
    /// Whether any entry has a Promise form, which needs the ticket.
    any_async: bool,
}

/// The converter in and out of one value that crosses by value.
fn scalar(shape: &ByValue) -> &'static str {
    match (shape.scalar, shape.rust) {
        (true, _) => "scalar",
        (_, "i32") => "i32",
        (_, "i64") => "i64",
        (_, "u8") => "u8",
        (_, "f64") => "f64",
        _ => "bool",
    }
}

/// The function an entry point is in C: `nk_<symbol>`.
fn function(symbol: &str) -> String {
    format!("nk_{symbol}")
}

impl Node {
    /// One entry point: a function, or a handle's constructor, method or
    /// getter, or an `extern` struct's method.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn entry(
        &mut self,
        entry: &Entry,
        symbol: &str,
        prefix: &str,
        parsed: &crate::parser::Parsed,
        plains: &[Plain],
        handles: &[Handled],
        records: &[Record],
        text_out: bool,
        pauses: bool,
    ) {
        let by_value = entry
            .owner
            .as_ref()
            .is_some_and(|owner| records.iter().any(|record| record.name == *owner));
        let constructor = entry.owner.is_some()
            && !by_value
            && entry.receiver.is_none()
            && !entry.getter
            && entry.name == "new";
        // Locals, declared before the first `goto done`.
        let mut locals: Vec<String> = Vec::new();
        // The steps that read the arguments, each `goto done` on a refusal.
        let mut steps: Vec<String> = Vec::new();
        // The arguments of the C call.
        let mut args: Vec<String> = Vec::new();
        // The callbacks' states, whose `failed` ends the call with what was thrown.
        let mut calls: Vec<String> = Vec::new();
        // The JavaScript argument the next parameter is read from.
        let mut position = 0_usize;
        let written = match &entry.owner {
            Some(owner) => format!("{owner}.{}", entry.name),
            None => entry.name.clone(),
        };
        if let (Some(_), Some(owner)) = (entry.receiver, &entry.owner) {
            match by_value {
                true => {
                    locals.push(format!("{prefix}_{owner} self_value;"));
                    steps.push(format!(
                        "if (!nk_in_{owner}(env, argv[0], \"self\", &self_value)) goto done;"
                    ));
                    args.push("&self_value".to_string());
                    position = 1;
                }
                false => {
                    locals.push(format!("{prefix}_{owner} *self_ptr = NULL;"));
                    steps.push(format!(
                        "if (!nk_held_{owner}(env, self, \"this\", &self_ptr)) goto done;"
                    ));
                    args.push("self_ptr".to_string());
                }
            }
        }
        for arg in entry.args {
            let name = parsed.text(arg.name).to_string();
            let at = position;
            position += 1;
            let value = format!("argv[{at}]");
            match taken(parsed, plains, handles, records, &arg.ty) {
                Some(In::Value(shape)) => {
                    let kind = scalar(&shape);
                    locals.push(format!("{} p{at} = 0;", shape.c));
                    steps.push(format!(
                        "if (!nk_{kind}(env, {value}, \"{name}\", &p{at})) goto done;"
                    ));
                    args.push(format!("p{at}"));
                }
                Some(In::Run { c, as_text, rust }) => {
                    match as_text {
                        true => {
                            locals.push(format!("const uint8_t *p{at} = NULL;"));
                            locals.push(format!("size_t p{at}_len = 0;"));
                            steps.push(format!(
                                "if (!nk_text(env, {value}, \"{name}\", &kept, &p{at}, &p{at}_len)) goto done;"
                            ));
                        }
                        false => {
                            let kind = scalar(&ByValue {
                                rust,
                                c,
                                scalar: false,
                            });
                            locals.push(format!("{c} *p{at} = NULL;"));
                            locals.push(format!("uint32_t p{at}_len = 0;"));
                            steps.push(format!(
                                "if (!nk_length(env, {value}, \"{name}\", &p{at}_len)) goto done;\n    \
                                 p{at} = nk_keep(env, &kept, sizeof *p{at} * p{at}_len);\n    \
                                 if (!p{at}) goto done;\n    \
                                 for (uint32_t i = 0; i < p{at}_len; i++) {{\n        \
                                 napi_value one;\n        \
                                 napi_get_element(env, {value}, i, &one);\n        \
                                 if (!nk_{kind}(env, one, \"{name}\", &p{at}[i])) goto done;\n    \
                                 }}"
                            ));
                        }
                    }
                    args.push(format!("p{at}"));
                    args.push(format!("p{at}_len"));
                }
                Some(In::Bytes) => {
                    locals.push(format!("const uint8_t *p{at} = NULL;"));
                    locals.push(format!("size_t p{at}_len = 0;"));
                    steps.push(format!(
                        "if (!nk_bytes(env, {value}, \"{name}\", &kept, &p{at}, &p{at}_len)) goto done;"
                    ));
                    args.push(format!("p{at}"));
                    args.push(format!("p{at}_len"));
                }
                Some(In::Choice(which)) => {
                    locals.push(format!("int32_t p{at} = 0;"));
                    steps.push(format!(
                        "if (!nk_i32(env, {value}, \"{name}\", &p{at})) goto done;"
                    ));
                    args.push(format!("({prefix}_{})p{at}", plains[which].name));
                }
                Some(In::Handle(which)) => {
                    let ty = &handles[which].name;
                    locals.push(format!("{prefix}_{ty} *p{at} = NULL;"));
                    steps.push(format!(
                        "if (!nk_held_{ty}(env, {value}, \"{name}\", &p{at})) goto done;"
                    ));
                    args.push(format!("p{at}"));
                }
                Some(In::Absent(present)) => match *present {
                    In::Handle(which) => {
                        let ty = &handles[which].name;
                        locals.push(format!("{prefix}_{ty} *p{at} = NULL;"));
                        steps.push(format!(
                            "if (!nk_absent(env, {value}) && !nk_held_{ty}(env, {value}, \"{name}\", &p{at})) goto done;"
                        ));
                        args.push(format!("p{at}"));
                    }
                    _ => {
                        locals.push(format!("const uint8_t *p{at} = NULL;"));
                        locals.push(format!("size_t p{at}_len = 0;"));
                        steps.push(format!(
                            "if (!nk_absent(env, {value}) && !nk_text(env, {value}, \"{name}\", &kept, &p{at}, &p{at}_len)) goto done;"
                        ));
                        args.push(format!("p{at}"));
                        args.push(format!("p{at}_len"));
                    }
                },
                Some(In::Record(which)) => {
                    let ty = &records[which].name;
                    locals.push(format!("{prefix}_{ty} p{at};"));
                    steps.push(format!(
                        "if (!nk_in_{ty}(env, {value}, \"{name}\", &p{at})) goto done;"
                    ));
                    args.push(format!("p{at}"));
                }
                Some(In::Records(which, _)) => {
                    let ty = &records[which].name;
                    locals.push(format!("{prefix}_{ty} *p{at} = NULL;"));
                    locals.push(format!("uint32_t p{at}_len = 0;"));
                    steps.push(format!(
                        "if (!nk_length(env, {value}, \"{name}\", &p{at}_len)) goto done;\n    \
                         p{at} = nk_keep(env, &kept, sizeof *p{at} * p{at}_len);\n    \
                         if (!p{at}) goto done;\n    \
                         for (uint32_t i = 0; i < p{at}_len; i++) {{\n        \
                         napi_value one;\n        \
                         napi_get_element(env, {value}, i, &one);\n        \
                         if (!nk_in_{ty}(env, one, \"{name}\", &p{at}[i])) goto done;\n    \
                         }}"
                    ));
                    args.push(format!("p{at}"));
                    args.push(format!("p{at}_len"));
                }
                Some(In::Callback { args: lent, result }) => {
                    let trampoline = format!("nk_back_{symbol}_{at}");
                    let mut c_params: Vec<String> = Vec::new();
                    let mut handed: Vec<String> = Vec::new();
                    for (number, part) in lent.iter().enumerate() {
                        match part {
                            Lent::Value(shape) => {
                                c_params.push(format!("{} a{number}", shape.c));
                                handed.push(format!(
                                    "handed[{number}] = nk_from_{}(call->env, a{number});",
                                    scalar(shape)
                                ));
                            }
                            Lent::Choice(which) => {
                                c_params
                                    .push(format!("{prefix}_{} a{number}", plains[*which].name));
                                handed.push(format!(
                                    "handed[{number}] = nk_from_i32(call->env, (int32_t)a{number});"
                                ));
                            }
                            Lent::Text => {
                                c_params.push(format!(
                                    "const uint8_t *a{number}, size_t a{number}_len"
                                ));
                                handed.push(format!(
                                    "napi_create_string_utf8(call->env, (const char *)a{number}, a{number}_len, &handed[{number}]);"
                                ));
                            }
                        }
                    }
                    c_params.push("void *ctx".to_string());
                    let count = lent.len().max(1);
                    let (c_result, fallback, back) = match &result {
                        Some(shape) => (
                            shape.c.to_string(),
                            "0".to_string(),
                            format!(
                                "{} back = 0;\n    \
                                 if (!nk_{}(call->env, result, \"the callback's result\", &back)) {{\n        \
                                 call->failed = true;\n        \
                                 return 0;\n    \
                                 }}\n    \
                                 return back;\n",
                                shape.c,
                                scalar(shape)
                            ),
                        ),
                        None => ("void".to_string(), String::new(), String::new()),
                    };
                    let early = match result.is_some() {
                        true => format!("return {fallback};"),
                        false => "return;".to_string(),
                    };
                    self.trampolines.push(format!(
                        "static {c_result} {trampoline}({}) {{\n    \
                         nk_call *call = ctx;\n    \
                         napi_value handed[{count}];\n    \
                         napi_value undefined;\n    \
                         napi_value result;\n    \
                         if (call->failed) {early}\n    \
                         {}\n    \
                         napi_get_undefined(call->env, &undefined);\n    \
                         if (napi_call_function(call->env, undefined, call->fn, {}, handed, &result) != napi_ok) {{\n        \
                         call->failed = true;\n        \
                         {early}\n    \
                         }}\n    \
                         (void)result;\n    \
                         {back}}}\n",
                        c_params.join(", "),
                        handed.join("\n    "),
                        lent.len(),
                    ));
                    locals.push(format!("nk_call p{at} = {{ env, NULL, false }};"));
                    steps.push(format!(
                        "if (!nk_function(env, {value}, \"{name}\")) goto done;\n    \
                         p{at}.fn = {value};"
                    ));
                    args.push(trampoline);
                    args.push(format!("&p{at}"));
                    calls.push(format!("p{at}"));
                }
                None => {}
            }
        }
        let takes = position;
        // What comes back, through the out-parameter.
        let result = result_of(entry, parsed, plains, handles, records).flatten();
        let call = |args: &[String]| format!("{symbol}({})", args.join(", "));
        let mut body: Vec<String> = Vec::new();
        // The C type of the value and how it is made a JavaScript one, for
        // the Promise form: `None` where nothing comes back.
        // The finisher's C type of `out`, its conversion, and whether a
        // `present` flag follows `out` (D30).
        let mut finish: Option<(Option<String>, String, bool)> = None;
        let failed = match calls.is_empty() {
            true => String::new(),
            false => format!(
                "if ({}) goto done;\n    ",
                calls
                    .iter()
                    .map(|one| format!("{one}.failed"))
                    .collect::<Vec<_>>()
                    .join(" || ")
            ),
        };
        let check = format!(
            "{failed}if (status != 0) {{\n        \
             nk_fail(env, status);\n        \
             goto done;\n    \
             }}"
        );
        match &result {
            None => {
                body.push(format!("status = {};", call(&args)));
                body.push(check);
                body.push("napi_get_undefined(env, &result);".to_string());
                finish = Some((None, "napi_get_undefined(env, &result);".to_string(), false));
            }
            Some(Out::Buffer) => {
                locals.push("uint8_t small[256];".to_string());
                locals.push("uint8_t *room = small;".to_string());
                locals.push("size_t written = 0;".to_string());
                let mut asked = args.clone();
                asked.extend(["room".into(), "sizeof small".into(), "&written".into()]);
                let mut again = args.clone();
                again.extend(["room".into(), "written".into(), "&written".into()]);
                // **A buffer of 256 bytes first**, and the size it asked for
                // where that was too small (D6): one call where it fits.
                body.push(format!("status = {};", call(&asked)));
                body.push(format!(
                    "if (status == -2) {{\n        \
                     room = nk_keep(env, &kept, written);\n        \
                     if (!room) goto done;\n        \
                     status = {};\n    \
                     }}",
                    call(&again)
                ));
                body.push(check);
                body.push(match text_out {
                    true => "napi_create_string_utf8(env, (const char *)room, written, &result);"
                        .to_string(),
                    false => {
                        "napi_create_buffer_copy(env, written, room, NULL, &result);".to_string()
                    }
                });
            }
            Some(Out::Records(which)) => {
                let ty = &records[*which].name;
                locals.push(format!("{prefix}_{ty} small[16];"));
                locals.push(format!("{prefix}_{ty} *room = small;"));
                locals.push("size_t written = 0;".to_string());
                let mut asked = args.clone();
                asked.extend(["room".into(), "16".into(), "&written".into()]);
                let mut again = args.clone();
                again.extend(["room".into(), "written".into(), "&written".into()]);
                body.push(format!("status = {};", call(&asked)));
                body.push(format!(
                    "if (status == -2) {{\n        \
                     room = nk_keep(env, &kept, sizeof *room * written);\n        \
                     if (!room) goto done;\n        \
                     status = {};\n    \
                     }}",
                    call(&again)
                ));
                body.push(check);
                body.push(format!(
                    "napi_create_array_with_length(env, written, &result);\n    \
                     for (size_t i = 0; i < written; i++) {{\n        \
                     napi_set_element(env, result, (uint32_t)i, nk_out_{ty}(env, &room[i]));\n    \
                     }}"
                ));
            }
            Some(out) => {
                // `null` where `present` is false (D30).
                let absent = |back: String| {
                    format!(
                        "if (present) {{\n        \
                         {back}\n    \
                         }} else {{\n        \
                         napi_get_null(env, &result);\n    \
                         }}"
                    )
                };
                let (c, back) = match out {
                    Out::Value(shape) => (
                        shape.c.to_string(),
                        format!("result = nk_from_{}(env, out);", scalar(shape)),
                    ),
                    Out::AbsentValue(shape) => (
                        shape.c.to_string(),
                        absent(format!("result = nk_from_{}(env, out);", scalar(shape))),
                    ),
                    Out::AbsentRecord(which) => {
                        let ty = &records[*which].name;
                        (
                            format!("{prefix}_{ty}"),
                            absent(format!("result = nk_out_{ty}(env, &out);")),
                        )
                    }
                    Out::Choice(which) => (
                        format!("{prefix}_{}", plains[*which].name),
                        "result = nk_from_i32(env, (int32_t)out);".to_string(),
                    ),
                    Out::Record(which) => {
                        let ty = &records[*which].name;
                        (
                            format!("{prefix}_{ty}"),
                            format!("result = nk_out_{ty}(env, &out);"),
                        )
                    }
                    Out::Handle(which) => {
                        let ty = &handles[*which].name;
                        (
                            format!("{prefix}_{ty} *"),
                            match constructor && entry.owner.as_deref() == Some(ty) {
                                true => format!("result = nk_adopt_{ty}(env, self, out);"),
                                false => format!("result = nk_made_{ty}(env, out);"),
                            },
                        )
                    }
                    Out::AbsentHandle(which) => {
                        let ty = &handles[*which].name;
                        (
                            format!("{prefix}_{ty} *"),
                            format!(
                                "if (out) {{\n        \
                                 result = nk_made_{ty}(env, out);\n    \
                                 }} else {{\n        \
                                 napi_get_null(env, &result);\n    \
                                 }}"
                            ),
                        )
                    }
                    Out::Buffer | Out::Records(_) => unreachable!("handled above"),
                };
                let zero = match out {
                    Out::Record(_) | Out::AbsentRecord(_) => "{0}",
                    _ => "0",
                };
                locals.push(format!("{c} out = {zero};"));
                let mut asked = args.clone();
                asked.push("&out".to_string());
                if out.flagged() {
                    locals.push("bool present = false;".to_string());
                    asked.push("&present".to_string());
                }
                body.push(format!("status = {};", call(&asked)));
                body.push(check);
                if !constructor {
                    finish = Some((Some(c), back.clone(), out.flagged()));
                }
                body.push(back);
            }
        }
        // An exclusive method of an `extern` struct changed `self`: the
        // object handed in gets its fields back (D16).
        if let (true, Some(super::Hold::Exclusive), Some(owner)) =
            (by_value, entry.receiver, &entry.owner)
        {
            body.push(format!("nk_assign_{owner}(env, argv[0], &self_value);"));
        }
        let argc = takes.max(1);
        let mut text = format!(
            "/* `{written}` */\n\
             static napi_value {}(napi_env env, napi_callback_info info) {{\n    \
             size_t argc = {argc};\n    \
             napi_value argv[{argc}];\n    \
             napi_value self = NULL;\n    \
             napi_value result = NULL;\n    \
             nk_kept *kept = NULL;\n    \
             int status = 0;\n",
            function(symbol)
        );
        for local in &locals {
            text.push_str(&format!("    {local}\n"));
        }
        text.push_str(
            "    if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) goto done;\n",
        );
        if takes > 0 {
            text.push_str(&format!(
                "    if (argc < {takes}) {{\n        \
                 napi_throw_type_error(env, NULL, \"`{written}` takes {takes} arguments\");\n        \
                 goto done;\n    \
                 }}\n"
            ));
        }
        for step in steps.iter().chain(&body) {
            text.push_str(&format!("    {step}\n"));
        }
        text.push_str(
            "done:\n    \
             nk_release(kept);\n    \
             (void)self;\n    \
             (void)status;\n    \
             return result;\n\
             }\n",
        );
        self.functions.push(text);
        // **The Promise form** (D9, D19): the `_async` call, settled on the
        // main thread through a thread-safe function, its `cancel()` the
        // ticket. Not for a result into a buffer, nor for a callback, nor for
        // `self` of a struct the call would have to keep.
        let promised = match (&finish, pauses && calls.is_empty() && !by_value) {
            (Some((c, back, flagged)), true) => {
                self.any_async = true;
                let finisher = format!("nk_finish_{symbol}");
                // A flagged result is kept as the value with its flag behind
                // it, one block the finisher reads back (D30).
                let (taken_out, read_out, out_arg) = match (c, flagged) {
                    (Some(c), false) => (
                        format!("{c} *out = NULL;"),
                        format!("{c} out = *({c} *)at;"),
                        vec!["out".to_string()],
                    ),
                    (Some(c), true) => (
                        format!("struct {{ {c} value; bool present; }} *out = NULL;"),
                        format!(
                            "struct {{ {c} value; bool present; }} *held = at;\n    \
                             {c} out = held->value;\n    \
                             bool present = held->present;"
                        ),
                        vec!["&out->value".to_string(), "&out->present".to_string()],
                    ),
                    (None, _) => (String::new(), "(void)at;".to_string(), Vec::new()),
                };
                let mut text = format!(
                    "static napi_value {finisher}(napi_env env, void *at) {{\n    \
                     napi_value result = NULL;\n    \
                     {read_out}\n    \
                     {back}\n    \
                     return result;\n\
                     }}\n\
                     \n\
                     /* `{written}`, as a Promise */\n\
                     static napi_value {}_async(napi_env env, napi_callback_info info) {{\n    \
                     size_t argc = {argc};\n    \
                     napi_value argv[{argc}];\n    \
                     napi_value self = NULL;\n    \
                     napi_value result = NULL;\n    \
                     nk_kept *kept = NULL;\n    \
                     nk_async *job = NULL;\n    \
                     int status = 0;\n    \
                     {taken_out}\n",
                    function(symbol)
                );
                for local in locals
                    .iter()
                    .filter(|local| !local.contains(" out = ") && !local.contains(" present = "))
                {
                    text.push_str(&format!("    {local}\n"));
                }
                text.push_str(
                    "    if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) goto done;\n",
                );
                if takes > 0 {
                    text.push_str(&format!(
                        "    if (argc < {takes}) {{\n        \
                         napi_throw_type_error(env, NULL, \"`{written}` takes {takes} arguments\");\n        \
                         goto done;\n    \
                         }}\n"
                    ));
                }
                for step in &steps {
                    text.push_str(&format!("    {step}\n"));
                }
                if !out_arg.is_empty() {
                    text.push_str(
                        "    out = nk_keep(env, &kept, sizeof *out);\n    \
                         if (!out) goto done;\n    \
                         memset(out, 0, sizeof *out);\n",
                    );
                }
                let mut handed = args.clone();
                handed.extend(out_arg);
                handed.extend([
                    "nk_landed".to_string(),
                    "job".to_string(),
                    "&job->ticket->op".to_string(),
                ]);
                let started = match c {
                    Some(_) => "out",
                    None => "NULL",
                };
                text.push_str(&format!(
                    "    job = nk_start(env, &kept, {started}, {finisher}, &result);\n    \
                     if (!job) goto done;\n    \
                     status = {symbol}_async({});\n    \
                     if (status != 0) nk_abandon(env, job, status);\n\
                     done:\n    \
                     nk_release(kept);\n    \
                     (void)self;\n    \
                     return result;\n\
                     }}\n",
                    handed.join(", ")
                ));
                self.functions.push(text);
                Some(format!("{}_async", function(symbol)))
            }
            _ => None,
        };
        let name = entry.name.clone();
        let function = function(symbol);
        if let Some(promised) = promised {
            let property = format!("{name}_async");
            match (&entry.owner, entry.receiver) {
                (None, _) => self.exported.push((property, promised)),
                (Some(owner), Some(_)) => {
                    self.members.entry(owner.clone()).or_default().push(format!(
                        "{{ \"{property}\", NULL, {promised}, NULL, NULL, NULL, napi_default_method, NULL }}"
                    ))
                }
                (Some(owner), None) => {
                    self.members.entry(owner.clone()).or_default().push(format!(
                        "{{ \"{property}\", NULL, {promised}, NULL, NULL, NULL, napi_static, NULL }}"
                    ))
                }
            }
        }
        match (&entry.owner, by_value, entry.receiver, entry.getter) {
            (None, _, _, _) => self.exported.push((name, function)),
            (Some(owner), true, _, _) => self
                .methods
                .entry(owner.clone())
                .or_default()
                .push((name, function)),
            (Some(owner), false, _, true) => {
                self.members.entry(owner.clone()).or_default().push(format!(
                    "{{ \"{name}\", NULL, NULL, {function}, NULL, NULL, napi_enumerable, NULL }}"
                ))
            }
            (Some(owner), false, Some(_), false) => {
                self.members.entry(owner.clone()).or_default().push(format!(
                    "{{ \"{name}\", NULL, {function}, NULL, NULL, NULL, napi_default_method, NULL }}"
                ))
            }
            (Some(owner), false, None, false) if constructor => {
                self.constructors.insert(owner.clone(), function);
            }
            (Some(owner), false, None, false) => {
                self.members.entry(owner.clone()).or_default().push(format!(
                    "{{ \"{name}\", NULL, {function}, NULL, NULL, NULL, napi_static, NULL }}"
                ))
            }
        }
    }

    /// The module: `<package>.c`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn module(
        &self,
        package: &str,
        prefix: &str,
        codes: &[(String, Vec<(String, i64)>)],
        plains: &[Plain],
        handles: &[Handled],
        held: &[bool],
        records: &[Record],
    ) -> String {
        let upper = prefix.to_uppercase();
        let mut statuses: Vec<(i64, String)> = [
            (-1, "ARGUMENT"),
            (-2, "TOO_SMALL"),
            (-3, "NOT_RUNNING"),
            (-4, "PANICKED"),
            (-5, "REENTRANT"),
            (-6, "CLEANUP"),
            (-7, "CANCELLED"),
        ]
        .iter()
        .map(|(code, name)| (*code, format!("{upper}_E_{name}")))
        .collect();
        for (_, variants) in codes {
            for (variant, code) in variants {
                statuses.push((*code, format!("{upper}_E_{}", variant.to_uppercase())));
            }
        }
        let cases: String = statuses
            .iter()
            .map(|(code, name)| format!("    case {code}: return \"{name}\";\n"))
            .collect();
        let exported: Vec<(&Handled, usize)> = handles
            .iter()
            .zip(held)
            .enumerate()
            .filter(|(_, (_, held))| **held)
            .map(|(at, (handle, _))| (handle, at))
            .collect();
        let state_fields: String = match exported.is_empty() {
            true => "    int none;\n".to_string(),
            false => exported
                .iter()
                .map(|(handle, _)| format!("    napi_ref {};\n", handle.name))
                .collect(),
        };
        let mut out = format!(
            "/* {package} - GENERATED by `nikaia bind node` from {package}'s ledger. Do not edit.\n\
             \x20  ledger: {LEDGER_DIGEST}\n\
             \x20  An N-API module over lib{package}: build it with the host's toolchain,\n\
             \x20  `node-gyp rebuild` with the binding.gyp beside it (ADR-284 D28). */\n\
             \n\
             #define NAPI_VERSION 8\n\
             #include <node_api.h>\n\
             #include <stdbool.h>\n\
             #include <stdint.h>\n\
             #include <stdio.h>\n\
             #include <stdlib.h>\n\
             #include <string.h>\n\
             #include \"{package}.h\"\n\
             \n\
             #if defined(__GNUC__)\n\
             #define NK_HELPER static __attribute__((unused))\n\
             #else\n\
             #define NK_HELPER static\n\
             #endif\n\
             \n\
             /* The constructors of this module's classes, per environment. */\n\
             typedef struct {{\n{state_fields}}} nk_state;\n\
             \n\
             static void nk_state_free(napi_env env, void *data, void *hint) {{\n    \
             (void)env;\n    \
             (void)hint;\n    \
             free(data);\n\
             }}\n\
             \n\
             /* What a call allocated, freed when it returns. */\n\
             typedef struct nk_kept {{\n    \
             void *at;\n    \
             struct nk_kept *next;\n\
             }} nk_kept;\n\
             \n\
             NK_HELPER void *nk_keep(napi_env env, nk_kept **kept, size_t size) {{\n    \
             nk_kept *one = malloc(sizeof *one);\n    \
             void *at = malloc(size ? size : 1);\n    \
             if (!one || !at) {{\n        \
             free(one);\n        \
             free(at);\n        \
             napi_throw_error(env, NULL, \"out of memory\");\n        \
             return NULL;\n    \
             }}\n    \
             one->at = at;\n    \
             one->next = *kept;\n    \
             *kept = one;\n    \
             return at;\n\
             }}\n\
             \n\
             NK_HELPER void nk_release(nk_kept *kept) {{\n    \
             while (kept) {{\n        \
             nk_kept *next = kept->next;\n        \
             free(kept->at);\n        \
             free(kept);\n        \
             kept = next;\n    \
             }}\n\
             }}\n\
             \n\
             /* The header's name for a status (ADR-284 D7). */\n\
             static const char *nk_code(int status) {{\n    \
             switch (status) {{\n{cases}    \
             default: return \"{upper}_E_UNKNOWN\";\n    \
             }}\n\
             }}\n\
             \n\
             /* The status as an Error: `code` its name, `status` its number, the message\n\
             \x20  what {prefix}_last_error said, or the name where it said nothing. */\n\
             NK_HELPER napi_value nk_error(napi_env env, int status, const char *said, size_t len) {{\n    \
             napi_value message, code, error, number;\n    \
             if (said) {{\n        \
             napi_create_string_utf8(env, said, len, &message);\n    \
             }} else {{\n        \
             napi_create_string_utf8(env, nk_code(status), NAPI_AUTO_LENGTH, &message);\n    \
             }}\n    \
             napi_create_string_utf8(env, nk_code(status), NAPI_AUTO_LENGTH, &code);\n    \
             napi_create_error(env, code, message, &error);\n    \
             napi_create_int32(env, status, &number);\n    \
             napi_set_named_property(env, error, \"status\", number);\n    \
             return error;\n\
             }}\n\
             \n\
             /* What {prefix}_last_error says on this thread, or NULL. */\n\
             NK_HELPER char *nk_said(size_t *len) {{\n    \
             size_t written = 0;\n    \
             char *said = NULL;\n    \
             {prefix}_last_error(NULL, 0, &written);\n    \
             said = malloc(written + 1);\n    \
             if (said && {prefix}_last_error((uint8_t *)said, written, &written) == 0) {{\n        \
             *len = written;\n        \
             return said;\n    \
             }}\n    \
             free(said);\n    \
             return NULL;\n\
             }}\n\
             \n\
             /* Throws the status, with what this thread's last failure said. */\n\
             NK_HELPER void nk_fail(napi_env env, int status) {{\n    \
             size_t len = 0;\n    \
             char *said = nk_said(&len);\n    \
             napi_throw(env, nk_error(env, status, said, len));\n    \
             free(said);\n\
             }}\n\
             \n\
             NK_HELPER bool nk_wrong(napi_env env, const char *name, const char *wanted) {{\n    \
             char said[256];\n    \
             snprintf(said, sizeof said, \"`%s` must be %s\", name, wanted);\n    \
             napi_throw_type_error(env, NULL, said);\n    \
             return false;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_absent(napi_env env, napi_value value) {{\n    \
             napi_valuetype type;\n    \
             napi_typeof(env, value, &type);\n    \
             return type == napi_null || type == napi_undefined;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_function(napi_env env, napi_value value, const char *name) {{\n    \
             napi_valuetype type;\n    \
             napi_typeof(env, value, &type);\n    \
             return type == napi_function || nk_wrong(env, name, \"a function\");\n\
             }}\n\
             \n\
             /* An integer between low and high: a number that is a safe integer, or a bigint. */\n\
             NK_HELPER bool nk_integer(napi_env env, napi_value value, const char *name, int64_t low, int64_t high, int64_t *out) {{\n    \
             napi_valuetype type;\n    \
             napi_typeof(env, value, &type);\n    \
             if (type == napi_bigint) {{\n        \
             bool lossless = false;\n        \
             int64_t n = 0;\n        \
             napi_get_value_bigint_int64(env, value, &n, &lossless);\n        \
             if (!lossless || n < low || n > high) {{\n            \
             char said[256];\n            \
             snprintf(said, sizeof said, \"`%s` is out of range\", name);\n            \
             napi_throw_range_error(env, NULL, said);\n            \
             return false;\n        \
             }}\n        \
             *out = n;\n        \
             return true;\n    \
             }}\n    \
             if (type != napi_number) return nk_wrong(env, name, \"an integer\");\n    \
             double d = 0;\n    \
             napi_get_value_double(env, value, &d);\n    \
             if (!(d >= -9007199254740991.0 && d <= 9007199254740991.0) || (double)(int64_t)d != d) {{\n        \
             return nk_wrong(env, name, \"a safe integer or a bigint\");\n    \
             }}\n    \
             if ((int64_t)d < low || (int64_t)d > high) {{\n        \
             char said[256];\n        \
             snprintf(said, sizeof said, \"`%s` is out of range\", name);\n        \
             napi_throw_range_error(env, NULL, said);\n        \
             return false;\n    \
             }}\n    \
             *out = (int64_t)d;\n    \
             return true;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_i32(napi_env env, napi_value value, const char *name, int32_t *out) {{\n    \
             int64_t n = 0;\n    \
             if (!nk_integer(env, value, name, INT32_MIN, INT32_MAX, &n)) return false;\n    \
             *out = (int32_t)n;\n    \
             return true;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_i64(napi_env env, napi_value value, const char *name, int64_t *out) {{\n    \
             return nk_integer(env, value, name, INT64_MIN, INT64_MAX, out);\n\
             }}\n\
             \n\
             NK_HELPER bool nk_u8(napi_env env, napi_value value, const char *name, uint8_t *out) {{\n    \
             int64_t n = 0;\n    \
             if (!nk_integer(env, value, name, 0, 255, &n)) return false;\n    \
             *out = (uint8_t)n;\n    \
             return true;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_f64(napi_env env, napi_value value, const char *name, double *out) {{\n    \
             napi_valuetype type;\n    \
             napi_typeof(env, value, &type);\n    \
             if (type != napi_number) return nk_wrong(env, name, \"a number\");\n    \
             napi_get_value_double(env, value, out);\n    \
             return true;\n\
             }}\n\
             \n\
             NK_HELPER bool nk_bool(napi_env env, napi_value value, const char *name, bool *out) {{\n    \
             napi_valuetype type;\n    \
             napi_typeof(env, value, &type);\n    \
             if (type != napi_boolean) return nk_wrong(env, name, \"a boolean\");\n    \
             napi_get_value_bool(env, value, out);\n    \
             return true;\n\
             }}\n\
             \n\
             /* A string of one character, which is its Unicode scalar value. */\n\
             NK_HELPER bool nk_scalar(napi_env env, napi_value value, const char *name, uint32_t *out) {{\n    \
             napi_valuetype type;\n    \
             char16_t units[3];\n    \
             size_t length = 0;\n    \
             napi_typeof(env, value, &type);\n    \
             if (type != napi_string) return nk_wrong(env, name, \"a string of one character\");\n    \
             napi_get_value_string_utf16(env, value, units, 3, &length);\n    \
             if (length == 1 && (units[0] < 0xD800 || units[0] > 0xDFFF)) {{\n        \
             *out = units[0];\n        \
             return true;\n    \
             }}\n    \
             if (length == 2 && units[0] >= 0xD800 && units[0] <= 0xDBFF && units[1] >= 0xDC00 && units[1] <= 0xDFFF) {{\n        \
             *out = 0x10000 + ((uint32_t)(units[0] - 0xD800) << 10) + (uint32_t)(units[1] - 0xDC00);\n        \
             return true;\n    \
             }}\n    \
             return nk_wrong(env, name, \"a string of one character\");\n\
             }}\n\
             \n\
             /* Text, as UTF-8 the call keeps until it returns. */\n\
             NK_HELPER bool nk_text(napi_env env, napi_value value, const char *name, nk_kept **kept, const uint8_t **at, size_t *len) {{\n    \
             napi_valuetype type;\n    \
             size_t length = 0;\n    \
             char *room = NULL;\n    \
             napi_typeof(env, value, &type);\n    \
             if (type != napi_string) return nk_wrong(env, name, \"a string\");\n    \
             napi_get_value_string_utf8(env, value, NULL, 0, &length);\n    \
             room = nk_keep(env, kept, length + 1);\n    \
             if (!room) return false;\n    \
             napi_get_value_string_utf8(env, value, room, length + 1, &length);\n    \
             *at = (const uint8_t *)room;\n    \
             *len = length;\n    \
             return true;\n\
             }}\n\
             \n\
             /* Bytes: a Buffer or another Uint8Array, or an ArrayBuffer. */\n\
             NK_HELPER bool nk_lent(napi_env env, napi_value value, const char *name, const uint8_t **at, size_t *len) {{\n    \
             bool is = false;\n    \
             void *data = NULL;\n    \
             napi_is_typedarray(env, value, &is);\n    \
             if (is) {{\n        \
             napi_typedarray_type type;\n        \
             napi_value buffer;\n        \
             size_t offset = 0;\n        \
             napi_get_typedarray_info(env, value, &type, len, &data, &buffer, &offset);\n        \
             if (type != napi_uint8_array && type != napi_uint8_clamped_array) return nk_wrong(env, name, \"a Uint8Array\");\n        \
             *at = data;\n        \
             return true;\n    \
             }}\n    \
             napi_is_arraybuffer(env, value, &is);\n    \
             if (is) {{\n        \
             napi_get_arraybuffer_info(env, value, &data, len);\n        \
             *at = data;\n        \
             return true;\n    \
             }}\n    \
             return nk_wrong(env, name, \"a Buffer, a Uint8Array or an ArrayBuffer\");\n\
             }}\n\
             \n\
             /* Bytes, copied: an `_async` call keeps them past the call that started it. */\n\
             NK_HELPER bool nk_bytes(napi_env env, napi_value value, const char *name, nk_kept **kept, const uint8_t **at, size_t *len) {{\n    \
             const uint8_t *lent = NULL;\n    \
             uint8_t *room = NULL;\n    \
             if (!nk_lent(env, value, name, &lent, len)) return false;\n    \
             room = nk_keep(env, kept, *len);\n    \
             if (!room) return false;\n    \
             if (*len) memcpy(room, lent, *len);\n    \
             *at = room;\n    \
             return true;\n\
             }}\n\
             \n\
             /* The length of an array, or of a typed array. */\n\
             NK_HELPER bool nk_length(napi_env env, napi_value value, const char *name, uint32_t *n) {{\n    \
             bool array = false;\n    \
             bool typed = false;\n    \
             napi_value length;\n    \
             napi_is_array(env, value, &array);\n    \
             napi_is_typedarray(env, value, &typed);\n    \
             if (!array && !typed) return nk_wrong(env, name, \"an array\");\n    \
             napi_get_named_property(env, value, \"length\", &length);\n    \
             napi_get_value_uint32(env, length, n);\n    \
             return true;\n\
             }}\n\
             \n\
             NK_HELPER napi_value nk_from_i32(napi_env env, int32_t n) {{\n    \
             napi_value made;\n    \
             napi_create_int32(env, n, &made);\n    \
             return made;\n\
             }}\n\
             \n\
             /* A number where it is a safe integer, a bigint beyond. */\n\
             NK_HELPER napi_value nk_from_i64(napi_env env, int64_t n) {{\n    \
             napi_value made;\n    \
             if (n >= -9007199254740991LL && n <= 9007199254740991LL) {{\n        \
             napi_create_int64(env, n, &made);\n    \
             }} else {{\n        \
             napi_create_bigint_int64(env, n, &made);\n    \
             }}\n    \
             return made;\n\
             }}\n\
             \n\
             NK_HELPER napi_value nk_from_u8(napi_env env, uint8_t n) {{\n    \
             napi_value made;\n    \
             napi_create_uint32(env, n, &made);\n    \
             return made;\n\
             }}\n\
             \n\
             NK_HELPER napi_value nk_from_f64(napi_env env, double n) {{\n    \
             napi_value made;\n    \
             napi_create_double(env, n, &made);\n    \
             return made;\n\
             }}\n\
             \n\
             NK_HELPER napi_value nk_from_bool(napi_env env, bool b) {{\n    \
             napi_value made;\n    \
             napi_get_boolean(env, b, &made);\n    \
             return made;\n\
             }}\n\
             \n\
             NK_HELPER napi_value nk_from_scalar(napi_env env, uint32_t c) {{\n    \
             char16_t units[2];\n    \
             size_t length = 1;\n    \
             napi_value made;\n    \
             if (c >= 0x10000) {{\n        \
             units[0] = (char16_t)(0xD800 + ((c - 0x10000) >> 10));\n        \
             units[1] = (char16_t)(0xDC00 + ((c - 0x10000) & 0x3FF));\n        \
             length = 2;\n    \
             }} else {{\n        \
             units[0] = (char16_t)c;\n    \
             }}\n    \
             napi_create_string_utf16(env, units, length, &made);\n    \
             return made;\n\
             }}\n\
             \n\
             /* An enum's number, which the library checks (ADR-284 D5). */\n\
             NK_HELPER bool nk_choice(napi_env env, napi_value value, const char *name, int *out) {{\n    \
             int32_t n = 0;\n    \
             if (!nk_i32(env, value, name, &n)) return false;\n    \
             *out = n;\n    \
             return true;\n\
             }}\n\
             \n\
             /* A callback's state: the function, and whether it threw. */\n\
             typedef struct {{\n    \
             napi_env env;\n    \
             napi_value fn;\n    \
             bool failed;\n\
             }} nk_call;\n\
             \n"
        );
        // **An `extern` struct is a plain object** (D14), read field by field
        // and checked, and made field by field.
        for record in records_in_order(records).iter().map(|at| &records[*at]) {
            let ty = &record.name;
            let mut reads = String::new();
            let mut writes = String::new();
            // One field's read and write: `at` is where it sits in the C
            // value, `owner` how a refusal names what holds it.
            let field_code = |field: &str,
                              part: &Part,
                              owner: &str,
                              at: &str,
                              reads: &mut String,
                              writes: &mut String| {
                let what = format!("{owner}{field}");
                let pair = |part: &Part, place: &str| -> (String, String) {
                    match part {
                        Part::Value(shape) => (
                            format!(
                                "if (!nk_{}(env, one, \"{what}\", &{place})) return false;",
                                scalar(shape)
                            ),
                            format!("nk_from_{}(env, {place})", scalar(shape)),
                        ),
                        Part::Choice(_) => (
                            format!(
                                "{{ int c = 0; if (!nk_choice(env, one, \"{what}\", &c)) return false; {place} = c; }}"
                            ),
                            format!("nk_from_i32(env, (int32_t){place})"),
                        ),
                        Part::Record(at) => (
                            format!(
                                "if (!nk_in_{}(env, one, \"{what}\", &{place})) return false;",
                                records[*at].name
                            ),
                            format!("nk_out_{}(env, &{place})", records[*at].name),
                        ),
                        Part::Array(..) => unreachable!("an array's element is no array"),
                    }
                };
                match part {
                    Part::Array(element, count) => {
                        let (read, write) = pair(element, &format!("out->{at}{field}[i]"));
                        reads.push_str(&format!(
                            "    napi_get_named_property(env, value, \"{field}\", &field);\n    \
                             if (!nk_length(env, field, \"{what}\", &n)) return false;\n    \
                             if (n != {count}) return nk_wrong(env, \"{what}\", \"an array of {count}\");\n    \
                             for (uint32_t i = 0; i < {count}; i++) {{\n        \
                             napi_value one;\n        \
                             napi_get_element(env, field, i, &one);\n        \
                             {read}\n    \
                             }}\n"
                        ));
                        let write = write.replace("out->", "value->");
                        writes.push_str(&format!(
                            "    napi_create_array_with_length(env, {count}, &field);\n    \
                             for (uint32_t i = 0; i < {count}; i++) {{\n        \
                             napi_set_element(env, field, i, {write});\n    \
                             }}\n    \
                             napi_set_named_property(env, target, \"{field}\", field);\n"
                        ));
                    }
                    other => {
                        let (read, write) = pair(other, &format!("out->{at}{field}"));
                        reads.push_str(&format!(
                            "    {{\n        \
                             napi_value one;\n        \
                             napi_get_named_property(env, value, \"{field}\", &one);\n        \
                             {read}\n    \
                             }}\n"
                        ));
                        let write = write.replace("out->", "value->");
                        writes.push_str(&format!(
                            "    napi_set_named_property(env, target, \"{field}\", {write});\n"
                        ));
                    }
                }
            };
            // **An `extern` enum is `{ kind: "Variant", field… }`** (D32): its
            // tag by the variant's name, its fields beside it.
            if record.is_enum() {
                let mut chosen = Vec::new();
                let mut cases = Vec::new();
                for (tag, variant) in record.variants.iter().enumerate() {
                    let (name, member) = (&variant.name, &variant.member);
                    let mut variant_reads = String::new();
                    let mut variant_writes = String::new();
                    for (field, part) in &variant.fields {
                        field_code(
                            field,
                            part,
                            &format!("{ty}::{name}."),
                            &format!("{member}."),
                            &mut variant_reads,
                            &mut variant_writes,
                        );
                    }
                    chosen.push(format!(
                        "if (strcmp(kind, \"{name}\") == 0) {{\n    \
                         out->tag = {tag};\n\
                         {variant_reads}    \
                         }}"
                    ));
                    cases.push(format!(
                        "    case {tag}:\n    \
                         napi_create_string_utf8(env, \"{name}\", NAPI_AUTO_LENGTH, &field);\n    \
                         napi_set_named_property(env, target, \"kind\", field);\n\
                         {variant_writes}    \
                         break;\n"
                    ));
                }
                let names: Vec<&str> = record.variants.iter().map(|v| v.name.as_str()).collect();
                reads.push_str(&format!(
                    "    char kind[64];\n    \
                     size_t kind_len = 0;\n    \
                     napi_get_named_property(env, value, \"kind\", &field);\n    \
                     if (napi_get_value_string_utf8(env, field, kind, sizeof kind, &kind_len) != napi_ok)\n        \
                     return nk_wrong(env, name, \"a {ty} with a kind\");\n    \
                     {} else {{\n        \
                     return nk_wrong(env, name, \"a {ty}: {}\");\n    \
                     }}\n",
                    chosen.join(" else "),
                    names.join(", ")
                ));
                writes.push_str(&format!(
                    "    switch (value->tag) {{\n{}    default:\n        break;\n    }}\n",
                    cases.concat()
                ));
            } else {
                for (field, part) in &record.fields {
                    field_code(field, part, &format!("{ty}."), "", &mut reads, &mut writes);
                }
            }
            out.push_str(&format!(
                "/* `{ty}`, read from an object: every field is there and of its type. */\n\
                 NK_HELPER bool nk_in_{ty}(napi_env env, napi_value value, const char *name, {prefix}_{ty} *out) {{\n    \
                 napi_valuetype type;\n    \
                 napi_value field;\n    \
                 uint32_t n = 0;\n    \
                 (void)field;\n    \
                 (void)n;\n    \
                 napi_typeof(env, value, &type);\n    \
                 if (type != napi_object) return nk_wrong(env, name, \"a {ty}\");\n\
                 {reads}    \
                 return true;\n\
                 }}\n\
                 \n\
                 /* `{ty}`'s fields, written onto an object. */\n\
                 NK_HELPER void nk_assign_{ty}(napi_env env, napi_value target, const {prefix}_{ty} *value) {{\n    \
                 napi_value field;\n    \
                 (void)field;\n\
                 {writes}\
                 }}\n\
                 \n\
                 NK_HELPER napi_value nk_out_{ty}(napi_env env, const {prefix}_{ty} *value) {{\n    \
                 napi_value made;\n    \
                 napi_create_object(env, &made);\n    \
                 nk_assign_{ty}(env, made, value);\n    \
                 return made;\n\
                 }}\n\
                 \n"
            ));
        }
        // **A handle is an object of its class** (D5), tagged so that no
        // other object passes for it, freed by `close()` or by the collector.
        for (handle, at) in &exported {
            let ty = &handle.name;
            out.push_str(&format!(
                "static const napi_type_tag nk_tag_{ty} = {{ 0x6e696b616961ULL, {at}ULL }};\n\
                 \n\
                 typedef struct {{\n    \
                 {prefix}_{ty} *ptr;\n\
                 }} nk_{ty};\n\
                 \n\
                 static void nk_finalize_{ty}(napi_env env, void *data, void *hint) {{\n    \
                 nk_{ty} *held = data;\n    \
                 (void)env;\n    \
                 (void)hint;\n    \
                 if (held->ptr) {prefix}_{ty}_free(held->ptr);\n    \
                 free(held);\n\
                 }}\n\
                 \n\
                 /* `this` takes the handle `ptr`. */\n\
                 NK_HELPER napi_value nk_adopt_{ty}(napi_env env, napi_value self, {prefix}_{ty} *ptr) {{\n    \
                 nk_{ty} *held = malloc(sizeof *held);\n    \
                 if (!held) {{\n        \
                 {prefix}_{ty}_free(ptr);\n        \
                 napi_throw_error(env, NULL, \"out of memory\");\n        \
                 return NULL;\n    \
                 }}\n    \
                 held->ptr = ptr;\n    \
                 if (napi_wrap(env, self, held, nk_finalize_{ty}, NULL, NULL) != napi_ok) {{\n        \
                 nk_finalize_{ty}(env, held, NULL);\n        \
                 return NULL;\n    \
                 }}\n    \
                 napi_type_tag_object(env, self, &nk_tag_{ty});\n    \
                 return self;\n\
                 }}\n\
                 \n\
                 /* A new object of the class around `ptr`. */\n\
                 NK_HELPER napi_value nk_made_{ty}(napi_env env, {prefix}_{ty} *ptr) {{\n    \
                 nk_state *state = NULL;\n    \
                 napi_value class, external, made = NULL;\n    \
                 napi_get_instance_data(env, (void **)&state);\n    \
                 napi_get_reference_value(env, state->{ty}, &class);\n    \
                 napi_create_external(env, ptr, NULL, NULL, &external);\n    \
                 if (napi_new_instance(env, class, 1, &external, &made) != napi_ok) return NULL;\n    \
                 return made;\n\
                 }}\n\
                 \n\
                 /* The handle `value` holds: one of this class, not closed. */\n\
                 NK_HELPER bool nk_held_{ty}(napi_env env, napi_value value, const char *name, {prefix}_{ty} **out) {{\n    \
                 napi_valuetype type;\n    \
                 bool tagged = false;\n    \
                 nk_{ty} *held = NULL;\n    \
                 napi_typeof(env, value, &type);\n    \
                 if (type == napi_object) napi_check_object_type_tag(env, value, &nk_tag_{ty}, &tagged);\n    \
                 if (!tagged) return nk_wrong(env, name, \"a {ty}\");\n    \
                 napi_unwrap(env, value, (void **)&held);\n    \
                 if (!held || !held->ptr) {{\n        \
                 napi_throw_error(env, \"{upper}_E_ARGUMENT\", \"the handle is closed\");\n        \
                 return false;\n    \
                 }}\n    \
                 *out = held->ptr;\n    \
                 return true;\n\
                 }}\n\
                 \n\
                 /* `close()`: frees the handle now; a closed one stays closed. */\n\
                 static napi_value nk_close_{ty}(napi_env env, napi_callback_info info) {{\n    \
                 napi_value self;\n    \
                 bool tagged = false;\n    \
                 nk_{ty} *held = NULL;\n    \
                 if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;\n    \
                 napi_check_object_type_tag(env, self, &nk_tag_{ty}, &tagged);\n    \
                 if (!tagged) {{\n        \
                 nk_wrong(env, \"this\", \"a {ty}\");\n        \
                 return NULL;\n    \
                 }}\n    \
                 napi_unwrap(env, self, (void **)&held);\n    \
                 if (held && held->ptr) {{\n        \
                 int status = {prefix}_{ty}_free(held->ptr);\n        \
                 if (status != 0) {{\n            \
                 nk_fail(env, status);\n            \
                 return NULL;\n        \
                 }}\n        \
                 held->ptr = NULL;\n    \
                 }}\n    \
                 return NULL;\n\
                 }}\n\
                 \n"
            ));
        }
        if self.any_async {
            out.push_str(&format!(
                "/* An `_async` call's ticket (ADR-284 D19): freed once the call has settled\n\
                 \x20  and its `cancel` function has been collected. */\n\
                 typedef struct {{\n    \
                 {prefix}_op *op;\n    \
                 int refs;\n    \
                 bool settled;\n\
                 }} nk_ticket;\n\
                 \n\
                 static void nk_ticket_drop(nk_ticket *ticket) {{\n    \
                 if (--ticket->refs == 0) {{\n        \
                 {prefix}_op_free(ticket->op);\n        \
                 free(ticket);\n    \
                 }}\n\
                 }}\n\
                 \n\
                 static void nk_ticket_collected(napi_env env, void *data, void *hint) {{\n    \
                 (void)env;\n    \
                 (void)hint;\n    \
                 nk_ticket_drop(data);\n\
                 }}\n\
                 \n\
                 /* `cancel()`: at the call's next pause point; after it settled, nothing. */\n\
                 static napi_value nk_cancel(napi_env env, napi_callback_info info) {{\n    \
                 void *data = NULL;\n    \
                 nk_ticket *ticket = NULL;\n    \
                 if (napi_get_cb_info(env, info, NULL, NULL, NULL, &data) != napi_ok) return NULL;\n    \
                 ticket = data;\n    \
                 if (!ticket->settled && ticket->op) {{\n        \
                 int status = {prefix}_cancel(ticket->op);\n        \
                 if (status != 0) nk_fail(env, status);\n    \
                 }}\n    \
                 return NULL;\n\
                 }}\n\
                 \n\
                 /* An `_async` call in flight: what it keeps, and the Promise it settles. */\n\
                 typedef struct {{\n    \
                 napi_deferred deferred;\n    \
                 napi_threadsafe_function tsfn;\n    \
                 nk_kept *kept;\n    \
                 void *out;\n    \
                 napi_value (*finish)(napi_env, void *);\n    \
                 nk_ticket *ticket;\n    \
                 int status;\n    \
                 char *said;\n    \
                 size_t said_len;\n\
                 }} nk_async;\n\
                 \n\
                 /* On the main thread: the Promise settled, and what the call kept freed. */\n\
                 static void nk_settle(napi_env env, napi_value js, void *context, void *data) {{\n    \
                 nk_async *job = data;\n    \
                 (void)js;\n    \
                 (void)context;\n    \
                 if (env) {{\n        \
                 if (job->status == 0) {{\n            \
                 napi_value value = job->finish(env, job->out);\n            \
                 if (!value) napi_get_undefined(env, &value);\n            \
                 napi_resolve_deferred(env, job->deferred, value);\n        \
                 }} else {{\n            \
                 napi_reject_deferred(env, job->deferred, nk_error(env, job->status, job->said, job->said_len));\n        \
                 }}\n    \
                 }}\n    \
                 job->ticket->settled = true;\n    \
                 nk_ticket_drop(job->ticket);\n    \
                 nk_release(job->kept);\n    \
                 free(job->said);\n    \
                 free(job);\n\
                 }}\n\
                 \n\
                 /* `done`, on a library thread: what last_error says is read here, where it was said. */\n\
                 static void nk_landed(int status, void *ctx) {{\n    \
                 nk_async *job = ctx;\n    \
                 job->status = status;\n    \
                 if (status != 0) job->said = nk_said(&job->said_len);\n    \
                 napi_call_threadsafe_function(job->tsfn, job, napi_tsfn_blocking);\n    \
                 napi_release_threadsafe_function(job->tsfn, napi_tsfn_release);\n\
                 }}\n\
                 \n\
                 /* A Promise with its `cancel()`, and the call that settles it, which takes what the\n\
                 \x20  arguments kept. */\n\
                 static nk_async *nk_start(napi_env env, nk_kept **kept, void *out, napi_value (*finish)(napi_env, void *), napi_value *promise) {{\n    \
                 nk_async *job = calloc(1, sizeof *job);\n    \
                 nk_ticket *ticket = calloc(1, sizeof *ticket);\n    \
                 napi_value name, cancel;\n    \
                 if (!job || !ticket) {{\n        \
                 free(job);\n        \
                 free(ticket);\n        \
                 napi_throw_error(env, NULL, \"out of memory\");\n        \
                 return NULL;\n    \
                 }}\n    \
                 napi_create_string_utf8(env, \"{prefix}\", NAPI_AUTO_LENGTH, &name);\n    \
                 if (napi_create_threadsafe_function(env, NULL, NULL, name, 0, 1, NULL, NULL, NULL, nk_settle, &job->tsfn) != napi_ok) {{\n        \
                 free(job);\n        \
                 free(ticket);\n        \
                 return NULL;\n    \
                 }}\n    \
                 napi_create_promise(env, &job->deferred, promise);\n    \
                 ticket->refs = 1;\n    \
                 if (napi_create_function(env, \"cancel\", NAPI_AUTO_LENGTH, nk_cancel, ticket, &cancel) == napi_ok\n        \
                 && napi_add_finalizer(env, cancel, ticket, nk_ticket_collected, NULL, NULL) == napi_ok) {{\n        \
                 ticket->refs = 2;\n        \
                 napi_set_named_property(env, *promise, \"cancel\", cancel);\n    \
                 }}\n    \
                 job->ticket = ticket;\n    \
                 job->out = out;\n    \
                 job->finish = finish;\n    \
                 job->kept = *kept;\n    \
                 *kept = NULL;\n    \
                 return job;\n\
                 }}\n\
                 \n\
                 /* The call did not start, and `done` will not run: the Promise is rejected now. */\n\
                 static void nk_abandon(napi_env env, nk_async *job, int status) {{\n    \
                 job->status = status;\n    \
                 job->said = nk_said(&job->said_len);\n    \
                 napi_release_threadsafe_function(job->tsfn, napi_tsfn_abort);\n    \
                 nk_settle(env, NULL, NULL, job);\n\
                 }}\n\
                 \n"
            ));
        }
        for trampoline in &self.trampolines {
            out.push_str(trampoline);
            out.push('\n');
        }
        for function in &self.functions {
            out.push_str(function);
            out.push('\n');
        }
        // Each class's constructor: around a handle the module made, or the
        // library's `_new`.
        for (handle, _) in &exported {
            let ty = &handle.name;
            let made = match self.constructors.get(ty) {
                Some(function) => format!("return {function}(env, info);"),
                None => format!(
                    "napi_throw_type_error(env, NULL, \"{ty} has no constructor: the library makes it\");\n    \
                     return NULL;"
                ),
            };
            out.push_str(&format!(
                "static napi_value nk_class_{ty}(napi_env env, napi_callback_info info) {{\n    \
                 size_t argc = 1;\n    \
                 napi_value argv[1];\n    \
                 napi_value self;\n    \
                 napi_valuetype type = napi_undefined;\n    \
                 if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) return NULL;\n    \
                 if (argc >= 1) napi_typeof(env, argv[0], &type);\n    \
                 if (type == napi_external) {{\n        \
                 void *ptr = NULL;\n        \
                 napi_get_value_external(env, argv[0], &ptr);\n        \
                 return nk_adopt_{ty}(env, self, ptr);\n    \
                 }}\n    \
                 {made}\n\
                 }}\n\
                 \n"
            ));
        }
        out.push_str(&format!(
            "static napi_value nk_init(napi_env env, napi_callback_info info) {{\n    \
             int status = {prefix}_init();\n    \
             (void)info;\n    \
             if (status != 0) nk_fail(env, status);\n    \
             return NULL;\n\
             }}\n\
             \n\
             static napi_value nk_shutdown(napi_env env, napi_callback_info info) {{\n    \
             int status = {prefix}_shutdown();\n    \
             (void)info;\n    \
             if (status != 0) nk_fail(env, status);\n    \
             return NULL;\n\
             }}\n\
             \n\
             NAPI_MODULE_INIT() {{\n    \
             nk_state *state = calloc(1, sizeof *state);\n    \
             if (!state || napi_set_instance_data(env, state, nk_state_free, NULL) != napi_ok) return NULL;\n"
        ));
        let mut functions: Vec<String> = vec![
            "{ \"init\", NULL, nk_init, NULL, NULL, NULL, napi_enumerable, NULL }".to_string(),
            "{ \"shutdown\", NULL, nk_shutdown, NULL, NULL, NULL, napi_enumerable, NULL }"
                .to_string(),
        ];
        for (name, function) in &self.exported {
            functions.push(format!(
                "{{ \"{name}\", NULL, {function}, NULL, NULL, NULL, napi_enumerable, NULL }}"
            ));
        }
        out.push_str(&format!(
            "    {{\n        \
             napi_property_descriptor functions[] = {{\n            {}\n        }};\n        \
             napi_define_properties(env, exports, sizeof functions / sizeof *functions, functions);\n    \
             }}\n",
            functions.join(",\n            ")
        ));
        for (handle, _) in &exported {
            let ty = &handle.name;
            let mut members = vec![format!(
                "{{ \"close\", NULL, nk_close_{ty}, NULL, NULL, NULL, napi_default_method, NULL }}"
            )];
            members.extend(self.members.get(ty).into_iter().flatten().cloned());
            out.push_str(&format!(
                "    {{\n        \
                 napi_property_descriptor members[] = {{\n            {}\n        }};\n        \
                 napi_value class;\n        \
                 napi_define_class(env, \"{ty}\", NAPI_AUTO_LENGTH, nk_class_{ty}, NULL, sizeof members / sizeof *members, members, &class);\n        \
                 napi_create_reference(env, class, 1, &state->{ty});\n        \
                 napi_set_named_property(env, exports, \"{ty}\", class);\n    \
                 }}\n",
                members.join(",\n            ")
            ));
        }
        for plain in plains {
            let values: String = plain
                .variants
                .iter()
                .enumerate()
                .map(|(number, variant)| {
                    format!(
                        "        napi_set_named_property(env, choices, \"{variant}\", nk_from_i32(env, {number}));\n"
                    )
                })
                .collect();
            out.push_str(&format!(
                "    {{\n        \
                 napi_value choices;\n        \
                 napi_create_object(env, &choices);\n\
                 {values}        \
                 napi_object_freeze(env, choices);\n        \
                 napi_set_named_property(env, exports, \"{}\", choices);\n    \
                 }}\n",
                plain.name
            ));
        }
        for (owner, methods) in &self.methods {
            let listed: Vec<String> = methods
                .iter()
                .map(|(name, function)| {
                    format!(
                        "{{ \"{name}\", NULL, {function}, NULL, NULL, NULL, napi_enumerable, NULL }}"
                    )
                })
                .collect();
            out.push_str(&format!(
                "    {{\n        \
                 napi_value methods;\n        \
                 napi_property_descriptor listed[] = {{\n            {}\n        }};\n        \
                 napi_create_object(env, &methods);\n        \
                 napi_define_properties(env, methods, sizeof listed / sizeof *listed, listed);\n        \
                 napi_object_freeze(env, methods);\n        \
                 napi_set_named_property(env, exports, \"{owner}\", methods);\n    \
                 }}\n",
                listed.join(",\n            ")
            ));
        }
        out.push_str("    return exports;\n}\n");
        out
    }
}

/// `binding.gyp`, which builds the module against the library beside it.
pub(super) fn gyp(package: &str) -> String {
    format!(
        "# GENERATED by `nikaia bind node`. Do not edit.\n\
         {{\n  \
         \"targets\": [\n    {{\n      \
         \"target_name\": \"{package}\",\n      \
         \"sources\": [\"{package}.c\"],\n      \
         \"include_dirs\": [\"..\"],\n      \
         \"libraries\": [\"-L<(module_root_dir)/..\", \"-l{package}\", \"-Wl,-rpath,<(module_root_dir)/..\"]\n    \
         }}\n  \
         ]\n\
         }}\n"
    )
}
