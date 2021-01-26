//! Client side interface to the proxmox backup server
//!
//! This library implements the client side to access the backups
//! server using https.

use anyhow::Error;

use crate::{
    api2::types::{Userid, Authid},
    tools::ticket::Ticket,
    auth_helpers::private_auth_key,
};



mod merge_known_chunks;
pub mod pipe_to_stream;

mod http_client;
pub use http_client::*;

mod task_log;
pub use task_log::*;

mod backup_reader;
pub use backup_reader::*;

mod backup_writer;
pub use backup_writer::*;

mod remote_chunk_reader;
pub use remote_chunk_reader::*;

mod pxar_backup_stream;
pub use pxar_backup_stream::*;

mod backup_repo;
pub use backup_repo::*;

mod backup_specification;
pub use backup_specification::*;

pub mod pull;

/// Connect to localhost:8007 as root@pam
///
/// This automatically creates a ticket if run as 'root' user.
pub fn connect_to_localhost() -> Result<HttpClient, Error> {

    let uid = nix::unistd::Uid::current();

    let client = if uid.is_root()  {
        let ticket = Ticket::new("PBS", Userid::root_userid())?
            .sign(private_auth_key(), None)?;
        let fingerprint = crate::tools::cert::CertInfo::new()?.fingerprint()?;
        let options = HttpClientOptions::new_non_interactive(ticket, Some(fingerprint));

        HttpClient::new("localhost", 8007, Authid::root_auth_id(), options)?
    } else {
        let options = HttpClientOptions::new_interactive(None, None);

        HttpClient::new("localhost", 8007, Authid::root_auth_id(), options)?
    };

    Ok(client)
}
