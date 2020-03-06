use crate::core::package_id::PackageId;
use crate::util::errors::CargoResult;
use libloading::Library;
use rental::rental;
use self::rent_libloading::RentSymbol;
use std::sync::Mutex;
use std::process::{Command, Child};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;

rental! {
    mod rent_libloading {
        use libloading;

        #[rental(deref_suffix)] // This struct will deref to the Deref::Target of Symbol.
        pub struct RentSymbol<S: 'static> {
            lib: Box<libloading::Library>, // Library is boxed for StableDeref.
            sym: libloading::Symbol<'lib, S>, // The 'lib lifetime borrows lib.
        }
    }
}

lazy_static::lazy_static! {
    static ref HOOK: Mutex<Hook> = {
        let hook = Hook::new(HookKind::Process).expect("Can't start hook process");
        Mutex::new(hook)
    };
}

enum HookKind {
    Process,
    Plugin,
}

enum Hook {
    Process(HookProcess),
    Plugin(HookPlugin),
}

impl Hook {
    fn new(kind: HookKind) -> CargoResult<Self> {
        Ok(match kind {
            HookKind::Process => Hook::Process(HookProcess::new()?),
            HookKind::Plugin => Hook::Plugin(HookPlugin::new()?),
        })
    }
    fn hook(&mut self, id: PackageId) -> CargoResult<bool> {
        match self {
            Hook::Process(p) => p.hook(id),
            Hook::Plugin(p) => p.hook(id),
        }
    }
}


struct HookPlugin {
    sym: RentSymbol<extern fn(query: * const u8, len: usize) -> u32>,
}

impl HookPlugin {
    fn new() -> CargoResult<Self> {
        let lib = Library::new("libcargo_resolver_hook.so")?;
        let sym_res = rent_libloading::RentSymbol::try_new(
            Box::new(lib),
            |lib| unsafe { lib.get::<extern fn(query: * const u8, len: usize) -> u32>(b"query") });
        let sym = if let Ok(sym) = sym_res {
            sym
        } else {
            anyhow::bail!("error during library loading");
        };
        Ok(Self {
            sym,
        })
    }
    fn query(&self, s :&str) -> u32 {
        (self.sym)(s.as_ptr(), s.len())
    }
    fn hook(&mut self, id: PackageId) -> CargoResult<bool> {
        let req = serde_json::to_string(&id)?;
        let f = format!("{}\n", req);
        match self.query(&f) {
            1 => Ok(true),
            0 => Ok(false),
            v => anyhow::bail!("wrong code {}", v),
        }
    }
}

pub(crate) fn resolver_hook(id: PackageId) -> bool {
    HOOK.lock().unwrap().hook(id).unwrap()
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
