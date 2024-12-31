mod event;
mod replacer;

use frankenstein::reqwest::{Client, Proxy};
use log::{debug, info, LevelFilter};
use log4rs::{
  append::console::ConsoleAppender,
  config::{Appender, Root},
  encode::pattern::PatternEncoder,
};
use serde::Deserialize;

use std::{
  fs::{self, File},
  io::{BufReader, BufWriter, Read, Write},
  path::PathBuf,
  process,
  sync::{Arc, OnceLock},
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueHint};
use clap_verbosity_flag::{LogLevel, Verbosity, VerbosityFilter};
use frankenstein::{AllowedUpdate, AsyncApi, AsyncTelegramApi, GetUpdatesParams};

use crate::event::process_update;

#[derive(Parser, Debug)]
struct Cli {
  #[arg(short = 'c', long, value_name = "DIR")]
  #[arg(value_hint = ValueHint::FilePath)]
  config_file: Option<PathBuf>,
  #[clap(flatten)]
  verbose: Verbosity<DefaultLevel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "kebab-case"))]
struct Config {
  telegram_token: String,
  #[serde(default = "Default::default")]
  enabled_chats: Vec<String>,
  proxy: Option<String>,
  #[serde(default = "Default::default")]
  time: Time,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "kebab-case"))]
struct Time {
  #[allow(dead_code)]
  fetch_delay: u64,
  failed_delay: u64,
}

impl Default for Time {
  fn default() -> Self {
    Self {
      fetch_delay: 1000,
      failed_delay: 5000,
    }
  }
}

static START_TIME: OnceLock<u64> = OnceLock::new();

fn start_time() -> u64 {
  *START_TIME.get_or_init(|| {
    let start = SystemTime::now();
    let since_the_epoch = start
      .duration_since(UNIX_EPOCH)
      .expect("Time went backwards");
    since_the_epoch.as_secs()
  })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let args = Cli::parse();
  init_logger(args.verbose.log_level_filter());
  info!("Start at: {:?}", start_time());
  debug!("{args:?}");
  let config = init_config(args.config_file).context("Failed to init config file")?;
  let config = Arc::new(config);
  debug!("{config:?}");

  let mut cli = Client::builder();
  if let Some(proxy) = &config.proxy {
    let proxy =
      Proxy::all(proxy.clone()).with_context(|| format!("Failed to set \"{proxy}\" as proxy"))?;
    cli = cli.proxy(proxy);
  }
  let cli = cli.build()?;

  let tg_api = AsyncApi::builder()
    .api_url(format!(
      "{}{}",
      frankenstein::BASE_API_URL,
      &*config.telegram_token,
    ))
    .client(cli.clone())
    .build();
  let tg_api = Arc::new(tg_api);
  let me = tg_api
    .get_me()
    .await
    .context("Failed to get telegram bot self info")?;
  info!(
    "Current tg bot: {}",
    me.result
      .username
      .context("Failed to get username for bot, maybe token is invalid")?
  );

  let mut update_params = GetUpdatesParams::builder()
    .allowed_updates(vec![AllowedUpdate::Message])
    .build();

  loop {
    let result = tg_api.get_updates(&update_params).await;
    match result {
      Ok(response) => {
        if let Some(last) = response.result.last() {
          update_params = GetUpdatesParams::builder()
            .allowed_updates(vec![AllowedUpdate::Message])
            .offset(last.update_id as i64 + 1)
            .build();
        }

        for update in response.result {
          let api = Arc::clone(&tg_api);
          let config = Arc::clone(&config);
          tokio::spawn(async move {
            let result = process_update(&api, config, update)
              .await
              .with_context(|| "Failed to process update".to_string());
            if let Err(err) = result {
              log::error!("{err:?}");
            }
          });
        }
      },
      Err(error) => {
        log::error!("Failed to get updates: {error:?}");
        tokio::time::sleep(Duration::from_millis(config.time.failed_delay)).await;
      },
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

fn init_config(path: Option<PathBuf>) -> Result<Config> {
  let path = if let Some(dir) = path {
    dir
  } else if cfg!(debug_assertions) {
    std::env::current_dir()
      .context("Failed to get current dir")?
      .join("work_dir")
      .join("config.toml")
  } else {
    std::env::current_dir()
      .context("Failed to get current dir")?
      .join("config.toml")
  };

  info!("Initializing config file...");

  if path.exists() && path.is_file() {
    info!("Reading config from {}...", &path.to_string_lossy());
    let file = File::open(&path).context("Failed to")?;
    let mut buf_reader = BufReader::new(file);
    let mut config_str = String::new();
    buf_reader
      .read_to_string(&mut config_str)
      .with_context(|| {
        format!(
          "Failed to read config file as String: {}",
          &path.to_string_lossy()
        )
      })?;
    let config: Config = toml::from_str(&config_str)
      .with_context(|| format!("Failed to parse config file: {}", &path.to_string_lossy()))?;
    Ok(config)
  } else if !path.exists() {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create folder: {}", parent.to_string_lossy()))?;
    };
    let config = File::create(&path).with_context(|| {
      format!(
        "Failed to create default config: {}",
        &path.to_string_lossy()
      )
    })?;
    const DEFAULT_CONFIG: &[u8] = include_bytes!("config.example.toml");

    {
      let mut buf_writer = BufWriter::new(config);
      buf_writer.write_all(DEFAULT_CONFIG).with_context(|| {
        format!(
          "Failed to write default config to: {}",
          &path.to_string_lossy()
        )
      })?;
    }
    info!("Default config writed to {}", &path.to_string_lossy());
    info!("Please take a look and configure bot, exiting...");
    process::exit(0)
  } else {
    bail!("Path is not a file: {}", path.to_string_lossy())
  }
}
