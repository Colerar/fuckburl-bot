use googletest::{
  assert_that,
  matchers::{contains_substring, elements_are, eq, is_empty, matches_pattern, not},
};
use rstest::{fixture, rstest};

use super::*;

const KEEP_PAGE: &str = "[[rules]]\nhosts = ['example.com']\nkeep-query = ['page']\n";
const DROP_QUERY: &str = "[[rules]]\nhosts = ['example.com']\nkeep-query = []\n";
const INPUT: &str = "https://example.com/item?page=2&tracking=1";

struct TestFiles {
  directory: tempfile::TempDir,
  config: PathBuf,
}

#[fixture]
fn directory() -> tempfile::TempDir {
  tempfile::tempdir().expect("create configuration fixture directory")
}

#[fixture]
fn files(directory: tempfile::TempDir) -> TestFiles {
  let config = directory.path().join("config.toml");
  fs::write(
    &config,
    "telegram-token = 'fake'\nenabled-chats = ['old-chat']\n",
  )
  .expect("write main configuration fixture");
  fs::write(directory.path().join("replacer.toml"), KEEP_PAGE)
    .expect("write replacer configuration fixture");
  TestFiles { directory, config }
}

async fn next(watcher: &mut ConfigWatcher) -> Result<Arc<Runtime>> {
  tokio::time::timeout(Duration::from_secs(3), watcher.updates.changed())
    .await
    .context("Configuration watcher did not publish a new revision")?
    .context("Configuration watcher stopped before publishing")?;
  Ok(Arc::clone(&watcher.updates.borrow_and_update()))
}

#[rstest]
fn startup_creates_separate_files_relative_to_the_main_config(
  directory: tempfile::TempDir,
) -> Result<()> {
  let path = directory.path().join("nested/config.toml");
  assert_that!(initialize(&path)?, eq(true));
  let config = parse_config(&read_file(&path)?, &path)?;
  assert_that!(config.replacer_file, eq(&PathBuf::from("replacer.toml")));
  let sources = Sources::read(&path)?;
  assert_that!(
    sources.replacer_path,
    eq(&directory.path().join("nested/replacer.toml"))
  );
  let rules: ReplacerConfig = toml::from_str(&sources.replacer)?;
  assert_that!(rules.rules, not(is_empty()));
  fs::write(&sources.replacer_path, "rules = []")?;
  assert_that!(initialize(&path)?, eq(false));
  assert_that!(read_file(&sources.replacer_path)?, eq("rules = []"));
  Ok(())
}

#[rstest]
#[tokio::test]
async fn atomic_file_replacement_reloads_rules_and_preserves_in_flight_snapshots(
  files: TestFiles,
) -> Result<()> {
  let mut watcher = ConfigWatcher::start_with_interval(files.config, Duration::from_millis(20))?;
  let previous = Arc::clone(&watcher.updates.borrow_and_update());
  let temporary = files.directory.path().join("replacement.tmp");
  fs::write(&temporary, DROP_QUERY)?;
  fs::rename(temporary, files.directory.path().join("replacer.toml"))?;
  let current = next(&mut watcher).await?;
  assert_that!(
    current.replacer.replace_all(INPUT).await?,
    eq("https://example.com/item")
  );
  assert_that!(
    previous.replacer.replace_all(INPUT).await?,
    eq("https://example.com/item?page=2")
  );
  Ok(())
}

#[rstest]
fn unchanged_contents_preserve_the_same_snapshot(files: TestFiles) -> Result<()> {
  let mut reloader = Reloader::load(&files.config)?;
  let previous = Arc::clone(&reloader.current);
  fs::write(files.directory.path().join("replacer.toml"), KEEP_PAGE)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Unchanged));
  assert_that!(Arc::ptr_eq(&previous, &reloader.current), eq(true));
  Ok(())
}

#[rstest]
fn changing_or_reverting_a_pending_save_resets_confirmation(files: TestFiles) -> Result<()> {
  let mut reloader = Reloader::load(&files.config)?;
  let previous = Arc::clone(&reloader.current);
  let path = files.directory.path().join("replacer.toml");
  fs::write(&path, DROP_QUERY)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  fs::write(&path, "rules = []")?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  fs::write(&path, KEEP_PAGE)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Unchanged));
  assert_that!(Arc::ptr_eq(&previous, &reloader.current), eq(true));
  fs::write(&path, DROP_QUERY)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Reloaded));
  assert_that!(Arc::ptr_eq(&previous, &reloader.current), eq(false));
  Ok(())
}

