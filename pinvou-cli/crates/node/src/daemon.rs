use crate::{NodeError, NodeInstanceLock, NodeSession, local_ipc::NodeLocalListener};

pub fn run_from_env() -> Result<(), NodeError> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["--check-config"] {
        println!("node configuration ok");
        return Ok(());
    }
    let endpoint = argument(&args, "--endpoint")?
        .to_string_lossy()
        .into_owned();
    let instance_id = argument(&args, "--instance-id")?
        .to_string_lossy()
        .into_owned();
    let lock_file = std::path::PathBuf::from(argument(&args, "--lock-file")?);
    let _lock = NodeInstanceLock::acquire(&lock_file)?;
    let session = NodeSession::new(instance_id)?;
    let mut listener = NodeLocalListener::bind(&endpoint)?;
    loop {
        listener.serve_one(&session)?;
    }
}

fn argument<'a>(
    args: &'a [std::ffi::OsString],
    name: &str,
) -> Result<&'a std::ffi::OsStr, NodeError> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_os_str())
        .ok_or(NodeError::Usage)
}
