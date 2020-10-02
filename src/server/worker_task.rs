use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::path::Path;
use std::io::{Read, Write, BufRead, BufReader};
use std::panic::UnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, format_err, Error};
use futures::*;
use lazy_static::lazy_static;
use nix::unistd::Pid;
use serde_json::{json, Value};
use serde::{Serialize, Deserialize};
use tokio::sync::oneshot;

use proxmox::sys::linux::procfs;
use proxmox::try_block;
use proxmox::tools::fs::{create_path, open_file_locked, replace_file, CreateOptions};

use super::UPID;

use crate::tools::logrotate::{LogRotate, LogRotateFiles};
use crate::tools::FileLogger;
use crate::api2::types::Userid;

macro_rules! PROXMOX_BACKUP_VAR_RUN_DIR_M { () => ("/run/proxmox-backup") }
macro_rules! PROXMOX_BACKUP_LOG_DIR_M { () => ("/var/log/proxmox-backup") }
macro_rules! PROXMOX_BACKUP_TASK_DIR_M { () => (concat!( PROXMOX_BACKUP_LOG_DIR_M!(), "/tasks")) }

pub const PROXMOX_BACKUP_VAR_RUN_DIR: &str = PROXMOX_BACKUP_VAR_RUN_DIR_M!();
pub const PROXMOX_BACKUP_LOG_DIR: &str = PROXMOX_BACKUP_LOG_DIR_M!();
pub const PROXMOX_BACKUP_TASK_DIR: &str = PROXMOX_BACKUP_TASK_DIR_M!();
pub const PROXMOX_BACKUP_TASK_LOCK_FN: &str = concat!(PROXMOX_BACKUP_TASK_DIR_M!(), "/.active.lock");
pub const PROXMOX_BACKUP_ACTIVE_TASK_FN: &str = concat!(PROXMOX_BACKUP_TASK_DIR_M!(), "/active");
pub const PROXMOX_BACKUP_INDEX_TASK_FN: &str = concat!(PROXMOX_BACKUP_TASK_DIR_M!(), "/index");
pub const PROXMOX_BACKUP_ARCHIVE_TASK_FN: &str = concat!(PROXMOX_BACKUP_TASK_DIR_M!(), "/archive");

const MAX_INDEX_TASKS: usize = 1000;

lazy_static! {
    static ref WORKER_TASK_LIST: Mutex<HashMap<usize, Arc<WorkerTask>>> = Mutex::new(HashMap::new());

    static ref MY_PID: i32 = unsafe { libc::getpid() };
    static ref MY_PID_PSTART: u64 = procfs::PidStat::read_from_pid(Pid::from_raw(*MY_PID))
        .unwrap()
        .starttime;
}

/// Test if the task is still running
pub async fn worker_is_active(upid: &UPID) -> Result<bool, Error> {
    if (upid.pid == *MY_PID) && (upid.pstart == *MY_PID_PSTART) {
        return Ok(WORKER_TASK_LIST.lock().unwrap().contains_key(&upid.task_id));
    }

    if !procfs::check_process_running_pstart(upid.pid, upid.pstart).is_some() {
        return Ok(false);
    }

    let socketname = format!(
        "\0{}/proxmox-task-control-{}.sock", PROXMOX_BACKUP_VAR_RUN_DIR, upid.pid);

    let cmd = json!({
        "command": "status",
        "upid": upid.to_string(),
    });

    let status = super::send_command(socketname, cmd).await?;

    if let Some(active) = status.as_bool() {
        Ok(active)
    } else {
        bail!("got unexpected result {:?} (expected bool)", status);
    }
}

/// Test if the task is still running (fast but inaccurate implementation)
///
/// If the task is spanned from a different process, we simply return if
/// that process is still running. This information is good enough to detect
/// stale tasks...
pub fn worker_is_active_local(upid: &UPID) -> bool {
    if (upid.pid == *MY_PID) && (upid.pstart == *MY_PID_PSTART) {
        WORKER_TASK_LIST.lock().unwrap().contains_key(&upid.task_id)
    } else {
        procfs::check_process_running_pstart(upid.pid, upid.pstart).is_some()
    }
}

