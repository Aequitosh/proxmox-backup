use failure::*;

use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead,BufReader};

use proxmox::{sortable, identity};

use crate::tools;
use crate::api_schema::*;
use crate::api_schema::router::*;
use crate::api2::types::*;
use crate::server::{self, UPID};

fn get_task_status(
    param: Value,
    _info: &ApiMethod,
    _rpcenv: &mut dyn RpcEnvironment,
) -> Result<Value, Error> {

    let upid = extract_upid(&param)?;

    let mut result = json!({
        "upid": param["upid"],
        "node": upid.node,
        "pid": upid.pid,
        "pstart": upid.pstart,
        "starttime": upid.starttime,
        "type": upid.worker_type,
        "id": upid.worker_id,
        "user": upid.username,
    });

    if crate::server::worker_is_active(&upid) {
        result["status"] = Value::from("running");
    } else {
        let exitstatus = crate::server::upid_read_status(&upid).unwrap_or(String::from("unknown"));
        result["status"] = Value::from("stopped");
        result["exitstatus"] = Value::from(exitstatus);
    };

    Ok(result)
}

fn extract_upid(param: &Value) -> Result<UPID, Error> {

    let upid_str = tools::required_string_param(&param, "upid")?;

    let upid = match upid_str.parse::<UPID>() {
        Ok(v) => v,
        Err(err) => bail!("unable to parse UPID '{}' - {}", upid_str, err),
    };

    Ok(upid)
}

fn read_task_log(
    param: Value,
    _info: &ApiMethod,
    rpcenv: &mut dyn RpcEnvironment,
) -> Result<Value, Error> {

    let upid = extract_upid(&param)?;
    let start = param["start"].as_u64().unwrap_or(0);
    let mut limit = param["limit"].as_u64().unwrap_or(50);
    let mut count: u64 = 0;

    let path = upid.log_path();

    let file = File::open(path)?;

    let mut lines: Vec<Value> = vec![];

    for line in BufReader::new(file).lines() {
        match line {
            Ok(line) => {
                count += 1;
                if count < start { continue };
	        if limit == 0 { continue };

                lines.push(json!({ "n": count, "t": line }));

                limit -= 1;
            }
            Err(err) => {
                log::error!("reading task log failed: {}", err);
                break;
            }
        }
    }

    rpcenv.set_result_attrib("total", Value::from(count));

    Ok(json!(lines))
}

fn stop_task(
    param: Value,
    _info: &ApiMethod,
    _rpcenv: &mut dyn RpcEnvironment,
) -> Result<Value, Error> {

    let upid = extract_upid(&param)?;

    if crate::server::worker_is_active(&upid) {
        server::abort_worker_async(upid);
    }

    Ok(Value::Null)
}

fn list_tasks(
    param: Value,
    _info: &ApiMethod,
    rpcenv: &mut dyn RpcEnvironment,
) -> Result<Value, Error> {

    let start = param["start"].as_u64().unwrap_or(0);
    let limit = param["limit"].as_u64().unwrap_or(50);
    let errors = param["errors"].as_bool().unwrap_or(false);

    let userfilter = param["userfilter"].as_str();

    let list = server::read_task_list()?;

    let mut result = vec![];

    let mut count = 0;

    for info in list.iter() {
        let mut entry = json!({
            "upid": info.upid_str,
            "node": "localhost",
            "pid": info.upid.pid,
            "pstart": info.upid.pstart,
            "starttime": info.upid.starttime,
            "type": info.upid.worker_type,
            "id": info.upid.worker_id,
            "user": info.upid.username,
        });

        if let Some(username) = userfilter {
            if !info.upid.username.contains(username) { continue; }
        }

        if let Some(ref state) = info.state {
            if errors && state.1 == "OK" {
                continue;
            }

            entry["endtime"] = Value::from(state.0);
            entry["status"] = Value::from(state.1.clone());
        }

        if (count as u64) < start {
            count += 1;
            continue;
        } else {
            count += 1;
        }

        if (result.len() as u64) < limit { result.push(entry); };
    }

    rpcenv.set_result_attrib("total", Value::from(count));

    Ok(json!(result))
}

const UPID_SCHEMA: Schema = StringSchema::new("Unique Process/Task ID.")
    .max_length(256)
    .schema();

#[sortable]
const UPID_API_SUBDIRS: SubdirMap = &[
    (
        "log", &Router::new()
            .get(
                &ApiMethod::new(
                    &ApiHandler::Sync(&read_task_log),
                    &ObjectSchema::new(
                        "Read task log.",
                        &sorted!([
                            ("node", false, &NODE_SCHEMA),
                            ("upid", false, &UPID_SCHEMA),
                            ("start", true, &IntegerSchema::new("Start at this line.")
                             .minimum(0)
                             .default(0)
                             .schema()
                            ),
                            ("limit", true, &IntegerSchema::new("Only list this amount of lines.")
                             .minimum(0)
                             .default(50)
                             .schema()
                            ),
                        ]),
                    )
                )
            )
    ),
    (
        "status", &Router::new()
            .get(
                &ApiMethod::new(
                    &ApiHandler::Sync(&get_task_status),
                    &ObjectSchema::new(
                        "Get task status.",
                        &sorted!([
                            ("node", false, &NODE_SCHEMA),
                            ("upid", false, &UPID_SCHEMA),
                        ]),
                    )
                )
            )
    )
];

#[sortable]
pub const UPID_API_ROUTER: Router = Router::new()
    .get(&list_subdirs_api_method!(UPID_API_SUBDIRS))
    .delete(
        &ApiMethod::new(
            &ApiHandler::Sync(&stop_task),
            &ObjectSchema::new(
                "Try to stop a task.",
                &sorted!([
                    ("node", false, &NODE_SCHEMA),
                    ("upid", false, &UPID_SCHEMA),
                ]),
            )
        ).protected(true)
    )
    .subdirs(&UPID_API_SUBDIRS);

#[sortable]
pub const ROUTER: Router = Router::new()
    .get(
        &ApiMethod::new(
            &ApiHandler::Sync(&list_tasks),
            &ObjectSchema::new(
                "List tasks.",
                &sorted!([
                    ("node", false, &NODE_SCHEMA),
                    ("start", true, &IntegerSchema::new("List tasks beginning from this offset.")
                     .minimum(0)
                     .default(0)
                     .schema()
                    ),
                    ("limit", true, &IntegerSchema::new("Only list this amount of tasks.")
                     .minimum(0)
                     .default(50)
                     .schema()
                    ),
                    ("errors", true, &BooleanSchema::new("Only list erroneous tasks.").schema()),
                    ("userfilter", true, &StringSchema::new("Only list tasks from this user.").schema()),
                ]),
            )
        )
    )
    .match_all("upid", &UPID_API_ROUTER);
