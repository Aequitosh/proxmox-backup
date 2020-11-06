use anyhow::{format_err, Error};

use proxmox::api::router::SubdirMap;
use proxmox::{list_subdirs_api_method, sortable};
use proxmox::api::{api, ApiMethod, Permission, Router, RpcEnvironment};

use crate::api2::types::*;
use crate::server::do_verification_job;
use crate::server::jobstate::{Job, JobState};
use crate::config::acl::{
    PRIV_DATASTORE_AUDIT,
    PRIV_DATASTORE_VERIFY,
};
use crate::config::cached_user_info::CachedUserInfo;
use crate::config::verify;
use crate::config::verify::{VerificationJobConfig, VerificationJobStatus};
use serde_json::Value;
use crate::tools::systemd::time::{parse_calendar_event, compute_next_event};
use crate::server::UPID;

#[api(
    input: {
        properties: {
            store: {
                schema: DATASTORE_SCHEMA,
                optional: true,
            },
        },
    },
    returns: {
        description: "List configured jobs and their status (filtered by access)",
        type: Array,
        items: { type: verify::VerificationJobStatus },
    },
    access: {
        permission: &Permission::Anybody,
        description: "Requires Datastore.Audit or Datastore.Verify on datastore.",
    },
)]
/// List all verification jobs
pub fn list_verification_jobs(
    store: Option<String>,
    _param: Value,
    mut rpcenv: &mut dyn RpcEnvironment,
) -> Result<Vec<VerificationJobStatus>, Error> {
    let auth_id: Authid = rpcenv.get_auth_id().unwrap().parse()?;
    let user_info = CachedUserInfo::new()?;

    let required_privs = PRIV_DATASTORE_AUDIT | PRIV_DATASTORE_VERIFY;

    let (config, digest) = verify::config()?;

    let mut list: Vec<VerificationJobStatus> = config
        .convert_to_typed_array("verification")?
        .into_iter()
        .filter(|job: &VerificationJobStatus| {
            let privs = user_info.lookup_privs(&auth_id, &["datastore", &job.store]);
            if privs & required_privs == 0 {
                return false;
            }

            if let Some(store) = &store {
                &job.store == store
            } else {
                true
            }
        }).collect();

    for job in &mut list {
        let last_state = JobState::load("verificationjob", &job.id)
            .map_err(|err| format_err!("could not open statefile for {}: {}", &job.id, err))?;

        let (upid, endtime, state, starttime) = match last_state {
            JobState::Created { time } => (None, None, None, time),
            JobState::Started { upid } => {
                let parsed_upid: UPID = upid.parse()?;
                (Some(upid), None, None, parsed_upid.starttime)
            },
            JobState::Finished { upid, state } => {
                let parsed_upid: UPID = upid.parse()?;
                (Some(upid), Some(state.endtime()), Some(state.to_string()), parsed_upid.starttime)
            },
        };

        job.last_run_upid = upid;
        job.last_run_state = state;
        job.last_run_endtime = endtime;

        let last = job.last_run_endtime.unwrap_or_else(|| starttime);

        job.next_run = (|| -> Option<i64> {
            let schedule = job.schedule.as_ref()?;
            let event = parse_calendar_event(&schedule).ok()?;
            // ignore errors
            compute_next_event(&event, last, false).unwrap_or_else(|_| None)
        })();
    }

    rpcenv["digest"] = proxmox::tools::digest_to_hex(&digest).into();

    Ok(list)
}

#[api(
    input: {
        properties: {
            id: {
                schema: JOB_ID_SCHEMA,
            }
        }
    },
    access: {
        permission: &Permission::Anybody,
        description: "Requires Datastore.Verify on job's datastore.",
    },
)]
/// Runs a verification job manually.
fn run_verification_job(
    id: String,
    _info: &ApiMethod,
    rpcenv: &mut dyn RpcEnvironment,
) -> Result<String, Error> {
    let auth_id: Authid = rpcenv.get_auth_id().unwrap().parse()?;
    let user_info = CachedUserInfo::new()?;

    let (config, _digest) = verify::config()?;
    let verification_job: VerificationJobConfig = config.lookup("verification", &id)?;

    user_info.check_privs(&auth_id, &["datastore", &verification_job.store], PRIV_DATASTORE_VERIFY, true)?;

    let job = Job::new("verificationjob", &id)?;

    let upid_str = do_verification_job(job, verification_job, &auth_id, None)?;

    Ok(upid_str)
}

#[sortable]
const VERIFICATION_INFO_SUBDIRS: SubdirMap = &[("run", &Router::new().post(&API_METHOD_RUN_VERIFICATION_JOB))];

const VERIFICATION_INFO_ROUTER: Router = Router::new()
    .get(&list_subdirs_api_method!(VERIFICATION_INFO_SUBDIRS))
    .subdirs(VERIFICATION_INFO_SUBDIRS);

pub const ROUTER: Router = Router::new()
    .get(&API_METHOD_LIST_VERIFICATION_JOBS)
    .match_all("id", &VERIFICATION_INFO_ROUTER);