pub fn create_task_control_socket() -> Result<(), Error> {

    let socketname = format!(
        "\0{}/proxmox-task-control-{}.sock", PROXMOX_BACKUP_VAR_RUN_DIR, *MY_PID);

    let control_future = super::create_control_socket(socketname, |param| {
        let param = param.as_object()
            .ok_or_else(|| format_err!("unable to parse parameters (expected json object)"))?;
        if param.keys().count() != 2 { bail!("wrong number of parameters"); }

        let command = param["command"].as_str()
            .ok_or_else(|| format_err!("unable to parse parameters (missing command)"))?;

        // we have only two commands for now
        if !(command == "abort-task" || command == "status") { bail!("got unknown command '{}'", command); }

        let upid_str = param["upid"].as_str()
            .ok_or_else(|| format_err!("unable to parse parameters (missing upid)"))?;

        let upid = upid_str.parse::<UPID>()?;

        if !((upid.pid == *MY_PID) && (upid.pstart == *MY_PID_PSTART)) {
            bail!("upid does not belong to this process");
        }

        let hash = WORKER_TASK_LIST.lock().unwrap();

        match command {
            "abort-task" => {
                if let Some(ref worker) = hash.get(&upid.task_id) {
                    worker.request_abort();
                } else {
                    // assume task is already stopped
                }
                Ok(Value::Null)
            }
            "status" => {
                let active = hash.contains_key(&upid.task_id);
                Ok(active.into())
            }
            _ => {
                bail!("got unknown command '{}'", command);
            }
        }
    })?;

    tokio::spawn(control_future);

    Ok(())
}

pub fn abort_worker_async(upid: UPID) {
    tokio::spawn(async move {
        if let Err(err) = abort_worker(upid).await {
            eprintln!("abort worker failed - {}", err);
        }
    });
}

pub async fn abort_worker(upid: UPID) -> Result<(), Error> {

    let target_pid = upid.pid;

    let socketname = format!(
        "\0{}/proxmox-task-control-{}.sock", PROXMOX_BACKUP_VAR_RUN_DIR, target_pid);

    let cmd = json!({
        "command": "abort-task",
        "upid": upid.to_string(),
    });

    super::send_command(socketname, cmd).map_ok(|_| ()).await
}

fn parse_worker_status_line(line: &str) -> Result<(String, UPID, Option<TaskState>), Error> {

    let data = line.splitn(3, ' ').collect::<Vec<&str>>();

    let len = data.len();

    match len {
        1 => Ok((data[0].to_owned(), data[0].parse::<UPID>()?, None)),
        3 => {
            let endtime = i64::from_str_radix(data[1], 16)?;
            let state = TaskState::from_endtime_and_message(endtime, data[2])?;
            Ok((data[0].to_owned(), data[0].parse::<UPID>()?, Some(state)))
        }
        _ => bail!("wrong number of components"),
    }
}

/// Create task log directory with correct permissions
pub fn create_task_log_dirs() -> Result<(), Error> {

    try_block!({
        let backup_user = crate::backup::backup_user()?;
        let opts = CreateOptions::new()
            .owner(backup_user.uid)
            .group(backup_user.gid);

        create_path(PROXMOX_BACKUP_LOG_DIR, None, Some(opts.clone()))?;
        create_path(PROXMOX_BACKUP_TASK_DIR, None, Some(opts.clone()))?;
        create_path(PROXMOX_BACKUP_VAR_RUN_DIR, None, Some(opts))?;
        Ok(())
    }).map_err(|err: Error| format_err!("unable to create task log dir - {}", err))?;

    Ok(())
}

