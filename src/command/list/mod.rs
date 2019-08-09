mod human;
mod plain;
mod toolchain;

use std::fmt;
use std::path::PathBuf;

use semver::Version;
use structopt::StructOpt;

use crate::command::Command;
use toolchain::Toolchain;
use volta_core::session::{ActivityKind, Session};
use volta_fail::{ExitCode, Fallible};

#[derive(Copy, Clone)]
enum Format {
    Human,
    Plain,
}

impl std::str::FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "human" => Ok(Format::Human),
            "plain" => Ok(Format::Plain),
            _ => Err("No".into()),
        }
    }
}

/// The source of a given item, from the perspective of a user.
///
/// Note: this is distinct from `volta_core::platform::sourced::Source`, which
/// represents the source only of a `Platform`, which is a composite structure.
/// By contrast, this `Source` is concerned *only* with a single item.
#[derive(Clone, PartialEq)]
enum Source {
    /// The item is from a project. The wrapped `PathBuf` is the path to the
    /// project's `package.json`.
    Project(PathBuf),

    /// The item is the user's default.
    Default,

    /// The item is one that has been *fetched* but is not *installed* anywhere.
    None,
}

impl Source {
    fn allowed_with(&self, filter: &Filter) -> bool {
        match filter {
            Filter::Default => self == &Source::Default,
            Filter::Current => match self {
                Source::Default | Source::Project(_) => true,
                _ => false,
            },
            Filter::None => true,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Source::Project(path) => format!(" (current @ {})", path.display()),
                Source::Default => String::from(" (default)"),
                Source::None => String::from(""),
            }
        )
    }
}

/// A package and its associated tools, for displaying to the user as part of
/// their toolchain.
struct Package {
    /// The name of the package.
    pub name: String,
    /// Where the package is specified.
    pub source: Source,
    /// The package's own version.
    pub version: Version,
    /// The version of Node the package is installed against.
    pub node: Version,
    /// The names of the tools associated with the package.
    pub tools: Vec<String>,
}

#[derive(Clone)]
struct Node {
    pub source: Source,
    pub version: Version,
}

#[derive(Clone)]
enum PackageManagerKind {
    Yarn,
    Npm,
}

impl fmt::Display for PackageManagerKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                PackageManagerKind::Npm => "npm",
                PackageManagerKind::Yarn => "yarn",
            }
        )
    }
}

#[derive(Clone)]
struct PackageManager {
    kind: PackageManagerKind,
    source: Source,
    version: Version,
}

/// How (if at all) should the list query be narrowed?
enum Filter {
    /// Display only the currently active tool(s).
    ///
    /// For example, if the user queries `volta list --current yarn`, show only
    /// the version of Yarn currently in use: project, default, or none.
    Current,

    /// Show only the user's default tool(s).
    ///
    /// For example, if the user queries `volta list --default node`, show only
    /// the user's default Node version.
    Default,

    /// Do not filter at all. Show all tool(s) matching the query.
    None,
}

#[derive(StructOpt)]
pub(crate) struct List {
    /// Display
    #[structopt(subcommand)]
    subcommand: Option<Subcommand>,

    /// Specify the output format.
    ///
    /// Defaults to `human` for TTYs, `plain` otherwise.
    #[structopt(long = "format", raw(possible_values = r#"&["human", "plain"]"#))]
    format: Option<Format>,

    /// Show the currently-active tool(s).
    ///
    /// Equivalent to `volta list` when not specifying a specific tool.
    #[structopt(long = "current", conflicts_with = "default")]
    current: bool,

    /// Show your default tool(s).
    #[structopt(long = "default", conflicts_with = "current")]
    default: bool,
}

#[derive(StructOpt)]
enum Subcommand {
    /// Show every item in the toolchain.
    #[structopt(name = "all")]
    All,

    /// Show locally cached Node versions.
    #[structopt(name = "node")]
    Node,

    /// Show locally cached Yarn versions.
    #[structopt(name = "yarn")]
    Yarn,

    /// Show locally cached versions of a package or a package binary.
    #[structopt(name = "<package or tool>")]
    PackageOrTool { name: String },
}

impl From<&str> for Subcommand {
    fn from(s: &str) -> Self {
        match s {
            "all" => Subcommand::All,
            "node" => Subcommand::Node,
            "yarn" => Subcommand::Yarn,
            s => Subcommand::PackageOrTool { name: s.into() },
        }
    }
}

impl List {
    fn output_format(&self) -> Format {
        // We start by checking if the user has explicitly set a value: if they
        // have, that trumps our TTY-checking. Then, if the user has *not*
        // specified an option, we use `Human` mode for TTYs and `Plain` for
        // non-TTY contexts.
        self.format.unwrap_or(if atty::is(atty::Stream::Stdout) {
            Format::Human
        } else {
            Format::Plain
        })
    }
}

impl Command for List {
    fn run(self, session: &mut Session) -> Fallible<ExitCode> {
        session.add_event_start(ActivityKind::List);

        let inventory = session.inventory()?;
        let project = session.project()?;
        let user_platform = session.user_platform()?;
        let format = match self.output_format() {
            Format::Human => human::format,
            Format::Plain => plain::format,
        };

        let filter = match (self.current, self.default) {
            (true, false) => Filter::Current,
            (false, true) => Filter::Default,
            (true, true) => unreachable!("simultaneous `current` and `default` forbidden by clap"),
            _ => Filter::None,
        };

        let toolchain = match self.subcommand {
            // For no subcommand, show the user's current toolchain
            None => Toolchain::active(&project, &user_platform, &inventory, &filter)?,
            Some(Subcommand::All) => Toolchain::all(&project, &user_platform, &inventory)?,
            Some(Subcommand::Node) => {
                Toolchain::node(&inventory, &project, &user_platform, &filter)
            }
            Some(Subcommand::Yarn) => {
                Toolchain::yarn(&inventory, &project, &user_platform, &filter)
            }
            Some(Subcommand::PackageOrTool { name }) => {
                Toolchain::package_or_tool(&name, inventory, &filter)?
            }
        };

        if let Some(string) = format(&toolchain) {
            println!("{}", string);
        };

        session.add_event_end(ActivityKind::List, ExitCode::Success);
        Ok(ExitCode::Success)
    }
}