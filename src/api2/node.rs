use failure::*;

use crate::tools;
use crate::api_schema::*;
use crate::api_schema::router::*;
use serde_json::{json};
use lazy_static::lazy_static;
use std::sync::Arc;

mod tasks;
mod time;
mod network;
mod dns;
mod syslog;
mod services;

lazy_static!{

    pub static ref NODE_SCHEMA: Arc<Schema> = Arc::new(
        StringSchema::new("Node name (or 'localhost')")
            .format(
                Arc::new(ApiStringFormat::VerifyFn(|node| {
                    if node == "localhost" || node == tools::nodename() {
                        Ok(())
                    } else {
                        Err(format_err!("no such node '{}'", node))
                    }
                }))
            )
            .into()
    );
}

pub fn router() -> Router {

    let route = Router::new()
        .get(ApiMethod::new(
            || Ok(json!([
                {"subdir": "dns"},
                {"subdir": "network"},
                {"subdir": "services"},
                {"subdir": "syslog"},
                {"subdir": "time"},
           ])),
            ObjectSchema::new("Directory index.")))
        .subdir("dns", dns::router())
        .subdir("network", network::router())
        .subdir("services", services::router())
        .subdir("syslog", syslog::router())
        .subdir("tasks", tasks::router())
        .subdir("time", time::router());

    route
}