/// Read endtime (time of last log line) and exitstatus from task log file
/// If there is not a single line with at valid datetime, we assume the
/// starttime to be the endtime
pub fn upid_read_status(upid: &UPID) -> Result<TaskState, Error> {

    let mut status = TaskState::Unknown { endtime: upid.starttime };

    let path = upid.log_path();

    let mut file = File::open(path)?;

    /// speedup - only read tail
    use std::io::Seek;
    use std::io::SeekFrom;
    let _ = file.seek(SeekFrom::End(-8192)); // ignore errors

    let mut data = Vec::with_capacity(8192);
    file.read_to_end(&mut data)?;

    // task logs should end with newline, we do not want it here
    if data.len() > 0 && data[data.len()-1] == b'\n' {
        data.pop();
    }

    let last_line = {
        let mut start = 0;
        for pos in (0..data.len()).rev() {
            if data[pos] == b'\n' {
                start = data.len().min(pos + 1);
                break;
            }
        }
        &data[start..]
    };

    let last_line = std::str::from_utf8(last_line)
        .map_err(|err| format_err!("upid_read_status: utf8 parse failed: {}", err))?;

    let mut iter = last_line.splitn(2, ": ");
    if let Some(time_str) = iter.next() {
        if let Ok(endtime) = proxmox::tools::time::parse_rfc3339(time_str) {
            if let Some(rest) = iter.next().and_then(|rest| rest.strip_prefix("TASK ")) {
                if let Ok(state) = TaskState::from_endtime_and_message(endtime, rest) {
                    status = state;
                }
            }
        }
    }

    Ok(status)
}

/// Task State
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// The Task ended with an undefined state
    Unknown { endtime: i64 },
    /// The Task ended and there were no errors or warnings
    OK { endtime: i64 },
    /// The Task had 'count' amount of warnings and no errors
    Warning { count: u64, endtime: i64 },
    /// The Task ended with the error described in 'message'
    Error { message: String, endtime: i64 },
}

impl TaskState {
    pub fn endtime(&self) -> i64 {
        match *self {
            TaskState::Unknown { endtime } => endtime,
            TaskState::OK { endtime } => endtime,
            TaskState::Warning { endtime, .. } => endtime,
            TaskState::Error { endtime, .. } => endtime,
        }
    }

    fn result_text(&self) -> String {
        match self {
            TaskState::Error { message, .. } => format!("TASK ERROR: {}", message),
            other => format!("TASK {}", other),
        }
    }

    fn from_endtime_and_message(endtime: i64, s: &str) -> Result<Self, Error> {
        if s == "unknown" {
            Ok(TaskState::Unknown { endtime })
        } else if s == "OK" {
            Ok(TaskState::OK { endtime })
        } else if s.starts_with("WARNINGS: ") {
            let count: u64 = s[10..].parse()?;
            Ok(TaskState::Warning{ count, endtime })
        } else if s.len() > 0 {
            let message = if s.starts_with("ERROR: ") { &s[7..] } else { s }.to_string();
            Ok(TaskState::Error{ message, endtime })
        } else {
            bail!("unable to parse Task Status '{}'", s);
        }
    }
}

impl std::cmp::PartialOrd for TaskState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.endtime().cmp(&other.endtime()))
    }
}

impl std::cmp::Ord for TaskState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.endtime().cmp(&other.endtime())
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskState::Unknown { .. } => write!(f, "unknown"),
            TaskState::OK { .. }=> write!(f, "OK"),
            TaskState::Warning { count, .. } => write!(f, "WARNINGS: {}", count),
            TaskState::Error { message, .. } => write!(f, "{}", message),
        }
    }
}

/// Task details including parsed UPID
///
/// If there is no `state`, the task is still running.
#[derive(Debug)]
pub struct TaskListInfo {
    /// The parsed UPID
    pub upid: UPID,
    /// UPID string representation
    pub upid_str: String,
    /// Task `(endtime, status)` if already finished
    pub state: Option<TaskState>, // endtime, status
}

