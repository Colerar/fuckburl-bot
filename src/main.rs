mod config;
mod event;
mod replacer;

use std::{
  path::PathBuf,
  sync::{Arc, OnceLock},
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueHint};
use clap_verbosity_flag::{LogLevel, Verbosity, VerbosityFilter};
use frankenstein::{methods::GetUpdatesParams, types::AllowedUpdate, AsyncTelegramApi};
use log::{debug, info, LevelFilter};
use log4rs::{
  append::console::ConsoleAppender,
  config::{Appender, Root},
  encode::pattern::PatternEncoder,
};

use crate::{config::ConfigWatcher, event::process_update};

const GET_UPDATES_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Parser, Debug)]
struct Cli {
  #[arg(short = 'c', long, value_name = "FILE")]
  #[arg(value_hint = ValueHint::FilePath)]
  config_file: Option<PathBuf>,
  #[clap(flatten)]
  verbose: Verbosity<DefaultLevel>,
}

static START_TIME: OnceLock<u64> = OnceLock::new();

fn start_time() -> u64 {
  *START_TIME.get_or_init(|| {
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("Time went backwards")
      .as_secs()
  })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let args = Cli::parse();
  init_logger(args.verbose.log_level_filter());
  info!("Start at: {:?}", start_time());
  debug!("{args:?}");
  let path = config::config_path(args.config_file)?;
  if config::initialize(&path).context("Failed to initialize configuration files")? {
    info!("Please configure the bot, then run it again.");
    return Ok(());
  }
  let mut watcher = ConfigWatcher::start(path).context("Failed to load configuration")?;
  let initial = Arc::clone(&watcher.updates.borrow());
  let me = initial
    .api
    .get_me()
    .await
    .context("Failed to get Telegram bot self info")?;
  info!(
    "Current tg bot: {}",
    me.result
      .username
      .context("Failed to get bot username; check the token")?
  );

  let mut token = initial.config.telegram_token.clone();
  drop(initial);
  let mut update_params = GetUpdatesParams::builder()
    .allowed_updates(vec![AllowedUpdate::Message])
    .build();

  loop {
    let runtime = Arc::clone(&watcher.updates.borrow_and_update());
    if runtime.config.telegram_token != token {
      // Offsets belong to the bot account and cannot be reused for a new token.
      update_params = GetUpdatesParams::builder()
        .allowed_updates(vec![AllowedUpdate::Message])
        .build();
      token.clone_from(&runtime.config.telegram_token);
    }
    // Reloads cancel an outstanding long poll, so new clients and chat filters
    // take effect promptly rather than waiting for the request timeout.
    let result = tokio::select! {
      changed = watcher.updates.changed() => {
        changed.context("Configuration watcher stopped")?;
        continue;
      },
      result = tokio::time::timeout(GET_UPDATES_TIMEOUT, runtime.api.get_updates(&update_params)) => result,
    };
    let delay = match result {
      Ok(Ok(response)) => {
        if let Some(last) = response.result.last() {
          update_params = GetUpdatesParams::builder()
            .allowed_updates(vec![AllowedUpdate::Message])
            .offset(last.update_id as i64 + 1)
            .build();
        }
        for update in response.result {
          // Keep one immutable snapshot throughout photo and text processing.
          let runtime = Arc::clone(&runtime);
          tokio::spawn(async move {
            if let Err(error) = process_update(
              &runtime.api,
              Arc::clone(&runtime.config),
              &runtime.replacer,
              update,
            )
            .await
            {
              log::error!("Failed to process update: {error:#}");
            }
          });
        }
        runtime.config.time.fetch_delay
      },
      Ok(Err(error)) => {
        log::error!("Failed to get updates: {error:?}");
        runtime.config.time.failed_delay
      },
      Err(error) => {
        log::error!(
          "Timed out waiting for Telegram updates after {GET_UPDATES_TIMEOUT:?}: {error}"
        );
        runtime.config.time.failed_delay
      },
    };
    tokio::select! {
      changed = watcher.updates.changed() => { changed.context("Configuration watcher stopped")?; },
      _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
    }
  }
}

#[cfg(debug_assertions)]
type DefaultLevel = DebugLevel;

#[cfg(not(debug_assertions))]
type DefaultLevel = clap_verbosity_flag::InfoLevel;

#[derive(Copy, Clone, Debug, Default)]
pub struct DebugLevel;

impl LogLevel for DebugLevel {
  fn default_filter() -> VerbosityFilter {
    VerbosityFilter::Debug
  }
}

fn init_logger(verbosity: LevelFilter) {
  const PATTERN: &str = "{d(%m-%d %H:%M)} {h({l:.1})} - {h({m})}{n}";
  let stdout = ConsoleAppender::builder()
    .encoder(Box::new(PatternEncoder::new(PATTERN)))
    .build();
  let config = log4rs::Config::builder()
    .appender(Appender::builder().build("stdout", Box::new(stdout)))
    .build(Root::builder().appender("stdout").build(verbosity))
    .unwrap();
  log4rs::init_config(config).unwrap();
}
