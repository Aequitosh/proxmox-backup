use proxmox::api::cli::{run_cli_command, CliCommandMap, CliEnvironment};

mod inspect;
mod recover;

fn main() {
    let cmd_def = CliCommandMap::new()
        .insert("inspect", inspect::inspect_commands())
        .insert("recover", recover::recover_commands());

    let rpcenv = CliEnvironment::new();
    run_cli_command(cmd_def, rpcenv, Some(|future| pbs_runtime::main(future)));
}
