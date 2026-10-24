use std::{
  fs::{self, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  sync::Arc,
  time::Duration,
};

use anyhow::{ensure, Context, Result};
use frankenstein::{
  client_reqwest::Bot,
  reqwest::{Client, Proxy},
};
use log::{error, info};
use serde::Deserialize;
use tokio::{sync::watch, task::JoinHandle};

use crate::replacer::{Replacer, ReplacerConfig};

const CONFIG_EXAMPLE: &str = include_str!("config.example.toml");
const REPLACER_EXAMPLE: &str = include_str!("replacer.example.toml");
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
  pub telegram_token: String,
  #[serde(default)]
  pub enabled_chats: Vec<String>,
  pub proxy: Option<String>,
  #[serde(default)]
  pub time: Time,
  #[serde(default = "default_replacer_file")]
  pub replacer_file: PathBuf,
}

fn default_replacer_file() -> PathBuf {
  PathBuf::from("replacer.toml")
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Time {
  pub fetch_delay: u64,
  pub failed_delay: u64,
}

impl Default for Time {
  fn default() -> Self {
    Self {
      fetch_delay: 1000,
      failed_delay: 5000,
    }
  }
}

pub struct Runtime {
  pub config: Arc<Config>,
  pub api: Bot,
  pub replacer: Replacer,
}

#[derive(Clone, PartialEq, Eq)]
struct Sources {
  config: String,
  replacer: String,
  replacer_path: PathBuf,
}

impl Sources {
  fn read(path: &Path) -> Result<Self> {
    let config = read_file(path)?;
    let parsed = parse_config(&config, path)?;
    let replacer_path = replacer_path(path, &parsed);
    let replacer = read_file(&replacer_path)?;
    Ok(Self {
      config,
      replacer,
      replacer_path,
    })
  }

  fn build(&self, path: &Path) -> Result<Runtime> {
    let config = parse_config(&self.config, path)?;
    ensure!(
      !config.telegram_token.trim().is_empty(),
      "telegram-token must not be empty"
    );
    let rules: ReplacerConfig = toml::from_str(&self.replacer).with_context(|| {
      format!(
        "Failed to parse replacer config: {}",
        self.replacer_path.display()
      )
    })?;
    let replacer = Replacer::new(&rules, config.proxy.as_deref())
      .with_context(|| format!("Invalid replacer config: {}", self.replacer_path.display()))?;
    let mut client = Client::builder()
      .connect_timeout(Duration::from_secs(10))
      .timeout(Duration::from_secs(90));
    if let Some(proxy) = &config.proxy {
      client = client.proxy(Proxy::all(proxy).context("Invalid proxy")?);
    }
    let api = Bot::builder()
      .api_url(format!(
        "{}{}",
        frankenstein::BASE_API_URL,
        config.telegram_token
      ))
      .client(client.build().context("Failed to build Telegram client")?)
      .build();
    Ok(Runtime {
      config: Arc::new(config),
      api,
      replacer,
    })
  }
}

fn read_file(path: &Path) -> Result<String> {
  fs::read_to_string(path).with_context(|| format!("Failed to read config: {}", path.display()))
}

fn parse_config(source: &str, path: &Path) -> Result<Config> {
  let config: Config = toml::from_str(source)
    .with_context(|| format!("Failed to parse config: {}", path.display()))?;
  ensure!(
    !config.replacer_file.as_os_str().is_empty(),
    "replacer-file must not be empty"
  );
  Ok(config)
}

fn replacer_path(path: &Path, config: &Config) -> PathBuf {
  path
    .parent()
    .unwrap_or_else(|| Path::new("."))
    .join(&config.replacer_file)
}

pub fn config_path(path: Option<PathBuf>) -> Result<PathBuf> {
  let path = path.unwrap_or_else(|| {
    if cfg!(debug_assertions) {
      PathBuf::from("work_dir/config.toml")
    } else {
      PathBuf::from("config.toml")
    }
  });
  // Resolve once, so later working-directory changes cannot redirect the watcher.
  Ok(
    std::env::current_dir()
      .context("Failed to get current directory")?
      .join(path),
  )
}

// Only initialize files at startup. A file removed during a reload must keep the
// last working configuration instead of silently restoring the default rules.
pub fn initialize(path: &Path) -> Result<bool> {
  let generated = create_missing(path, CONFIG_EXAMPLE)?;
  let config = parse_config(&read_file(path)?, path)?;
  let rules_path = replacer_path(path, &config);
  create_missing(&rules_path, REPLACER_EXAMPLE)?;
  Ok(generated)
}

fn create_missing(path: &Path, content: &str) -> Result<bool> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
  }
  let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
    Ok(file) => file,
    Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
    Err(error) => {
      return Err(error).with_context(|| format!("Failed to create config: {}", path.display()))
    },
  };
  file
    .write_all(content.as_bytes())
    .with_context(|| format!("Failed to write config: {}", path.display()))?;
  info!("Created configuration: {}", path.display());
  Ok(true)
}