fn lock_task_list_files(exclusive: bool) -> Result<std::fs::File, Error> {
    let backup_user = crate::backup::backup_user()?;

    let lock = open_file_locked(PROXMOX_BACKUP_TASK_LOCK_FN, std::time::Duration::new(10, 0), exclusive)?;
    nix::unistd::chown(PROXMOX_BACKUP_TASK_LOCK_FN, Some(backup_user.uid), Some(backup_user.gid))?;

    Ok(lock)
}

/// checks if the Task Archive is bigger that 'size_threshold' bytes, and
/// rotates it if it is
pub fn rotate_task_log_archive(size_threshold: u64, compress: bool, max_files: Option<usize>) -> Result<bool, Error> {
    let _lock = lock_task_list_files(true)?;
    let path = Path::new(PROXMOX_BACKUP_ARCHIVE_TASK_FN);
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) =>  bail!("unable to open task archive - {}", err),
    };

    if metadata.len() > size_threshold {
        let mut logrotate = LogRotate::new(PROXMOX_BACKUP_ARCHIVE_TASK_FN, compress).ok_or_else(|| format_err!("could not get archive file names"))?;
        let backup_user = crate::backup::backup_user()?;
        logrotate.rotate(
            CreateOptions::new()
                .owner(backup_user.uid)
                .group(backup_user.gid),
            max_files,
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// atomically read/update the task list, update status of finished tasks
// new_upid is added to the list when specified.
fn update_active_workers(new_upid: Option<&UPID>) -> Result<(), Error> {

    let backup_user = crate::backup::backup_user()?;

    let lock = lock_task_list_files(true)?;

    let mut finish_list: Vec<TaskListInfo> = read_task_file_from_path(PROXMOX_BACKUP_INDEX_TASK_FN)?;
    let mut active_list: Vec<TaskListInfo> = read_task_file_from_path(PROXMOX_BACKUP_ACTIVE_TASK_FN)?
        .into_iter()
        .filter_map(|info| {
            if info.state.is_some() {
                // this can happen when the active file still includes finished tasks
                finish_list.push(info);
                return None;
            }

            if !worker_is_active_local(&info.upid) {
                println!("Detected stopped UPID {}", &info.upid_str);
                let now = proxmox::tools::time::epoch_i64();
                let status = upid_read_status(&info.upid)
                    .unwrap_or_else(|_| TaskState::Unknown { endtime: now });
                finish_list.push(TaskListInfo {
                    upid: info.upid,
                    upid_str: info.upid_str,
                    state: Some(status)
                });
                return None;
            }

            Some(info)
        }).collect();

    if let Some(upid) = new_upid {
        active_list.push(TaskListInfo { upid: upid.clone(), upid_str: upid.to_string(), state: None });
    }

    let active_raw = render_task_list(&active_list);

    replace_file(
        PROXMOX_BACKUP_ACTIVE_TASK_FN,
        active_raw.as_bytes(),
        CreateOptions::new()
            .owner(backup_user.uid)
            .group(backup_user.gid),
    )?;

    finish_list.sort_unstable_by(|a, b| {
        match (&a.state, &b.state) {
            (Some(s1), Some(s2)) => s1.cmp(&s2),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => a.upid.starttime.cmp(&b.upid.starttime),
        }
    });


    let start = if finish_list.len() > MAX_INDEX_TASKS {
        finish_list.len() - MAX_INDEX_TASKS
    } else {
        0
    };

    let end = (start+MAX_INDEX_TASKS).min(finish_list.len());

    let index_raw = if end > start {
        render_task_list(&finish_list[start..end])
    } else {
        "".to_string()
    };

    replace_file(
        PROXMOX_BACKUP_INDEX_TASK_FN,
        index_raw.as_bytes(),
        CreateOptions::new()
            .owner(backup_user.uid)
            .group(backup_user.gid),
    )?;

    if !finish_list.is_empty() && start > 0 {
        match std::fs::OpenOptions::new().append(true).create(true).open(PROXMOX_BACKUP_ARCHIVE_TASK_FN) {
            Ok(mut writer) => {
                for info in &finish_list[0..start] {
                    writer.write_all(render_task_line(&info).as_bytes())?;
                }
            },
            Err(err) => bail!("could not write task archive - {}", err),
        }

        nix::unistd::chown(PROXMOX_BACKUP_ARCHIVE_TASK_FN, Some(backup_user.uid), Some(backup_user.gid))?;
    }

    drop(lock);

    Ok(())
}

fn render_task_line(info: &TaskListInfo) -> String {
    let mut raw = String::new();
    if let Some(status) = &info.state {
        raw.push_str(&format!("{} {:08X} {}\n", info.upid_str, status.endtime(), status));
    } else {
        raw.push_str(&info.upid_str);
        raw.push('\n');
    }

    raw
}

fn render_task_list(list: &[TaskListInfo]) -> String {
    let mut raw = String::new();
    for info in list {
        raw.push_str(&render_task_line(&info));
    }
    raw
}

// note this is not locked, caller has to make sure it is
// this will skip (and log) lines that are not valid status lines
fn read_task_file<R: Read>(reader: R) -> Result<Vec<TaskListInfo>, Error>
{
    let reader = BufReader::new(reader);
    let mut list = Vec::new();
    for line in reader.lines() {
        let line = line?;
        match parse_worker_status_line(&line) {
            Ok((upid_str, upid, state)) => list.push(TaskListInfo {
                upid_str,
                upid,
                state
            }),
            Err(err) => {
                eprintln!("unable to parse worker status '{}' - {}", line, err);
                continue;
            }
        };
    }

    Ok(list)
}

// note this is not locked, caller has to make sure it is
fn read_task_file_from_path<P>(path: P) -> Result<Vec<TaskListInfo>, Error>
where
    P: AsRef<std::path::Path> + std::fmt::Debug,
{
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => bail!("unable to open task list {:?} - {}", path, err),
    };

    read_task_file(file)
}

enum TaskFile {
    Active,
    Index,
    Archive,
    End,
}

pub struct TaskListInfoIterator {
    list: VecDeque<TaskListInfo>,
    file: TaskFile,
    archive: Option<LogRotateFiles>,
    lock: Option<File>,
}

impl TaskListInfoIterator {
    pub fn new(active_only: bool) -> Result<Self, Error> {
        let (read_lock, active_list) = {
            let lock = lock_task_list_files(false)?;
            let active_list = read_task_file_from_path(PROXMOX_BACKUP_ACTIVE_TASK_FN)?;

            let needs_update = active_list
                .iter()
                .any(|info| info.state.is_some() || !worker_is_active_local(&info.upid));

            if needs_update {
                drop(lock);
                update_active_workers(None)?;
                let lock = lock_task_list_files(false)?;
                let active_list = read_task_file_from_path(PROXMOX_BACKUP_ACTIVE_TASK_FN)?;
                (lock, active_list)
            } else {
                (lock, active_list)
            }
        };

        let archive = if active_only {
            None
        } else {
            let logrotate = LogRotate::new(PROXMOX_BACKUP_ARCHIVE_TASK_FN, true).ok_or_else(|| format_err!("could not get archive file names"))?;
            Some(logrotate.files())
        };

        let file = if active_only { TaskFile::End } else { TaskFile::Active };
        let lock = if active_only { None } else { Some(read_lock) };

        Ok(Self {
            list: active_list.into(),
            file,
            archive,
            lock,
        })
    }
}

impl Iterator for TaskListInfoIterator {
    type Item = Result<TaskListInfo, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(element) = self.list.pop_back() {
                return Some(Ok(element));
            } else {
                match self.file {
                    TaskFile::Active => {
                        let index = match read_task_file_from_path(PROXMOX_BACKUP_INDEX_TASK_FN) {
                            Ok(index) => index,
                            Err(err) => return Some(Err(err)),
                        };
                        self.list.append(&mut index.into());
                        self.file = TaskFile::Index;
                    },
                    TaskFile::Index | TaskFile::Archive => {
                        if let Some(mut archive) = self.archive.take() {
                            if let Some(file) = archive.next() {
                                let list = match read_task_file(file) {
                                    Ok(list) => list,
                                    Err(err) => return Some(Err(err)),
                                };
                                self.list.append(&mut list.into());
                                self.archive = Some(archive);
                                self.file = TaskFile::Archive;
                                continue;
                            }
                        }
                        self.file = TaskFile::End;
                        self.lock.take();
                        return None;
                    }
                    TaskFile::End => return None,
                }
            }
        }
    }
}

