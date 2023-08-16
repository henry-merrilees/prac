use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use skim::prelude::*;
use std::collections::btree_map;
use std::io::Cursor;
use std::{
    collections::BTreeMap,
    env::var,
    fs::File,
    io::{BufReader, BufWriter},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context, Result};

pub mod utils;
use utils::TimeUnit;

#[derive(Serialize, Deserialize)]
pub struct Config {
    grace_period: GracePeriod,
    // TODO, add version so that at least we have a chance of being backwards compatible
    // potential implementation for new fields: use options on import, and let-else w/
    // defaults on save.
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grace_period: GracePeriod::Fractional(1.1),
        }
    }
}

#[derive(Serialize, Deserialize)]
enum GracePeriod {
    Fixed(Duration),
    Fractional(f64),
}

// TODO, solve backwards compatibility issue, see Config
#[derive(Serialize, Deserialize, Default)]
pub struct State {
    routines: BTreeMap<String, Practice>,
    config: Config,
}

#[derive(Serialize, Deserialize)]
pub struct Practice {
    pub created: SystemTime,
    // last time logged
    pub logged: SystemTime,
    // how often you wish to repeat practice
    pub period: Duration,
    // unique id of practice, will be used for retrieval
    pub name: String,
    // take notes
    pub body: String,
    pub cumulative: Duration,
    // TODO maybe a Completion struct? then a body enum {practice, Task} that contains Vec<Comepletion> for practice and raw
    // Completion for task. Trying not to prematurely optimize.
}

impl Practice {
    pub fn new(name: String, body: String, period: Duration) -> Self {
        let created = SystemTime::now();
        let logged = created;

        Self {
            created,
            logged,
            period,
            name,
            body,
            cumulative: Duration::new(0, 0),
        }
    }

    // TODO: find external crate for this
}

impl State {
    fn new() -> Self {
        Self::default()
    }

    fn find(&mut self, name: Option<&str>) -> Result<btree_map::OccupiedEntry<String, Practice>> {
        let name = name
            .context("name not provided")
            .map(String::from)
            .or_else(|_e| {
                let options = SkimOptionsBuilder::default().build().unwrap();

                let items = SkimItemReader::default().of_bufread(Cursor::new(
                    self.routines
                        .keys()
                        .map(|k| format!("{}", k))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));

                // TODO figure out what these errors acutally are
                let selected_items = Skim::run_with(&options, Some(items))
                    .context("Selection error.")?
                    .selected_items;

                // ensure only one item is selected
                let item = match selected_items.len() {
                    0 => bail!("No item selected"),
                    1 => selected_items[0].text(),
                    2.. => bail!("Multiple items selected"),
                    _ => unreachable!(),
                };

                Ok(item.into())
            })
            .context("failure to obtain name")?;

        match self.routines.entry(name) {
            btree_map::Entry::Vacant(_) => bail!("Practice not found."),
            btree_map::Entry::Occupied(o) => Ok(o),
        }
    }
}

/// The dead-simple practice-cultivating utility.
#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    command: SubCommand,
}

#[derive(Subcommand)]
enum SubCommand {
    /// Add a new practice, specifying name, body(optional), and period.
    Add {
        name: String,
        #[arg(short, long)]
        body: Option<String>,
        period: u64,
        #[arg(value_enum)]
        unit: TimeUnit,
    },
    /// Track a practice, specify name or select from list.
    Track {
        name: Option<String>,
        time: u64,
        #[arg(value_enum)]
        unit: TimeUnit,
    },
    /// Edit log, specify name or select from list.
    Log {
        name: Option<String>,
    },
    /// Edit period, specify name or select from list.
    EditPeriod {
        name: Option<String>,
        period: Option<u64>,
        #[arg(value_enum)]
        unit: Option<TimeUnit>,
    },
    /// View list of practices.
    List {
        /// show cumulative hours tracked alongside practices
        #[arg(short, long)]
        cumulative: bool,
        /// show period alongside practices
        #[arg(short, long)]
        period: bool,
    },
    /// Remove practice, specify name or select from list.
    Remove {
        name: Option<String>,
    },
    /// Rename practice.
    Rename {
        name: String,
        new_name: String,
    },
    // Reset progress bars. Equivalent to tracking all practices w/ zero time.
    Reset,
}

