use crate::core::package_id::PackageId;
use crate::util::errors::CargoResult;
use std::process::{Command, Child};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref HOOK_PROCESS: Mutex<HookProcess> = {
        let hpc = HookProcess::new().expect("Can't start hook process");
        Mutex::new(hpc)
    };
}

struct HookProcess {
    chld: Child,
    stdout: Box<dyn BufRead + Send>,
}

impl HookProcess {
    fn new() -> CargoResult<Self> {
        let mut chld = Command::new("cargo-resolver-hook")
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()?;
        let stdout = chld.stdout.take().expect("stdout needed");
        let stdout = Box::new(BufReader::new(stdout));
        Ok(Self {
            chld,
            stdout,
        })
    }
    fn hook(&mut self, id: PackageId) -> CargoResult<bool> {
        let stdin = self.chld.stdin.as_mut().unwrap();
        let req = serde_json::to_string(&id)?;
        write!(stdin, "{}\n", req)?;
        stdin.flush()?;
        let mut line_buf = String::new();
        self.stdout.read_line(&mut line_buf)?;
        let line = line_buf.trim();
        if line == "true" {
            Ok(true)
        } else if line == "false" {
            Ok(false)
        } else {
            anyhow::bail!("");
        }
    }
}

pub(crate) fn resolver_hook(id: PackageId) -> bool {
    HOOK_PROCESS.lock().unwrap().hook(id).unwrap()
}
