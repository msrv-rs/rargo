use crate::core::package_id::PackageId;
use crate::util::errors::CargoResult;
use crate::util::config::Config;
use libloading::Library;
use rental::rental;
use self::rent_libloading::RentSymbols;
use std::process::{Command, Child};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::ffi::c_void;

rental! {
    mod rent_libloading {
        use libloading;

        #[rental] // This struct will deref to the Deref::Target of Symbol.
        pub(crate) struct RentSymbols {
            lib: Box<libloading::Library>, // Library is boxed for StableDeref.
            symbols: super::Symbols<'lib>,
        }
    }
}

pub(crate) struct Symbols<'lib> {
    sym_init: libloading::Symbol<'lib, extern fn(params: * const u8, len: usize) -> * mut c_void>,
    sym_query: libloading::Symbol<'lib, extern fn(ctx: * mut c_void, query: * const u8, len: usize) -> u32>,
    sym_delete: libloading::Symbol<'lib, extern fn(* mut c_void)>,
}

impl<'lib> Symbols<'lib> {
    fn new(lib: &'lib libloading::Library) -> libloading::Result<Self> {
        unsafe {
            Ok(Self {
                sym_init: lib.get(b"plugin_init")?,
                sym_query: lib.get(b"plugin_query")?,
                sym_delete: lib.get(b"plugin_delete")?,
            })
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "lowercase")]
enum HookConfig {
    Process {
        path: String,
    },
    Plugin {
        path: String,
        params: Option<toml::Value>,
    },
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
                HookConfig::Process { path } => {
                    HookKind::Process(HookProcess::new(&path)?)
                },
                HookConfig::Plugin { path, params } => {
                    HookKind::Plugin(HookPlugin::new(&path, &params)?)
                },
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
    syms: RentSymbols,
    plugin_ctx: * mut c_void,
}

impl HookPlugin {
    fn new(path: &str, params: &Option<toml::Value>) -> CargoResult<Self> {
        let params_str = if let Some(params) = params {
            toml::to_string(params)?
        } else {
            String::new()
        };
        let lib = Library::new(path)?;
        let syms_res = rent_libloading::RentSymbols::try_new(
            Box::new(lib),
            |lib| Symbols::new(lib),
        );
        let syms = if let Ok(syms) = syms_res {
            syms
        } else {
            anyhow::bail!("error during library loading");
        };
        let plugin_ctx = syms.rent(|syms|(syms.sym_init)(params_str.as_ptr(), params_str.len()));
        Ok(Self {
            syms,
            plugin_ctx,
        })
    }
    fn query(&self, s :&str) -> u32 {
        self.syms.rent(|syms| (syms.sym_query)(self.plugin_ctx, s.as_ptr(), s.len()))
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

impl Drop for HookPlugin {
    fn drop(&mut self) {
        self.syms.rent(|syms| (syms.sym_delete)(self.plugin_ctx));
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