#[rstest]
#[case::malformed_rules("replacer.toml", Some("[[rules"), "Failed to parse replacer config")]
#[case::empty_hosts(
  "replacer.toml",
  Some("[[rules]]\nhosts = []"),
  "hosts must not be empty"
)]
#[case::unknown_capture(
  "replacer.toml",
  Some("[[rules]]\nhosts = ['example.com']\nrewrite-path = '/{missing}'"),
  "Unknown rewrite-path capture"
)]
#[case::missing_rules("replacer.toml", None, "Failed to read config")]
#[case::malformed_main("config.toml", Some("telegram-token = ["), "Failed to parse config")]
#[case::empty_token(
  "config.toml",
  Some("telegram-token = ''"),
  "telegram-token must not be empty"
)]
#[case::empty_rule_path(
  "config.toml",
  Some("telegram-token = 'fake'\nreplacer-file = ''"),
  "replacer-file must not be empty"
)]
#[case::invalid_proxy(
  "config.toml",
  Some("telegram-token = 'fake'\nproxy = ''"),
  "Invalid proxy"
)]
#[case::missing_main("config.toml", None, "Failed to read config")]
#[tokio::test]
async fn rejected_files_preserve_the_snapshot_and_recover(
  files: TestFiles,
  #[case] name: &str,
  #[case] content: Option<&str>,
  #[case] expected_error: &str,
) -> Result<()> {
  let mut reloader = Reloader::load(&files.config)?;
  let previous = Arc::clone(&reloader.current);
  let path = files.directory.path().join(name);
  match content {
    Some(content) => fs::write(&path, content)?,
    None => fs::remove_file(&path)?,
  }
  let first = reloader.poll(&files.config);
  let outcome = if first == ReloadOutcome::Pending {
    reloader.poll(&files.config)
  } else {
    first
  };
  assert_that!(
    outcome,
    matches_pattern!(ReloadOutcome::Rejected(contains_substring(expected_error)))
  );
  assert_that!(reloader.poll(&files.config), eq(&outcome));
  assert_that!(Arc::ptr_eq(&previous, &reloader.current), eq(true));
  assert_that!(
    reloader.current.replacer.replace_all(INPUT).await?,
    eq("https://example.com/item?page=2")
  );
  if content.is_none() {
    assert_that!(path.exists(), eq(false));
  }
  let repaired = if name == "replacer.toml" {
    DROP_QUERY
  } else {
    "telegram-token = 'recovered'"
  };
  fs::write(path, repaired)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Reloaded));
  assert_that!(Arc::ptr_eq(&previous, &reloader.current), eq(false));
  let expected = if name == "replacer.toml" {
    "https://example.com/item"
  } else {
    "https://example.com/item?page=2"
  };
  assert_that!(
    reloader.current.replacer.replace_all(INPUT).await?,
    eq(expected)
  );
  Ok(())
}

#[rstest]
#[tokio::test]
async fn main_config_reload_switches_settings_and_the_watched_rules_path(
  files: TestFiles,
) -> Result<()> {
  let mut reloader = Reloader::load(&files.config)?;
  let previous = Arc::clone(&reloader.current);
  let rules_path = files.directory.path().join("rules/new.toml");
  fs::create_dir(rules_path.parent().context("rule fixture has no parent")?)?;
  fs::write(&rules_path, DROP_QUERY)?;
  fs::write(
    &files.config,
    r#"
    telegram-token = 'another-bot'
    enabled-chats = ['new-chat']
    proxy = 'http://127.0.0.1:12345'
    replacer-file = 'rules/new.toml'
    [time]
    fetch-delay = 12
    failed-delay = 34
  "#,
  )?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Reloaded));
  let current = Arc::clone(&reloader.current);
  assert_that!(&current.config.telegram_token, eq("another-bot"));
  assert_that!(
    &current.api.api_url,
    eq(format!("{}another-bot", frankenstein::BASE_API_URL).as_str())
  );
  assert_that!(&current.config.enabled_chats, elements_are![eq("new-chat")]);
  assert_that!(
    current.config.proxy.as_deref(),
    eq(Some("http://127.0.0.1:12345"))
  );
  assert_that!(current.config.time.fetch_delay, eq(12));
  assert_that!(current.config.time.failed_delay, eq(34));
  assert_that!(
    &current.config.replacer_file,
    eq(&PathBuf::from("rules/new.toml"))
  );
  assert_that!(
    current.replacer.replace_all(INPUT).await?,
    eq("https://example.com/item")
  );
  assert_that!(
    &previous.config.enabled_chats,
    elements_are![eq("old-chat")]
  );
  fs::write(
    files.directory.path().join("replacer.toml"),
    "invalid old file",
  )?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Unchanged));
  assert_that!(Arc::ptr_eq(&current, &reloader.current), eq(true));
  fs::write(rules_path, KEEP_PAGE)?;
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Pending));
  assert_that!(reloader.poll(&files.config), eq(&ReloadOutcome::Reloaded));
  assert_that!(
    reloader.current.replacer.replace_all(INPUT).await?,
    eq("https://example.com/item?page=2")
  );
  Ok(())
}

#[rstest]
fn absolute_rule_paths_and_partial_time_settings_are_supported(files: TestFiles) -> Result<()> {
  let absolute = files.directory.path().join("absolute.toml");
  let mut table: toml::Table = toml::from_str("telegram-token = 'fake'\n[time]\nfailed-delay = 1")?;
  table.insert(
    "replacer-file".to_string(),
    toml::Value::String(absolute.to_string_lossy().to_string()),
  );
  fs::write(&files.config, toml::to_string(&table)?)?;
  initialize(&files.config)?;
  let sources = Sources::read(&files.config)?;
  assert_that!(sources.replacer_path, eq(&absolute));
  let parsed = parse_config(&sources.config, &files.config)?;
  assert_that!(parsed.time.fetch_delay, eq(1000));
  assert_that!(parsed.time.failed_delay, eq(1));
  Ok(())
}