/// Launch long running worker tasks.
///
/// A worker task can either be a whole thread, or a simply tokio
/// task/future. Each task can `log()` messages, which are stored
/// persistently to files. Task should poll the `abort_requested`
/// flag, and stop execution when requested.
#[derive(Debug)]
pub struct WorkerTask {
    upid: UPID,
    data: Mutex<WorkerTaskData>,
    abort_requested: AtomicBool,
}

impl std::fmt::Display for WorkerTask {

    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.upid.fmt(f)
    }
}

#[derive(Debug)]
struct WorkerTaskData {
    logger: FileLogger,
    progress: f64, // 0..1
    warn_count: u64,
    pub abort_listeners: Vec<oneshot::Sender<()>>,
}

impl Drop for WorkerTask {

    fn drop(&mut self) {
        println!("unregister worker");
    }
}

impl WorkerTask {

    pub fn new(worker_type: &str, worker_id: Option<String>, userid: Userid, to_stdout: bool) -> Result<Arc<Self>, Error> {
        println!("register worker");

        let upid = UPID::new(worker_type, worker_id, userid)?;
        let task_id = upid.task_id;

        let mut path = std::path::PathBuf::from(PROXMOX_BACKUP_TASK_DIR);

        path.push(format!("{:02X}", upid.pstart % 256));

        let backup_user = crate::backup::backup_user()?;

        create_path(&path, None, Some(CreateOptions::new().owner(backup_user.uid).group(backup_user.gid)))?;

        path.push(upid.to_string());

        println!("FILE: {:?}", path);

        let logger = FileLogger::new(&path, to_stdout)?;
        nix::unistd::chown(&path, Some(backup_user.uid), Some(backup_user.gid))?;

        let worker = Arc::new(Self {
            upid: upid.clone(),
            abort_requested: AtomicBool::new(false),
            data: Mutex::new(WorkerTaskData {
                logger,
                progress: 0.0,
                warn_count: 0,
                abort_listeners: vec![],
            }),
        });

        // scope to drop the lock again after inserting
        {
            let mut hash = WORKER_TASK_LIST.lock().unwrap();
            hash.insert(task_id, worker.clone());
            super::set_worker_count(hash.len());
        }

        update_active_workers(Some(&upid))?;

        Ok(worker)
    }

