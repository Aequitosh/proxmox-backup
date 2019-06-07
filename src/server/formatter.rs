use failure::*;
use serde_json::{json, Value};

use crate::api_schema::router::{HttpError, RpcEnvironment};

use hyper::{Body, Response, StatusCode};
use hyper::header;

/// Extension to set error message for server side logging
pub struct ErrorMessageExtension(pub String);

pub struct OutputFormatter {

    pub format_data: fn(data: Value, rpcenv: &dyn RpcEnvironment) -> Response<Body>,

    pub format_error: fn(err: Error) -> Response<Body>,
}

static JSON_CONTENT_TYPE: &str = "application/json;charset=UTF-8";

pub fn json_response(result: Result<Value, Error>) -> Response<Body> {
    match result {
        Ok(data) => json_data_response(data),
        Err(err) => json_error_response(err),
    }
}

pub fn json_data_response(data: Value) -> Response<Body> {

    let json_str = data.to_string();

    let raw = json_str.into_bytes();

    let mut response = Response::new(raw.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(JSON_CONTENT_TYPE));

    response
}

fn json_format_data(data: Value, rpcenv: &dyn RpcEnvironment) -> Response<Body> {

    let mut result = json!({
        "data": data
    });

    if let Some(total) = rpcenv.get_result_attrib("total").and_then(|v| v.as_u64()) {
        result["total"] = Value::from(total);
    }

    if let Some(changes) = rpcenv.get_result_attrib("changes") {
        result["changes"] = changes.clone();
    }

    json_data_response(result)
}

pub fn json_error_response(err: Error) -> Response<Body> {

    let mut response = if let Some(apierr) = err.downcast_ref::<HttpError>() {
        let mut resp = Response::new(Body::from(apierr.message.clone()));
        *resp.status_mut() = apierr.code;
        resp
    } else {
        let mut resp = Response::new(Body::from(err.to_string()));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        resp
    };

    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(JSON_CONTENT_TYPE));

    response.extensions_mut().insert(ErrorMessageExtension(err.to_string()));

    response
}

pub static JSON_FORMATTER: OutputFormatter = OutputFormatter {
    format_data: json_format_data,
    format_error: json_error_response,
};

fn extjs_format_data(data: Value, rpcenv: &dyn RpcEnvironment) -> Response<Body> {

    let mut result = json!({
        "data": data,
        "success": true
    });

    if let Some(total) = rpcenv.get_result_attrib("total").and_then(|v| v.as_u64()) {
        result["total"] = Value::from(total);
    }

    if let Some(changes) = rpcenv.get_result_attrib("changes") {
        result["changes"] = changes.clone();
    }


    json_data_response(result)
}

fn extjs_format_error(err: Error) -> Response<Body> {

    let mut errors = vec![];

    let message = err.to_string();
    errors.push(&message);

    let result = json!({
        "message": message,
        "errors": errors,
        "success": false
    });

    let mut response = json_data_response(result);

    response.extensions_mut().insert(ErrorMessageExtension(message));

    response
}

pub static EXTJS_FORMATTER: OutputFormatter = OutputFormatter {
    format_data: extjs_format_data,
    format_error: extjs_format_error,
};