pub struct ConfigWatcher {
  pub updates: watch::Receiver<Arc<Runtime>>,
  task: JoinHandle<()>,
}

impl ConfigWatcher {
  pub fn start(path: PathBuf) -> Result<Self> {
    Self::start_with_interval(path, POLL_INTERVAL)
  }

  fn start_with_interval(path: PathBuf, interval: Duration) -> Result<Self> {
    let reloader = Reloader::load(&path)?;
    let (sender, updates) = watch::channel(Arc::clone(&reloader.current));
    info!(
      "Watching {} and {}",
      path.display(),
      reloader.active.replacer_path.display()
    );
    let task = tokio::spawn(watch_files(path, interval, reloader, sender));
    Ok(Self { updates, task })
  }
}

impl Drop for ConfigWatcher {
  fn drop(&mut self) {
    self.task.abort();
  }
}

#[derive(Debug, PartialEq, Eq)]
enum ReloadOutcome {
  Unchanged,
  Pending,
  Rejected(String),
  Reloaded,
}

struct Reloader {
  active: Sources,
  pending: Option<Sources>,
  rejected: Option<(Sources, String)>,
  current: Arc<Runtime>,
}

impl Reloader {
  fn load(path: &Path) -> Result<Self> {
    let active = Sources::read(path)?;
    let current = Arc::new(active.build(path)?);
    Ok(Self {
      active,
      pending: None,
      rejected: None,
      current,
    })
  }

  // One complete scan. The async watcher runs this on the blocking pool; tests
  // can drive each scan directly without depending on scheduler timing.
  fn poll(&mut self, path: &Path) -> ReloadOutcome {
    let sources = match Sources::read(path) {
      Ok(sources) => sources,
      Err(error) => {
        self.pending = None;
        return ReloadOutcome::Rejected(format!("{error:#}"));
      },
    };
    if sources == self.active {
      self.pending = None;
      self.rejected = None;
      return ReloadOutcome::Unchanged;
    }
    if self.pending.as_ref() != Some(&sources) {
      self.pending = Some(sources);
      return ReloadOutcome::Pending;
    }
    if let Some((rejected, message)) = &self.rejected {
      if &sources == rejected {
        return ReloadOutcome::Rejected(message.clone());
      }
    }
    match sources.build(path) {
      Ok(runtime) => {
        self.active = sources;
        self.pending = None;
        self.rejected = None;
        self.current = Arc::new(runtime);
        ReloadOutcome::Reloaded
      },
      Err(error) => {
        let message = format!("{error:#}");
        self.rejected = Some((sources, message.clone()));
        ReloadOutcome::Rejected(message)
      },
    }
  }
}

async fn watch_files(
  path: PathBuf,
  interval: Duration,
  mut reloader: Reloader,
  sender: watch::Sender<Arc<Runtime>>,
) {
  let mut last_error = None;
  loop {
    tokio::time::sleep(interval).await;
    let read_path = path.clone();
    let scan = tokio::task::spawn_blocking(move || {
      let outcome = reloader.poll(&read_path);
      (reloader, outcome)
    })
    .await;
    let outcome;
    (reloader, outcome) = match scan {
      Ok(scan) => scan,
      Err(error) => {
        error!("Configuration watcher failed: {error}");
        return;
      },
    };
    match outcome {
      ReloadOutcome::Unchanged => {
        last_error = None;
      },
      ReloadOutcome::Pending => {},
      ReloadOutcome::Reloaded => {
        last_error = None;
        if sender.send(Arc::clone(&reloader.current)).is_err() {
          return;
        }
        info!(
          "Reloaded configuration: {} and {}",
          path.display(),
          reloader.active.replacer_path.display()
        );
      },
      ReloadOutcome::Rejected(message) => {
        if last_error.as_ref() != Some(&message) {
          error!("Configuration reload rejected; keeping the last valid configuration: {message}");
          last_error = Some(message);
        }
      },
    }
  }
}

#[cfg(test)]
mod tests;