    /// Spawn a new tokio task/future.
    pub fn spawn<F, T>(
        worker_type: &str,
        worker_id: Option<String>,
        userid: Userid,
        to_stdout: bool,
        f: F,
    ) -> Result<String, Error>
        where F: Send + 'static + FnOnce(Arc<WorkerTask>) -> T,
              T: Send + 'static + Future<Output = Result<(), Error>>,
    {
        let worker = WorkerTask::new(worker_type, worker_id, userid, to_stdout)?;
        let upid_str = worker.upid.to_string();
        let f = f(worker.clone());
        tokio::spawn(async move {
            let result = f.await;
            worker.log_result(&result);
        });

        Ok(upid_str)
    }

    /// Create a new worker thread.
    pub fn new_thread<F>(
        worker_type: &str,
        worker_id: Option<String>,
        userid: Userid,
        to_stdout: bool,
        f: F,
    ) -> Result<String, Error>
        where F: Send + UnwindSafe + 'static + FnOnce(Arc<WorkerTask>) -> Result<(), Error>
    {
        println!("register worker thread");

        let worker = WorkerTask::new(worker_type, worker_id, userid, to_stdout)?;
        let upid_str = worker.upid.to_string();

        let _child = std::thread::Builder::new().name(upid_str.clone()).spawn(move || {
            let worker1 = worker.clone();
            let result = match std::panic::catch_unwind(move || f(worker1)) {
                Ok(r) => r,
                Err(panic) => {
                    match panic.downcast::<&str>() {
                        Ok(panic_msg) => {
                            Err(format_err!("worker panicked: {}", panic_msg))
                        }
                        Err(_) => {
                            Err(format_err!("worker panicked: unknown type."))
                        }
                    }
                }
            };

            worker.log_result(&result);
        });

        Ok(upid_str)
    }

