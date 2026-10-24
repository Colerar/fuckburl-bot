use anyhow::{Context, Result};
use googletest::{
  assert_that,
  matchers::{eq, is_empty},
};
use rstest::{fixture, rstest};

use super::*;
use crate::replacer::ReplacerConfig;

#[fixture]
fn rules() -> Rules {
  let config: ReplacerConfig = toml::from_str(
    r#"
    [[rules]]
    hosts = ["example.com", "www.amazon.co.jp", "例子.测试", "münich.example", "[::1]"]
  "#,
  )
  .expect("parse URL extraction fixture");
  Rules::new(&config).expect("compile URL extraction fixture")
}

#[rstest]
#[case::unicode_path(
  "前言 https://example.com/中文/😊 结束",
  "https://example.com/中文/😊",
  "https://example.com/%E4%B8%AD%E6%96%87/%F0%9F%98%8A"
)]
#[case::unicode_query(
  "链接 https://example.com/search?标题=你好&emoji=😊",
  "https://example.com/search?标题=你好&emoji=😊",
  "https://example.com/search?%E6%A0%87%E9%A2%98=%E4%BD%A0%E5%A5%BD&emoji=%F0%9F%98%8A"
)]
#[case::unicode_fragment(
  "https://example.com/目录#章节一",
  "https://example.com/目录#章节一",
  "https://example.com/%E7%9B%AE%E5%BD%95#%E7%AB%A0%E8%8A%82%E4%B8%80"
)]
#[case::mixed_encoding(
  "https://example.com/%E4%B8%AD文?q=%F0%9F%98%8A你好",
  "https://example.com/%E4%B8%AD文?q=%F0%9F%98%8A你好",
  "https://example.com/%E4%B8%AD%E6%96%87?q=%F0%9F%98%8A%E4%BD%A0%E5%A5%BD"
)]
#[case::unicode_punctuation(
  "https://example.com/【你好，世界】/（测试）?q=你好，世界",
  "https://example.com/【你好，世界】/（测试）?q=你好，世界",
  "https://example.com/%E3%80%90%E4%BD%A0%E5%A5%BD%EF%BC%8C%E4%B8%96%E7%95%8C%E3%80%91/%EF%BC%88%E6%B5%8B%E8%AF%95%EF%BC%89?q=%E4%BD%A0%E5%A5%BD%EF%BC%8C%E4%B8%96%E7%95%8C"
)]
#[case::unicode_domain(
  "https://例子.测试/中文?标题=你好",
  "https://例子.测试/中文?标题=你好",
  "https://xn--fsqu00a.xn--0zwm56d/%E4%B8%AD%E6%96%87?%E6%A0%87%E9%A2%98=%E4%BD%A0%E5%A5%BD"
)]
#[case::unicode_domain_separator(
  "https://例子。测试/中文",
  "https://例子。测试/中文",
  "https://xn--fsqu00a.xn--0zwm56d/%E4%B8%AD%E6%96%87"
)]
#[case::unicode_hostname_suffix(
  "https://bad。example.com/路径",
  "https://bad。example.com/路径",
  "https://bad.example.com/%E8%B7%AF%E5%BE%84"
)]
#[case::http_prefix(
  "HTTP://EXAMPLE.COM/中文",
  "HTTP://EXAMPLE.COM/中文",
  "http://example.com/%E4%B8%AD%E6%96%87"
)]
#[case::bare_path(
  "看example.com/中文?标题=你好。",
  "example.com/中文?标题=你好",
  "https://example.com/%E4%B8%AD%E6%96%87?%E6%A0%87%E9%A2%98=%E4%BD%A0%E5%A5%BD"
)]
#[case::bare_hostname("example.com", "example.com", "https://example.com/")]
#[case::bare_port(
  "example.com:8443/中文",
  "example.com:8443/中文",
  "https://example.com:8443/%E4%B8%AD%E6%96%87"
)]
#[case::bare_case(
  "WWW.AMAZON.CO.JP/【新作】/dp/B00NLZUM36",
  "WWW.AMAZON.CO.JP/【新作】/dp/B00NLZUM36",
  "https://www.amazon.co.jp/%E3%80%90%E6%96%B0%E4%BD%9C%E3%80%91/dp/B00NLZUM36"
)]
#[case::bare_unicode_domain(
  "链接：例子.测试/中文?q=😊",
  "例子.测试/中文?q=😊",
  "https://xn--fsqu00a.xn--0zwm56d/%E4%B8%AD%E6%96%87?q=%F0%9F%98%8A"
)]
#[case::bare_unicode_separator(
  "例子。测试/中文",
  "例子。测试/中文",
  "https://xn--fsqu00a.xn--0zwm56d/%E4%B8%AD%E6%96%87"
)]
#[case::bare_latin_idn(
  "münich.example/straße?q=café",
  "münich.example/straße?q=café",
  "https://xn--mnich-kva.example/stra%C3%9Fe?q=caf%C3%A9"
)]
#[case::scheme_relative(
  "//example.com/中文?q=😊",
  "//example.com/中文?q=😊",
  "https://example.com/%E4%B8%AD%E6%96%87?q=%F0%9F%98%8A"
)]
#[case::ipv6(
  "http://[::1]:8080/中文",
  "http://[::1]:8080/中文",
  "http://[::1]:8080/%E4%B8%AD%E6%96%87"
)]
#[case::bare_ipv6(
  "[::1]:8080/中文",
  "[::1]:8080/中文",
  "https://[::1]:8080/%E4%B8%AD%E6%96%87"
)]
#[case::wrapped_unicode(
  "（https://example.com/（测试）?q=中文）",
  "https://example.com/（测试）?q=中文",
  "https://example.com/%EF%BC%88%E6%B5%8B%E8%AF%95%EF%BC%89?q=%E4%B8%AD%E6%96%87"
)]
#[case::markdown(
  "[图片](https://example.com/图片【春季】.png)",
  "https://example.com/图片【春季】.png",
  "https://example.com/%E5%9B%BE%E7%89%87%E3%80%90%E6%98%A5%E5%AD%A3%E3%80%91.png"
)]
#[case::wrapped_prose(
  "前言（https://example.com/中文）后续说明",
  "https://example.com/中文",
  "https://example.com/%E4%B8%AD%E6%96%87"
)]
#[case::bare_markdown_prose(
  "[链接](example.com/路径)后续说明",
  "example.com/路径",
  "https://example.com/%E8%B7%AF%E5%BE%84"
)]
fn extracts_original_spans_and_parses_unicode_and_bare_urls(
  rules: Rules,
  #[case] text: &str,
  #[case] original: &str,
  #[case] normalized: &str,
) -> Result<()> {
  let found = find_urls(text, &rules);
  assert_that!(found.len(), eq(1));
  let (range, url) = found.first().context("expected one URL")?;
  let start = text
    .find(original)
    .context("expected URL is missing from fixture")?;
  assert_that!(range, eq(&(start..start + original.len())));
  assert_that!(&text[range.clone()], eq(original));
  assert_that!(url.as_str(), eq(normalized));
  Ok(())
}

