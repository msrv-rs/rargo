use crate::core::package_id::PackageId;
use crate::util::errors::CargoResult;
use crate::util::config::Config;
use libloading::Library;
use rental::rental;
use self::rent_libloading::RentSymbol;
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

#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
enum HookConfig {
    Process { path: String },
    Plugin { path: String },
}

pub struct Hook {
    kind: HookKind,
}

enum HookKind {
    Process(HookProcess),
    Plugin(HookPlugin),
    None,
}

impl Hook {
    pub fn from_config(config: &Config) -> CargoResult<Self> {
        let hk_cfg_opt = config.get::<Option<HookConfig>>("resolver_hook")?;
        let kind = if let Some(hk_cfg) = hk_cfg_opt {
            match hk_cfg {
                HookConfig::Process { path } => HookKind::Process(HookProcess::new(&path)?),
                HookConfig::Plugin { path } => HookKind::Plugin(HookPlugin::new(&path)?),
            }
        } else {
            HookKind::None
        };
        Ok(Hook {
            kind,
        })
    }
    pub fn hook(&mut self, id: PackageId) -> CargoResult<bool> {
        match &mut self.kind {
            HookKind::Process(ref mut p) => p.hook(id),
            HookKind::Plugin(ref mut p) => p.hook(id),
            HookKind::None => Ok(true),
        }
    }
}


struct HookPlugin {
    sym: RentSymbol<extern fn(query: * const u8, len: usize) -> u32>,
}

impl HookPlugin {
    fn new(path: &str) -> CargoResult<Self> {
        let lib = Library::new(path)?;
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

struct HookProcess {
    chld: Child,
    stdout: Box<dyn BufRead + Send>,
}

impl HookProcess {
    fn new(path: &str) -> CargoResult<Self> {
        let mut chld = Command::new(path)
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
            anyhow::bail!("expected true or false in hook response. Got: {}", line);
        }
    }
}