    /// create state from self and a result
    pub fn create_state(&self, result: &Result<(), Error>) -> TaskState {
        let warn_count = self.data.lock().unwrap().warn_count;

        let endtime = proxmox::tools::time::epoch_i64();

        if let Err(err) = result {
            TaskState::Error { message: err.to_string(), endtime }
        } else if warn_count > 0 {
            TaskState::Warning { count: warn_count, endtime }
        } else {
            TaskState::OK { endtime }
        }
    }

    /// Log task result, remove task from running list
    pub fn log_result(&self, result: &Result<(), Error>) {
        let state = self.create_state(result);
        self.log(state.result_text());

        WORKER_TASK_LIST.lock().unwrap().remove(&self.upid.task_id);
        let _ = update_active_workers(None);
        super::set_worker_count(WORKER_TASK_LIST.lock().unwrap().len());
    }

    /// Log a message.
    pub fn log<S: AsRef<str>>(&self, msg: S) {
        let mut data = self.data.lock().unwrap();
        data.logger.log(msg);
    }

    /// Log a message as warning.
    pub fn warn<S: AsRef<str>>(&self, msg: S) {
        let mut data = self.data.lock().unwrap();
        data.logger.log(format!("WARN: {}", msg.as_ref()));
        data.warn_count += 1;
    }

    /// Set progress indicator
    pub fn progress(&self, progress: f64) {
        if progress >= 0.0 && progress <= 1.0 {
            let mut data = self.data.lock().unwrap();
            data.progress = progress;
        } else {
           // fixme:  log!("task '{}': ignoring strange value for progress '{}'", self.upid, progress);
        }
    }

    /// Request abort
    pub fn request_abort(&self) {
        eprintln!("set abort flag for worker {}", self.upid);
        self.abort_requested.store(true, Ordering::SeqCst);
        // noitify listeners
        let mut data = self.data.lock().unwrap();
        loop {
            match data.abort_listeners.pop() {
                None => { break; },
                Some(ch) => {
                    let _ = ch.send(()); // ignore erros here
                },
            }
        }
    }

    /// Test if abort was requested.
    pub fn abort_requested(&self) -> bool {
        self.abort_requested.load(Ordering::SeqCst)
    }

    /// Fail if abort was requested.
    pub fn fail_on_abort(&self) -> Result<(), Error> {
        if self.abort_requested() {
            bail!("abort requested - aborting task");
        }
        Ok(())
    }

    /// Get a future which resolves on task abort
    pub fn abort_future(&self) ->  oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel::<()>();

        let mut data = self.data.lock().unwrap();
        if self.abort_requested() {
            let _ = tx.send(());
        } else {
            data.abort_listeners.push(tx);
        }
        rx
    }

    pub fn upid(&self) -> &UPID {
        &self.upid
    }
}
