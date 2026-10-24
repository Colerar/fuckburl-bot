use std::ops::Range;

use url::Url;

use super::rules::Rules;

// Locate each URL once; rebuilding the message from these original spans avoids
// invalidating subsequent byte offsets when a replacement changes the length.
pub fn find_urls(text: &str, rules: &Rules) -> Vec<(Range<usize>, Url)> {
  let mut urls = Vec::new();
  let mut cursor = 0;
  let mut previous = None;
  while cursor < text.len() {
    let rest = &text[cursor..];
    let ch = rest.chars().next().unwrap();
    let has_scheme = has_http_scheme(rest);
    let boundary = previous.is_none_or(|c: char| {
      !c.is_ascii_alphanumeric()
        && if has_scheme {
          !"_@/\\".contains(c)
        } else {
          !"_@.-/:%&=?#\\".contains(c)
        }
    });
    if boundary && !is_delimiter(ch) {
      let length = candidate_length(rest, rules);
      let candidate = trim_punctuation(&rest[..length]);
      let scheme_relative = candidate.starts_with("//");
      let host = bare_host(candidate.strip_prefix("//").unwrap_or(candidate));
      // Unicode letters may themselves belong to an IDN. Do not recognize a
      // configured suffix inside a longer, unconfigured Unicode hostname.
      let bare_boundary = host.is_ascii() || previous.is_none_or(|c| !c.is_alphanumeric());
      if has_scheme || (bare_boundary && rules.contains_host(host)) {
        let parsed = if has_scheme {
          Url::parse(candidate)
        } else if scheme_relative {
          Url::parse(&format!("https:{candidate}"))
        } else {
          Url::parse(&format!("https://{candidate}"))
        };
        if let Ok(url) = parsed {
          if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
            let end = cursor + candidate.len();
            urls.push((cursor..end, url));
            cursor = end;
            previous = candidate.chars().next_back();
            continue;
          }
        }
      }
    }
    previous = Some(ch);
    cursor += ch.len_utf8();
  }
  urls
}

fn has_http_scheme(text: &str) -> bool {
  ["http://", "https://"].iter().any(|prefix| {
    text
      .get(..prefix.len())
      .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
  })
}

fn bare_host(text: &str) -> &str {
  let authority = text.split(['/', '?', '#']).next().unwrap_or("");
  if authority.starts_with('[') {
    authority
      .find(']')
      .map_or(authority, |end| &authority[..=end])
  } else {
    authority.split(':').next().unwrap_or("")
  }
}

fn candidate_length(text: &str, rules: &Rules) -> usize {
  let end = text.find(is_delimiter).unwrap_or(text.len());
  let token = &text[..end];
  let authority_start = if has_http_scheme(token) {
    token.find("://").map_or(0, |index| index + 3)
  } else if token.starts_with("//") {
    2
  } else {
    0
  };
  let path_start = token[authority_start..]
    .find(['/', '?', '#'])
    .map(|index| index + authority_start);
  let mut brackets = Vec::new();
  // Keep raw Unicode punctuation inside URLs. A punctuation-separated next URL
  // starts a new span, including when there is no intervening whitespace.
  for (index, ch) in token.char_indices() {
    if let Some(close) = matching_close(ch) {
      brackets.push(close);
    } else if ")]}）】》」』".contains(ch) && brackets.pop() != Some(ch) {
      // An outer wrapper ends the URL even when prose follows immediately.
      return index;
    }
    if is_separator(ch) {
      let next = token[index + ch.len_utf8()..].trim_start_matches(is_trailing_punctuation);
      if has_http_scheme(next)
        || (path_start.is_some_and(|start| index > start) && rules.contains_host(bare_host(next)))
      {
        return index;
      }
    }
  }
  end
}

fn matching_close(ch: char) -> Option<char> {
  match ch {
    '(' => Some(')'),
    '[' => Some(']'),
    '{' => Some('}'),
    '（' => Some('）'),
    '【' => Some('】'),
    '《' => Some('》'),
    '「' => Some('」'),
    '『' => Some('』'),
    _ => None,
  }
}

fn is_delimiter(ch: char) -> bool {
  ch.is_whitespace() || ch.is_control() || "<>\"'`".contains(ch)
}

fn is_separator(ch: char) -> bool {
  ",;，。！？；：、".contains(ch)
}

fn is_trailing_punctuation(ch: char) -> bool {
  ".,!?;:，。！？；：、".contains(ch)
}

fn trim_punctuation(text: &str) -> &str {
  text.trim_end_matches(is_trailing_punctuation)
}

#[cfg(test)]
mod tests;
