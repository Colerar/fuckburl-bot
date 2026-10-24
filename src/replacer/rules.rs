use std::collections::HashMap;

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use url::{Host, Url};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplacerConfig {
  #[serde(default)]
  pub rules: Vec<Rule>,
}

impl Default for ReplacerConfig {
  fn default() -> Self {
    toml::from_str(include_str!("../replacer.example.toml"))
      .expect("Invalid bundled replacer rules")
  }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Rule {
  hosts: Vec<String>,
  path: Option<String>,
  rewrite_host: Option<String>,
  rewrite_path: Option<String>,
  rewrite_scheme: Option<String>,
  keep_query: Option<Vec<String>>,
  #[serde(default)]
  pub resolve_redirect: bool,
}

pub struct Rules {
  by_host: HashMap<String, Vec<usize>>,
  paths: Vec<Option<PathPattern>>,
}

impl Rules {
  pub fn new(config: &ReplacerConfig) -> Result<Self> {
    let mut by_host: HashMap<String, Vec<usize>> = HashMap::new();
    let mut paths = Vec::new();
    for (index, rule) in config.rules.iter().enumerate() {
      let path = rule
        .validate()
        .with_context(|| format!("Invalid replacer rule #{}", index + 1))?;
      paths.push(path);
      for host in &rule.hosts {
        let host = Host::parse(host)?.to_string();
        let indices = by_host.entry(host).or_default();
        if !indices.contains(&index) {
          indices.push(index);
        }
      }
    }
    Ok(Self { by_host, paths })
  }

  pub fn contains_host(&self, host: &str) -> bool {
    Host::parse(host).is_ok_and(|host| self.by_host.contains_key(&host.to_string()))
  }

  pub fn dispatch<'a>(
    &self,
    config: &'a ReplacerConfig,
    url: &Url,
  ) -> Option<(&'a Rule, Captures)> {
    let indices = self.by_host.get(url.host_str()?)?;
    indices.iter().find_map(|&index| {
      let captures = match &self.paths[index] {
        Some(path) => path.captures(url.path())?,
        None => Vec::new(),
      };
      Some((&config.rules[index], captures))
    })
  }
}

impl Rule {
  fn validate(&self) -> Result<Option<PathPattern>> {
    ensure!(!self.hosts.is_empty(), "hosts must not be empty");
    for host in self.hosts.iter().chain(self.rewrite_host.iter()) {
      Host::parse(host).with_context(|| format!("Invalid host: {host}"))?;
    }
    if let Some(scheme) = &self.rewrite_scheme {
      ensure!(
        matches!(scheme.as_str(), "http" | "https"),
        "rewrite-scheme must be http or https"
      );
    }
    let path = self.path.as_deref().map(PathPattern::parse).transpose()?;
    if let Some(template) = &self.rewrite_path {
      ensure!(template.starts_with('/'), "rewrite-path must start with /");
      ensure!(
        !template.contains(['?', '#']),
        "rewrite-path cannot contain a query or fragment"
      );
      let mut rest = template.as_str();
      while let Some((literal, capture)) = rest.split_once('{') {
        ensure!(!literal.contains('}'), "Unmatched brace in rewrite-path");
        let (name, tail) = capture
          .split_once('}')
          .context("Unclosed rewrite-path capture")?;
        ensure!(
          path.as_ref().is_some_and(|path| path
            .0
            .iter()
            .any(|part| matches!(part, Part::Capture(n) if n == name))),
          "Unknown rewrite-path capture: {name}"
        );
        rest = tail;
      }
      ensure!(!rest.contains('}'), "Unmatched brace in rewrite-path");
    }
    Ok(path)
  }

  pub fn apply(&self, url: &mut Url, captures: Captures) -> Result<()> {
    if let Some(scheme) = &self.rewrite_scheme {
      url
        .set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("Invalid rewrite scheme"))?;
    }
    if let Some(host) = &self.rewrite_host {
      url.set_host(Some(host))?;
    }
    if let Some(template) = &self.rewrite_path {
      let mut path = String::new();
      let mut rest = template.as_str();
      while let Some((literal, capture)) = rest.split_once('{') {
        let (name, tail) = capture
          .split_once('}')
          .context("Unclosed rewrite-path capture")?;
        path.push_str(literal);
        let (_, value) = captures
          .iter()
          .find(|(key, _)| key == name)
          .context("Missing rewrite-path capture")?;
        path.push_str(value);
        rest = tail;
      }
      path.push_str(rest);
      url.set_path(&path);
    }
    if let Some(keys) = &self.keep_query {
      let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| keys.iter().any(|allowed| allowed == key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
      url.set_query(None);
      if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
      }
    }
    Ok(())
  }
}

type Captures = Vec<(String, String)>;

pub struct PathPattern(Vec<Part>);

enum Part {
  Literal(String),
  Capture(String),
  Any,
  Rest,
}

fn capture_name(segment: &str) -> Option<&str> {
  segment.strip_prefix('{')?.strip_suffix('}')
}

impl PathPattern {
  fn parse(pattern: &str) -> Result<Self> {
    ensure!(pattern.starts_with('/'), "path must start with /");
    let mut parts = Vec::new();
    let mut names = Vec::new();
    for segment in path_segments(pattern) {
      ensure!(!segment.is_empty(), "Empty path segment in pattern");
      let part = match segment {
        "*" => Part::Any,
        "**" => Part::Rest,
        _ => {
          if let Some(name) = capture_name(segment) {
            ensure!(
              !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
              "Invalid capture: {segment}"
            );
            ensure!(!names.contains(&name), "Duplicate capture: {name}");
            names.push(name);
            Part::Capture(name.to_string())
          } else if segment.contains(['{', '}', '*', '?', '#']) {
            bail!("Invalid path segment: {segment}");
          } else {
            Part::Literal(segment.to_string())
          }
        },
      };
      parts.push(part);
    }
    Ok(Self(parts))
  }

  fn captures(&self, path: &str) -> Option<Captures> {
    let segments = path_segments(path);
    match_path(&self.0, &segments)
  }
}

fn path_segments(path: &str) -> Vec<&str> {
  let path = path.strip_prefix('/').unwrap_or(path);
  let path = path.strip_suffix('/').unwrap_or(path);
  if path.is_empty() {
    Vec::new()
  } else {
    path.split('/').collect()
  }
}

fn match_path(parts: &[Part], segments: &[&str]) -> Option<Captures> {
  let Some((part, rest)) = parts.split_first() else {
    return segments.is_empty().then(Vec::new);
  };
  if matches!(part, Part::Rest) {
    return (0..=segments.len()).find_map(|skip| match_path(rest, &segments[skip..]));
  }
  let (segment, tail) = segments.split_first()?;
  if segment.is_empty() && matches!(part, Part::Capture(_) | Part::Any) {
    return None;
  }
  if let Part::Literal(literal) = part {
    if literal != segment {
      return None;
    }
  }
  let mut captures = match_path(rest, tail)?;
  if let Part::Capture(name) = part {
    captures.push((name.clone(), segment.to_string()));
  }
  Some(captures)
}