fn main() -> Result<()> {
    let home = dirs::home_dir().context("could not find home directory")?;
    let default_path = home.join(".practice"); // TODO to userdata home for easy git saving
    let state_path: PathBuf = var("PRACTICE_PATH")
        .map(PathBuf::from)
        .unwrap_or(default_path);

    // return file if exists, if open fails, tansform to create new file.

    // I did this way because open can error on more than just file non-existence
    // try_exists returns Ok(False) if confirmed not to exist, we need to handle.
    // ... bad default semantics, maybe use OpenOptions instead
    let mut state = if state_path.try_exists().is_ok_and(|b| b) {
        let state_file =
            File::open(&state_path).context("attempted to open existing path, but found error")?;
        serde_json::from_reader(BufReader::new(state_file)).context("failed to parse state")?
    } else {
        State::new()
    };

    let cli = Cli::parse();

    match cli.command {
        SubCommand::Add {
            name,
            body,
            period,
            unit,
        } => {
            let body = body.unwrap_or_else(|| utils::long_edit(None).unwrap());
            let new = Practice::new(name, body, utils::parse_time(period, unit));
            state.routines.insert(new.name.clone(), new);
        }
        SubCommand::Track { name, time, unit } => {
            let mut find = state.find(name.as_deref())?;
            let practice = find.get_mut();
            practice.logged = SystemTime::now();

            let duration = utils::parse_time(time, unit);
            practice.cumulative += duration;
        }
        SubCommand::Log { name } => {
            let mut find = state.find(name.as_deref())?;
            let practice = find.get_mut();
            let body = utils::long_edit(Some(practice.body.clone()))?;
            practice.body = body;
        }
        SubCommand::Remove { name } => {
            let practice = state.find(name.as_deref())?;
            practice.remove();
        }
        SubCommand::List { cumulative, period } => {
            let max_name_len = state
                .routines
                .keys()
                .map(|name| name.len())
                .max()
                .unwrap_or(0);
            let max_name_len = max_name_len.max(30);
            let term_width = termsize::get().context("failed to obtain termsize")?.cols;

            println!();
            let now = SystemTime::now();
            for (name, practice) in state.routines.iter() {
                let mut truncated_name = name.clone();
                truncated_name.truncate(max_name_len); // better way to do this?

                let start_message = format!(" {:>max_name_len$} ", name);

                let hours_cumulative = practice.cumulative.as_secs_f64() / 3600.0;
                let hours_period = practice.period.as_secs_f64() / 3600.0;

                let end_message = match (cumulative, period) {
                    (true, true) => format!(
                        " {:>4}h c  {:>4}h p ",
                        hours_cumulative as u64, hours_period as u64
                    ),
                    (true, false) => format!(" {:>4}h cumulative ", hours_cumulative as u64),
                    (false, true) => format!(" {:>4}h period ", hours_period as u64),
                    (false, false) => String::new(),
                };

                let elapsed = now
                    .duration_since(practice.logged)
                    .context("last log is in future")?;

                let grace_adjusted_period = match state.config.grace_period {
                    GracePeriod::Fixed(d) => (d + practice.period).as_secs_f64(),
                    GracePeriod::Fractional(f) => practice.period.as_secs_f64() * f,
                };
                let fraction = elapsed.as_secs_f64() / grace_adjusted_period;

                let bar_width = (term_width as usize)
                    .checked_sub(start_message.len() + end_message.len())
                    .context("terminal too narrow")?;

                let whole_bar = format!(
                    "{}{}{}",
                    start_message,
                    utils::bar(bar_width, fraction),
                    end_message
                );

                println!("{}", whole_bar);
            }
            println!();
        }
        SubCommand::Rename { name, new_name } => {
            let mut practice = state.routines.remove(&name).context("practice not found")?;
            practice.name = new_name.clone();
            state.routines.insert(new_name, practice);
        }
        SubCommand::Reset => {
            for practice in state.routines.values_mut() {
                practice.logged = SystemTime::now();
            }
        }
        SubCommand::EditPeriod { name, period, unit } => {
            let mut find = state.find(name.as_deref())?;
            let practice = find.get_mut();
            let period = utils::parse_time(period.unwrap_or(0), unit.unwrap_or(TimeUnit::Hours));
            practice.period = period;
        }
    }

    let state_file = File::create(state_path).context("failed to create state file")?;
    serde_json::to_writer_pretty(BufWriter::new(state_file), &state)
        .context("failed to write state to file")?;
    Ok(())
}
