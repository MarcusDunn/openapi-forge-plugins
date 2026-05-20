//! Per-operation rendering.
//!
//! For each [`ir::Operation`] we emit a small module containing:
//!  - a request struct holding path/query/header params + body
//!  - an `Output` enum with one variant per declared status code
//!  - an `Error` enum (`UndeclaredStatus`, decode failures, body failures, builder failures)
//!  - `impl runtime::Operation for <op>`
//!
//! Everything below builds `proc_macro2::TokenStream`s and the final file
//! is unparsed via prettyplease in [`crate::emit`].

use forge_plugin_sdk::diag;
use forge_plugin_sdk::ir;
use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use codegen_rust_serde::diagnostics;
use codegen_rust_serde::models::doc_attrs;
use codegen_rust_serde::naming;
use codegen_rust_serde::types::{self, type_ref_to_rust, ModelsPath};

fn models_path() -> ModelsPath {
    quote! { super::super::models }
}

pub fn render(spec: &ir::Ir, op: &ir::Operation) -> TokenStream {
    let struct_name = format_ident!("{}", naming::pascal_case(&op.id));
    let output_name = format_ident!("{}Output", naming::pascal_case(&op.id));
    let error_name = format_ident!("{}Error", naming::pascal_case(&op.id));

    let request_struct = render_request_struct(spec, op, &struct_name);
    let output_enum = render_output_enum(spec, op, &output_name);
    let error_enum = render_error_enum(&error_name);
    let op_impl = render_operation_impl(spec, op, &struct_name, &output_name, &error_name);

    quote! {
        #![allow(non_snake_case, non_camel_case_types, clippy::all, clippy::pedantic, clippy::nursery, unused_imports, unused_mut, dead_code)]

        use super::super::runtime;
        use serde::{Deserialize, Serialize};

        #request_struct
        #output_enum
        #error_enum
        #op_impl
    }
}

