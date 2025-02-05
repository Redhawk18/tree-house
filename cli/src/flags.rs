use std::path::PathBuf;

xflags::xflags! {
    src "./src/flags.rs"

    cmd skidder {
        cmd import {
            /// Whether to import queries
            optional --import-queries
            /// Whether to (re)generate metadata
            optional --metadata
            /// The repository/directory where repos are copied into.
            /// Defaults to the current working directory
            optional -r,--repo repo: PathBuf
            /// The path of the grammars to import. The name of the directory
            /// will be used as the grammar name. To overwrite you can append
            /// the grammar name with a colon
            repeated path: PathBuf
        }
        cmd build {
            optional --verbose
            optional -j, --threads threads: usize
            optional -f, --force
            required repo: PathBuf
            optional grammar: String
        }
        cmd init-repo {
            required repo: PathBuf
        }
        cmd load-grammar {
            optional -r, --recursive
            required path: PathBuf
        }
        cmd regenerate-parser {
            optional -r, --recursive
            required path: PathBuf
        }
    }
}
// generated start
// The following code is generated by `xflags` macro.
// Run `env UPDATE_XFLAGS=1 cargo build` to regenerate.
#[derive(Debug)]
pub struct Skidder {
    pub subcommand: SkidderCmd,
}

#[derive(Debug)]
pub enum SkidderCmd {
    Import(Import),
    Build(Build),
    InitRepo(InitRepo),
    LoadGrammar(LoadGrammar),
    RegenerateParser(RegenerateParser),
}

#[derive(Debug)]
pub struct Import {
    pub path: Vec<PathBuf>,

    pub import_queries: bool,
    pub metadata: bool,
    pub repo: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Build {
    pub repo: PathBuf,
    pub grammar: Option<String>,

    pub verbose: bool,
    pub threads: Option<usize>,
    pub force: bool,
}

#[derive(Debug)]
pub struct InitRepo {
    pub repo: PathBuf,
}

#[derive(Debug)]
pub struct LoadGrammar {
    pub path: PathBuf,

    pub recursive: bool,
}

#[derive(Debug)]
pub struct RegenerateParser {
    pub path: PathBuf,

    pub recursive: bool,
}

impl Skidder {
    #[allow(dead_code)]
    pub fn from_env_or_exit() -> Self {
        Self::from_env_or_exit_()
    }

    #[allow(dead_code)]
    pub fn from_env() -> xflags::Result<Self> {
        Self::from_env_()
    }

    #[allow(dead_code)]
    pub fn from_vec(args: Vec<std::ffi::OsString>) -> xflags::Result<Self> {
        Self::from_vec_(args)
    }
}
// generated end