#[rstest]
#[case::empty("")]
#[case::plain_unicode("这是没有链接的文本 😊")]
#[case::ascii_email("user@example.com/path")]
#[case::unicode_email("用户@例子.测试/中文")]
#[case::ascii_domain_suffix("badexample.com/path")]
#[case::unconfigured_subdomain("sub.example.com/path")]
#[case::unicode_domain_suffix("坏例子.测试/中文")]
#[case::unconfigured_unicode_domain("其他.测试/中文")]
#[case::unsupported_scheme("ftp://example.com/path")]
#[case::unsupported_unicode_scheme("ftp://例子.测试/中文")]
#[case::malformed_port("https://example.com:bad/path")]
fn avoids_false_matches(rules: Rules, #[case] text: &str) {
  assert_that!(find_urls(text, &rules), is_empty());
}

#[rstest]
#[case::mixed_schemes(
  "开头💡 https://例子.测试/路径?标题=你好，世界。example.com/第二个?x=😊 完",
  vec!["https://例子.测试/路径?标题=你好，世界", "example.com/第二个?x=😊"]
)]
#[case::bare_urls(
  "example.com/第一个，münich.example/第二个",
  vec!["example.com/第一个", "münich.example/第二个"]
)]
#[case::wrapped_and_adjacent(
  "（https://example.com/（测试）?q=中文）。https://例子.测试/目录",
  vec!["https://example.com/（测试）?q=中文", "https://例子.测试/目录"]
)]
fn multiple_urls_preserve_utf8_byte_offsets(
  rules: Rules,
  #[case] text: &str,
  #[case] originals: Vec<&str>,
) -> Result<()> {
  let found = find_urls(text, &rules);
  assert_that!(found.len(), eq(originals.len()));
  let actual: Vec<_> = found
    .iter()
    .map(|(range, _)| (range.clone(), &text[range.clone()]))
    .collect();
  let expected: Vec<_> = originals
    .iter()
    .map(|&original| {
      let start = text
        .find(original)
        .context("expected URL is missing from fixture")?;
      Ok((start..start + original.len(), original))
    })
    .collect::<Result<_>>()?;
  assert_that!(actual, eq(&expected));
  Ok(())
}