fn render_request_struct(spec: &ir::Ir, op: &ir::Operation, name: &Ident) -> TokenStream {
    let docs = doc_attrs(&op.documentation);
    let mut fields = TokenStream::new();
    for p in &op.path_params {
        let field = types::ident(&naming::snake_case(&p.name));
        let ty = type_ref_to_rust(spec, &p.r#type, &models_path());
        fields.extend(quote! { pub #field: #ty, });
    }
    for p in op.query_params.iter().chain(&op.header_params) {
        let field = types::ident(&naming::snake_case(&p.name));
        let ty = type_ref_to_rust(spec, &p.r#type, &models_path());
        let final_ty = if p.required {
            ty
        } else {
            quote! { Option<#ty> }
        };
        fields.extend(quote! { pub #field: #final_ty, });
    }
    if let Some(body) = &op.request_body {
        if let Some(json) = body
            .content
            .iter()
            .find(|c| is_json_media_type(&c.media_type))
        {
            let ty = type_ref_to_rust(spec, &json.r#type, &models_path());
            let final_ty = if body.required {
                ty
            } else {
                quote! { Option<#ty> }
            };
            fields.extend(quote! { pub body: #final_ty, });
        } else {
            let media_types: Vec<&str> =
                body.content.iter().map(|c| c.media_type.as_str()).collect();
            diagnostics::report_fatal(diag::error(
                "rust-tower/request-body-non-json",
                format!(
                    "operation `{}` has request body content types {:?} but no JSON family \
                     match; this generator only models JSON bodies",
                    op.id, media_types
                ),
            ));
            fields.extend(quote! { pub body: serde_json::Value, });
        }
    }
    quote! {
        #docs
        #[derive(Debug, Clone)]
        pub struct #name {
            #fields
        }
    }
}

fn render_output_enum(spec: &ir::Ir, op: &ir::Operation, name: &Ident) -> TokenStream {
    if op.responses.is_empty() {
        return quote! {
            #[derive(Debug)]
            pub enum #name {
                /// Spec declared no responses; treat any 2xx as success.
                Success,
            }
        };
    }
    let mut variants = TokenStream::new();
    for resp in &op.responses {
        let variant = format_ident!("{}", status_variant(&resp.status));
        match pick_json_response_body(resp) {
            Some(type_ref) => {
                let ty = type_ref_to_rust(spec, type_ref, &models_path());
                variants.extend(quote! { #variant(#ty), });
            }
            None => variants.extend(quote! { #variant, }),
        }
    }
    quote! {
        #[derive(Debug)]
        pub enum #name {
            #variants
        }
    }
}

fn render_error_enum(name: &Ident) -> TokenStream {
    quote! {
        #[derive(Debug, thiserror::Error)]
        pub enum #name {
            #[error("undeclared status {status}: {body}")]
            UndeclaredStatus { status: u16, body: String },
            #[error("json decode error: {0}")]
            Decode(#[from] serde_json::Error),
            #[error("response body collection failed: {0}")]
            Body(Box<dyn std::error::Error + Send + Sync + 'static>),
            #[error("http builder error: {0}")]
            HttpBuild(#[from] http::Error),
        }
    }
}

fn render_operation_impl(
    spec: &ir::Ir,
    op: &ir::Operation,
    struct_name: &Ident,
    output_name: &Ident,
    error_name: &Ident,
) -> TokenStream {
    let method = format_ident!("{}", op.method.as_str());
    let path_template = &op.path_template;
    let op_id = op.original_id.as_deref().unwrap_or(&op.id);

    let into_request = render_into_http_request(spec, op);
    let parse_response = render_parse_response(spec, op, output_name);

    quote! {
        impl runtime::Operation for #struct_name {
            type RequestBody = http_body_util::Full<bytes::Bytes>;
            type Output = #output_name;
            type Error = #error_name;

            const METHOD: http::Method = http::Method::#method;
            const PATH_TEMPLATE: &'static str = #path_template;
            const OPERATION_ID: &'static str = #op_id;

            #into_request

            #parse_response
        }
    }
}

fn render_into_http_request(spec: &ir::Ir, op: &ir::Operation) -> TokenStream {
    let mut field_idents: Vec<Ident> = Vec::new();
    for p in op
        .path_params
        .iter()
        .chain(&op.query_params)
        .chain(&op.header_params)
    {
        field_idents.push(types::ident(&naming::snake_case(&p.name)));
    }
    if op.request_body.is_some() {
        field_idents.push(format_ident!("body"));
    }
    let destructure = if field_idents.is_empty() {
        TokenStream::new()
    } else {
        quote! { let Self { #(#field_idents),* } = self; }
    };

    let path_stmt = render_path_stmt(op);
    let query_block = render_query_block(spec, op);
    let header_block = render_header_block(op);
    let body_stmt = render_body_stmt(op);

    quote! {
        fn into_http_request(self) -> Result<http::Request<Self::RequestBody>, Self::Error> {
            #destructure

            #path_stmt
            let mut builder = http::Request::builder()
                .method(Self::METHOD)
                .uri(path.as_str());
            #query_block
            #header_block
            #body_stmt
            Ok(builder.body(request_body)?)
        }
    }
}

fn render_path_stmt(op: &ir::Operation) -> TokenStream {
    let template = rewrite_path_for_format(&op.path_template, &op.path_params);
    if op.path_params.is_empty() {
        quote! { let path = String::from(#template); }
    } else {
        let args: Vec<TokenStream> = op
            .path_params
            .iter()
            .map(|p| {
                let snake = types::ident(&naming::snake_case(&p.name));
                types::format_arg(&snake)
            })
            .collect();
        quote! { let path = format!(#template, #(#args),*); }
    }
}

fn render_query_block(spec: &ir::Ir, op: &ir::Operation) -> TokenStream {
    if op.query_params.is_empty() {
        return TokenStream::new();
    }
    // OpenAPI 3 defaults `style=form, explode=true` for query params, which
    // means arrays serialize as repeated `key=value` pairs.
    //
    // The accumulator is named `__query_buf` (not `query`) so it can't
    // collide with a query *parameter* whose own name is `query` —
    // otherwise the param's destructured binding gets shadowed and the
    // borrow checker rejects `&query.to_string()` next to
    // `&mut query`.
    let mut emits = TokenStream::new();
    for p in &op.query_params {
        let snake = types::ident(&naming::snake_case(&p.name));
        let key = &p.name;
        let is_array = is_array_type(spec, &p.r#type);
        let emit = match (p.required, is_array) {
            (true, true) => quote! {
                for item in &#snake {
                    runtime::push_query(&mut __query_buf, #key, &item.to_string());
                }
            },
            (false, true) => quote! {
                if let Some(items) = &#snake {
                    for item in items {
                        runtime::push_query(&mut __query_buf, #key, &item.to_string());
                    }
                }
            },
            (true, false) => quote! {
                runtime::push_query(&mut __query_buf, #key, &#snake.to_string());
            },
            (false, false) => quote! {
                if let Some(v) = &#snake {
                    runtime::push_query(&mut __query_buf, #key, &v.to_string());
                }
            },
        };
        emits.extend(emit);
    }
    quote! {
        let mut __query_buf = String::new();
        #emits
        let final_uri = if __query_buf.is_empty() {
            path
        } else {
            format!("{path}?{__query_buf}")
        };
        builder = builder.uri(final_uri.as_str());
    }
}

fn render_header_block(op: &ir::Operation) -> TokenStream {
    if op.header_params.is_empty() {
        return TokenStream::new();
    }
    let mut emits = TokenStream::new();
    for p in &op.header_params {
        let snake = types::ident(&naming::snake_case(&p.name));
        let key = &p.name;
        let emit = if p.required {
            quote! { builder = builder.header(#key, #snake.to_string()); }
        } else {
            quote! {
                if let Some(v) = #snake.as_ref() {
                    builder = builder.header(#key, v.to_string());
                }
            }
        };
        emits.extend(emit);
    }
    emits
}

fn render_body_stmt(op: &ir::Operation) -> TokenStream {
    match &op.request_body {
        Some(body)
            if body
                .content
                .iter()
                .any(|c| is_json_media_type(&c.media_type)) =>
        {
            if body.required {
                quote! {
                    builder = builder.header(http::header::CONTENT_TYPE, "application/json");
                    let bytes = serde_json::to_vec(&body).map_err(Self::Error::Decode)?;
                    let request_body = http_body_util::Full::new(bytes::Bytes::from(bytes));
                }
            } else {
                quote! {
                    builder = builder.header(http::header::CONTENT_TYPE, "application/json");
                    let bytes = match body {
                        Some(b) => serde_json::to_vec(&b).map_err(Self::Error::Decode)?,
                        None => Vec::new(),
                    };
                    let request_body = http_body_util::Full::new(bytes::Bytes::from(bytes));
                }
            }
        }
        Some(_) => quote! {
            let request_body = http_body_util::Full::new(bytes::Bytes::new());
        },
        None => quote! {
            let request_body = http_body_util::Full::new(bytes::Bytes::new());
        },
    }
}

fn render_parse_response(spec: &ir::Ir, op: &ir::Operation, output_name: &Ident) -> TokenStream {
    // Rust matches arms top-to-bottom and `400..=499` shadows a later `400 =>`.
    // Partition so explicit codes come first, then range arms, then `default`
    // falls into the catch-all arm.
    let explicit: Vec<&ir::Response> = op
        .responses
        .iter()
        .filter(|r| matches!(r.status, ir::ResponseStatus::Explicit { .. }))
        .collect();
    let ranges: Vec<&ir::Response> = op
        .responses
        .iter()
        .filter(|r| matches!(r.status, ir::ResponseStatus::Range { .. }))
        .collect();
    let default_resp = op
        .responses
        .iter()
        .find(|r| matches!(r.status, ir::ResponseStatus::Default));

    let mut arms = TokenStream::new();
    for resp in explicit.iter().chain(ranges.iter()) {
        let variant = format_ident!("{}", status_variant(&resp.status));
        let codes = numeric_status_patterns(&resp.status);
        let body = pick_json_response_body(resp);
        for pat in codes {
            let arm = match body {
                Some(_) => quote! {
                    #pat => Ok(#output_name::#variant(serde_json::from_slice(&bytes)?)),
                },
                None => quote! { #pat => Ok(#output_name::#variant), },
            };
            arms.extend(arm);
        }
    }
    if op.responses.is_empty() {
        arms.extend(quote! {
            s if (200..300).contains(&s) => Ok(#output_name::Success),
        });
    }
    let catch_all = match default_resp {
        Some(resp) => {
            let variant = format_ident!("{}", status_variant(&resp.status));
            match pick_json_response_body(resp) {
                Some(_) => quote! {
                    _ => Ok(#output_name::#variant(serde_json::from_slice(&bytes)?)),
                },
                None => quote! { _ => Ok(#output_name::#variant), },
            }
        }
        None => quote! {
            status => Err(Self::Error::UndeclaredStatus {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            }),
        },
    };
    let _ = spec;
    quote! {
        async fn parse_response<B>(
            resp: http::Response<B>,
        ) -> Result<Self::Output, Self::Error>
        where
            B: http_body::Body + Send + 'static,
            B::Data: Send,
            B::Error: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
        {
            let (parts, body) = resp.into_parts();
            let bytes = runtime::collect_bytes(body)
                .await
                .map_err(|e| Self::Error::Body(e.into()))?;
            match parts.status.as_u16() {
                #arms
                #catch_all
            }
        }
    }
}

/// Rewrite `/foo/{rawName}/bar` to `/foo/{rust_name}/bar` so it composes
/// with `format!(..., rust_name = rust_name)`.
fn rewrite_path_for_format(path: &str, path_params: &[ir::Parameter]) -> String {
    let mut out = String::with_capacity(path.len());
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            let end = chars[i..].iter().position(|&c| c == '}').map(|p| i + p);
            match end {
                Some(end) => {
                    let raw: String = chars[i + 1..end].iter().collect();
                    let mapped = path_params
                        .iter()
                        .find(|p| p.name == raw)
                        .map(|p| naming::snake_case(&p.name))
                        .unwrap_or_else(|| naming::snake_case(&raw));
                    // Strip any `r#` prefix — `format!`'s named-arg syntax
                    // doesn't accept raw idents on the LHS.
                    let stripped = mapped.strip_prefix("r#").unwrap_or(&mapped).to_string();
                    out.push('{');
                    out.push_str(&stripped);
                    out.push('}');
                    i = end + 1;
                    continue;
                }
                None => {
                    out.push(c);
                    i += 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Recognize JSON-shaped content types: `application/json`, the `+json`
/// structured-syntax suffix (`application/problem+json`,
/// `application/vnd.api+json`), and either with trailing media-type
/// parameters (`; charset=utf-8`). Case-insensitive per RFC 9110.
fn is_json_media_type(media_type: &str) -> bool {
    let essence = media_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();
    essence == "application/json"
        || essence
            .strip_prefix("application/")
            .is_some_and(|rest| rest.ends_with("+json"))
}

/// Whether a parameter's type ref resolves to a `TypeDef::Array`. Drives
/// query-string codegen: arrays serialize as repeated `key=value` pairs.
fn is_array_type(spec: &ir::Ir, type_ref: &str) -> bool {
    spec.types
        .iter()
        .find(|t| t.id == type_ref)
        .is_some_and(|t| matches!(t.definition, ir::TypeDef::Array(_)))
}

fn pick_json_response_body(resp: &ir::Response) -> Option<&str> {
    resp.content
        .iter()
        .find(|c| is_json_media_type(&c.media_type))
        .map(|c| c.r#type.as_str())
}

/// Variant name on the per-op response enum for a given status.
fn status_variant(s: &ir::ResponseStatus) -> String {
    match s {
        ir::ResponseStatus::Explicit { code } => {
            well_known_variant(*code).unwrap_or_else(|| format!("Status{code}"))
        }
        ir::ResponseStatus::Range { class } => match class {
            1 => "OneXx".to_string(),
            2 => "TwoXx".to_string(),
            3 => "ThreeXx".to_string(),
            4 => "FourXx".to_string(),
            5 => "FiveXx".to_string(),
            other => format!("Status{other}xx"),
        },
        ir::ResponseStatus::Default => "Default".to_string(),
    }
}

/// Match patterns for a [`ir::ResponseStatus`]. Explicit codes produce a
/// `200` literal; ranges produce `200..=299`; `Default` is handled by the
/// catch-all arm and returns no pattern of its own.
fn numeric_status_patterns(s: &ir::ResponseStatus) -> Vec<TokenStream> {
    match s {
        ir::ResponseStatus::Explicit { code } => {
            let lit = Literal::u16_unsuffixed(*code);
            vec![quote! { #lit }]
        }
        ir::ResponseStatus::Range { class } => match class {
            1 => vec![quote! { 100..=199 }],
            2 => vec![quote! { 200..=299 }],
            3 => vec![quote! { 300..=399 }],
            4 => vec![quote! { 400..=499 }],
            5 => vec![quote! { 500..=599 }],
            _ => vec![],
        },
        ir::ResponseStatus::Default => vec![],
    }
}

fn well_known_variant(code: u16) -> Option<String> {
    let n = match code {
        100 => "Continue",
        101 => "SwitchingProtocols",
        200 => "Ok",
        201 => "Created",
        202 => "Accepted",
        203 => "NonAuthoritativeInformation",
        204 => "NoContent",
        205 => "ResetContent",
        206 => "PartialContent",
        300 => "MultipleChoices",
        301 => "MovedPermanently",
        302 => "Found",
        303 => "SeeOther",
        304 => "NotModified",
        307 => "TemporaryRedirect",
        308 => "PermanentRedirect",
        400 => "BadRequest",
        401 => "Unauthorized",
        402 => "PaymentRequired",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        406 => "NotAcceptable",
        407 => "ProxyAuthenticationRequired",
        408 => "RequestTimeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "LengthRequired",
        412 => "PreconditionFailed",
        413 => "PayloadTooLarge",
        414 => "RequestUriTooLong",
        415 => "UnsupportedMediaType",
        416 => "RangeNotSatisfiable",
        417 => "ExpectationFailed",
        422 => "UnprocessableEntity",
        425 => "TooEarly",
        426 => "UpgradeRequired",
        428 => "PreconditionRequired",
        429 => "TooManyRequests",
        431 => "RequestHeaderFieldsTooLarge",
        500 => "InternalServerError",
        501 => "NotImplemented",
        502 => "BadGateway",
        503 => "ServiceUnavailable",
        504 => "GatewayTimeout",
        505 => "HttpVersionNotSupported",
        _ => return None,
    };
    Some(n.to_string())
}
