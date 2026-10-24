use std::{
  io::{Read, Write},
  net::TcpListener,
  thread,
};

use googletest::{
  assert_that,
  matchers::{contains_substring, eq, err, gt, is_empty, lt, not, starts_with},
};
use rstest::{fixture, rstest};

use super::*;

fn configured(source: &str) -> Result<Replacer> {
  Replacer::new(&toml::from_str(source)?, None)
}

#[fixture]
fn default_replacer() -> Replacer {
  Replacer::new(&ReplacerConfig::default(), None).expect("build bundled replacer fixture")
}

#[rstest]
#[case::bilibili_parameters(
  "https://www.bilibili.com/video/BV114514/?t=123&p=1&spm=tracking",
  "https://www.bilibili.com/video/BV114514/?t=123&p=1"
)]
#[case::bilibili_tracking_only(
  "https://www.bilibili.com/video/BV114514/?spm=tracking",
  "https://www.bilibili.com/video/BV114514/"
)]
#[case::bilibili_article(
  "https://www.bilibili.com/read/mobile/19172625?share=1",
  "https://www.bilibili.com/read/cv19172625"
)]
#[case::bilibili_opus(
  "http://m.bilibili.com/opus/12345?share=1",
  "https://t.bilibili.com/12345"
)]
#[case::youtube_watch(
  "https://www.youtube.com/watch?si=tracking&v=abc&t=123&list=xyz&index=2",
  "https://www.youtube.com/watch?v=abc&t=123&list=xyz&index=2"
)]
#[case::youtube_short_link("https://youtu.be/abc?si=tracking&t=123", "https://youtu.be/abc?t=123")]
#[case::youtube_shorts(
  "https://www.youtube.com/shorts/abc?si=tracking",
  "https://www.youtube.com/shorts/abc"
)]
#[case::twitter(
  "http://twitter.com/Penny_0571/status/1587323246506528769?s=20",
  "https://fixupx.com/Penny_0571/status/1587323246506528769"
)]
#[case::vxtwitter(
  "https://c.vxtwitter.com/user/status/123?t=tracking",
  "https://fixupx.com/user/status/123"
)]
#[case::x(
  "https://x.com/user/status/123?s=20",
  "https://fixupx.com/user/status/123"
)]
#[case::fixupx(
  "https://fixupx.com/user/status/123?s=20",
  "https://fixupx.com/user/status/123"
)]
#[case::amazon_product(
  "https://www.amazon.com/Redragon-S101/dp/B00NLZUM36/ref=sr_1_1?keywords=keyboard",
  "https://www.amazon.com/dp/B00NLZUM36/"
)]
#[case::amazon_japan(
  "https://www.amazon.co.jp/Redragon-S101/dp/B00NLZUM36/ref=sr_1_1?tracking=1",
  "https://www.amazon.co.jp/dp/B00NLZUM36/"
)]
#[case::amazon_direct_product(
  "https://www.amazon.com/dp/B00NLZUM36?tracking=1",
  "https://www.amazon.com/dp/B00NLZUM36/"
)]
#[case::amazon_search(
  "https://www.amazon.com/s?ref=tracking&k=%E4%BD%A0%E5%A5%BD%26+%2B",
  "https://www.amazon.com/s?k=%E4%BD%A0%E5%A5%BD%26+%2B"
)]
#[case::weixin("https://mp.weixin.qq.com/s?__biz=MzIzzMwNjc1NzU==&mid=2650309&idx=114514&sn=abc&chksm=tracking#rd", "https://mp.weixin.qq.com/s?__biz=MzIzzMwNjc1NzU%3D%3D&mid=2650309&idx=114514&sn=abc#rd")]
#[case::jd(
  "https://item.m.jd.com/product/100026923531.html?&utm_source=iosapp",
  "https://item.m.jd.com/product/100026923531.html"
)]
#[tokio::test]
async fn default_rules_cover_existing_sites(
  default_replacer: Replacer,
  #[case] input: &str,
  #[case] expected: &str,
) -> Result<()> {
  assert_that!(default_replacer.replace_all(input).await?, eq(expected));
  assert_that!(default_replacer.replace_all(expected).await?, eq(expected));
  Ok(())
}

#[tokio::test]
async fn custom_configuration_controls_dispatch_and_priority() -> Result<()> {
  let replacer = configured(
    r#"
    [[rules]]
    hosts = ["EXAMPLE.com"]
    path = "/post/{id}"
    rewrite-host = "clean.example.org"
    rewrite-scheme = "https"
    rewrite-path = "/article/item-{id}"
    keep-query = ["page"]

    [[rules]]
    hosts = ["example.com"]
    keep-query = []
  "#,
  )?;
  assert_that!(
    replacer
      .replace_all("http://example.com/post/a%2Fb?page=2&ref=1")
      .await?,
    eq("https://clean.example.org/article/item-a%2Fb?page=2")
  );
  assert_that!(
    replacer
      .replace_all("https://example.com/other?ref=1")
      .await?,
    eq("https://example.com/other")
  );
  let defaults = default_replacer();
  let custom_url = "https://example.com/post/1?ref=1";
  assert_that!(defaults.replace_all(custom_url).await?, eq(custom_url));
  let builtin_url = "https://x.com/user/status/123?s=20";
  assert_that!(replacer.replace_all(builtin_url).await?, eq(builtin_url));
  Ok(())
}

#[rstest]
#[case::shorter_urls("前文 [视频](https://www.bilibili.com/video/BV1/?share=long-value)；\nhttps://youtu.be/abc?si=tracking&t=2, 推文（https://x.com/user/status/123?s=20）。https://youtu.be/def?si=tracking!", "前文 [视频](https://www.bilibili.com/video/BV1/)；\nhttps://youtu.be/abc?t=2, 推文（https://fixupx.com/user/status/123）。https://youtu.be/def!")]
#[case::longer_urls(
  "x.com/a/status/1?s=1 x.com/b/status/2?s=2",
  "https://fixupx.com/a/status/1 https://fixupx.com/b/status/2"
)]
#[tokio::test]
async fn multiple_urls_keep_original_offsets_and_surrounding_text(
  default_replacer: Replacer,
  #[case] input: &str,
  #[case] expected: &str,
) -> Result<()> {
  assert_that!(default_replacer.replace_all(input).await?, eq(expected));
  Ok(())
}

#[rstest]
#[case::bare_domain(
  "看WWW.YouTube.COM/watch?v=abc&si=tracking。",
  "看https://www.youtube.com/watch?v=abc。"
)]
#[case::label_prefix("link:https://youtu.be/abc?si=tracking", "link:https://youtu.be/abc")]
#[tokio::test]
async fn bare_urls_and_label_prefixes(
  default_replacer: Replacer,
  #[case] input: &str,
  #[case] expected: &str,
) -> Result<()> {
  assert_that!(default_replacer.replace_all(input).await?, eq(expected));
  Ok(())
}

#[rstest]
#[case::raw_unicode(
  "开头：https://example.com/【你好，世界】?标题=中文&tracking=追踪。",
  "开头：https://example.com/%E3%80%90%E4%BD%A0%E5%A5%BD%EF%BC%8C%E4%B8%96%E7%95%8C%E3%80%91?%E6%A0%87%E9%A2%98=%E4%B8%AD%E6%96%87。"
)]
#[case::bare_unicode_domain(
  "例子.测试/你好?q=😊&tracking=1",
  "https://xn--fsqu00a.xn--0zwm56d/%E4%BD%A0%E5%A5%BD?q=%F0%9F%98%8A"
)]
#[case::bare_latin_idn(
  "münich.example/straße?q=café&tracking=1",
  "https://xn--mnich-kva.example/stra%C3%9Fe?q=caf%C3%A9"
)]
#[case::mixed_encoding(
  "example.com/%E4%B8%AD文?q=%F0%9F%98%8A你好&tracking=1#章节",
  "https://example.com/%E4%B8%AD%E6%96%87?q=%F0%9F%98%8A%E4%BD%A0%E5%A5%BD#%E7%AB%A0%E8%8A%82"
)]
#[case::multiple_urls(
  "看看 example.com/你好?标题=中文&share=1 https://例子.测试/😀?q=好&tracking=1",
  "看看 https://example.com/%E4%BD%A0%E5%A5%BD?%E6%A0%87%E9%A2%98=%E4%B8%AD%E6%96%87 https://xn--fsqu00a.xn--0zwm56d/%F0%9F%98%80?q=%E5%A5%BD"
)]
#[case::unmatched_unicode_url(
  "https://unknown.example/你好?q=😊&tracking=1",
  "https://unknown.example/你好?q=😊&tracking=1"
)]
#[tokio::test]
async fn unicode_and_bare_urls_are_dispatched_and_cleaned(
  #[case] input: &str,
  #[case] expected: &str,
) -> Result<()> {
  let replacer = configured(
    r#"
    [[rules]]
    hosts = ["example.com", "例子.测试", "münich.example"]
    keep-query = ["q", "标题"]
  "#,
  )?;
  assert_that!(replacer.replace_all(input).await?, eq(expected));
  Ok(())
}

#[rstest]
#[case::embedded_domain("https://evil.example/youtube.com/watch?v=1&si=2")]
#[case::domain_suffix("https://youtube.com.evil.example/watch?v=1&si=2")]
#[case::email("user@youtube.com/watch?v=1&si=2")]
#[case::unlisted_subdomain("sub.youtube.com/watch?v=1&si=2")]
#[case::unsupported_scheme("ftp://youtube.com/watch?v=1&si=2")]
#[case::unmatched_path("https://x.com/user?tracking=1")]
#[tokio::test]
async fn unmatched_urls_are_preserved(
  default_replacer: Replacer,
  #[case] input: &str,
) -> Result<()> {
  assert_that!(default_replacer.replace_all(input).await?, eq(input));
  Ok(())
}

#[rstest]
#[case::several_segments(
  "https://example.com/a/b/item/123/tail/?x=1&drop=2&x=3#fragment",
  "https://example.com/123?x=1&x=3#fragment"
)]
#[case::zero_segments("https://example.com/item/123/tail?x=1", "https://example.com/123?x=1")]
#[case::empty_capture(
  "https://example.com/item//tail?x=1&drop=2",
  "https://example.com/item//tail?x=1&drop=2"
)]
#[tokio::test]
async fn path_wildcards_and_repeated_parameters(
  #[case] input: &str,
  #[case] expected: &str,
) -> Result<()> {
  let replacer = configured(
    r#"
    [[rules]]
    hosts = ["example.com"]
    path = "/**/item/{id}/*"
    rewrite-path = "/{id}"
    keep-query = ["x"]
  "#,
  )?;
  assert_that!(replacer.replace_all(input).await?, eq(expected));
  Ok(())
}

#[rstest]
#[case::empty_hosts("[[rules]]\nhosts = []", "hosts must not be empty")]
#[case::invalid_host("[[rules]]\nhosts = ['https://example.com']", "Invalid host")]
#[case::invalid_scheme(
  "[[rules]]\nhosts = ['example.com']\nrewrite-scheme = 'ftp'",
  "rewrite-scheme must be http or https"
)]
#[case::relative_path(
  "[[rules]]\nhosts = ['example.com']\npath = 'missing/slash'",
  "path must start with /"
)]
#[case::duplicate_capture(
  "[[rules]]\nhosts = ['example.com']\npath = '/{id}/{id}'",
  "Duplicate capture: id"
)]
#[case::unknown_capture(
  "[[rules]]\nhosts = ['example.com']\nrewrite-path = '/{missing}'",
  "Unknown rewrite-path capture: missing"
)]
#[case::unclosed_capture(
  "[[rules]]\nhosts = ['example.com']\npath = '/{id}'\nrewrite-path = '/{id'",
  "Unclosed rewrite-path capture"
)]
#[case::unmatched_brace(
  "[[rules]]\nhosts = ['example.com']\nrewrite-path = '/oops}'",
  "Unmatched brace in rewrite-path"
)]
#[case::query_in_path(
  "[[rules]]\nhosts = ['example.com']\nrewrite-path = '/path?query=1'",
  "rewrite-path cannot contain a query or fragment"
)]
#[case::unknown_field(
  "[[rules]]\nhosts = ['example.com']\nkeep-qurey = []",
  "unknown field `keep-qurey`"
)]
fn invalid_configuration_fails_for_the_expected_reason(
  #[case] source: &str,
  #[case] expected_error: &str,
) {
  let result = configured(source)
    .map(|_| ())
    .map_err(|error| format!("{error:#}"));
  assert_that!(result, err(contains_substring(expected_error)));
}

#[tokio::test]
async fn standalone_configuration_and_explicit_empty_rules() -> Result<()> {
  let config: ReplacerConfig = toml::from_str(include_str!("../replacer.example.toml"))?;
  assert_that!(config.rules, not(is_empty()));
  Replacer::new(&config, None)?;
  let replacer = configured("rules = []")?;
  let text = "https://youtu.be/abc?si=tracking";
  assert_that!(replacer.replace_all(text).await?, eq(text));
  Ok(())
}

// Local redirects are integration tests; matching assertions panic immediately
// so a failure in the server thread is also propagated through join().
fn http_fixture(
  responses: Vec<(&'static str, &'static str)>,
) -> Result<(String, thread::JoinHandle<Result<()>>)> {
  let listener = TcpListener::bind("127.0.0.1:0")?;
  listener.set_nonblocking(true)?;
  let address = listener.local_addr()?;
  let server = thread::spawn(move || -> Result<()> {
    let start = std::time::Instant::now();
    for (expected, response) in responses {
      let (mut stream, _) = loop {
        assert_that!(start.elapsed(), lt(Duration::from_secs(5)));
        match listener.accept() {
          Ok(connection) => break connection,
          Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            thread::sleep(Duration::from_millis(10))
          },
          Err(error) => return Err(error.into()),
        }
      };
      stream.set_read_timeout(Some(Duration::from_secs(2)))?;
      let mut request = Vec::new();
      while !request.ends_with(b"\r\n\r\n") {
        let mut buffer = [0; 4096];
        let length = stream.read(&mut buffer)?;
        assert_that!(length, gt(0));
        assert_that!(request.len(), lt(16384));
        request.extend_from_slice(&buffer[..length]);
      }
      assert_that!(std::str::from_utf8(&request)?, starts_with(expected));
      stream.write_all(response.as_bytes())?;
    }
    Ok(())
  });
  Ok((format!("http://{address}"), server))
}

fn local_client() -> Result<Client> {
  Ok(
    Client::builder()
      .no_proxy()
      .timeout(Duration::from_secs(2))
      .build()?,
  )
}

const HTTP_OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[tokio::test]
async fn redirects_are_redispatched_using_destination_rules() -> Result<()> {
  let (base, server) = http_fixture(vec![
    ("GET /short ", "HTTP/1.1 302 Found\r\nLocation: /watch?v=abc&tracking=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    ("GET /watch?v=abc&tracking=1 ", HTTP_OK),
  ])?;
  let mut replacer = configured(
    r#"
    [[rules]]
    hosts = ["127.0.0.1"]
    path = "/short"
    resolve-redirect = true

    [[rules]]
    hosts = ["127.0.0.1"]
    path = "/watch"
    keep-query = ["v"]
  "#,
  )?;
  replacer.client = local_client()?;
  let result = replacer.replace_all(&format!("{base}/short")).await;
  server.join().expect("HTTP fixture thread panicked")?;
  assert_that!(result?, eq(format!("{base}/watch?v=abc").as_str()));
  Ok(())
}

#[tokio::test]
async fn redirects_to_unmatched_paths_still_expand_and_keep_their_query() -> Result<()> {
  let (base, server) = http_fixture(vec![
    ("GET /short ", "HTTP/1.1 302 Found\r\nLocation: /landing?key=value\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    ("GET /landing?key=value ", HTTP_OK),
  ])?;
  let mut replacer =
    configured("[[rules]]\nhosts = ['127.0.0.1']\npath = '/short'\nresolve-redirect = true")?;
  replacer.client = local_client()?;
  let result = replacer.replace_all(&format!("{base}/short")).await;
  server.join().expect("HTTP fixture thread panicked")?;
  assert_that!(result?, eq(format!("{base}/landing?key=value").as_str()));
  Ok(())
}

#[tokio::test]
async fn unchanged_redirect_response_is_not_fetched_again_after_query_filtering() -> Result<()> {
  let (base, server) = http_fixture(vec![("GET /short?tracking=1 ", HTTP_OK)])?;
  let mut replacer = configured(
    "[[rules]]\nhosts = ['127.0.0.1']\npath = '/short'\nresolve-redirect = true\nkeep-query = []",
  )?;
  replacer.client = local_client()?;
  let result = replacer
    .replace_all(&format!("{base}/short?tracking=1"))
    .await;
  server.join().expect("HTTP fixture thread panicked")?;
  assert_that!(result?, eq(format!("{base}/short").as_str()));
  Ok(())
}

#[rstest]
#[case::matching("https://example.com/item?tracking=1", Some(vec!["https://example.com/item".to_string()]))]
#[case::unicode(
  "https://example.com/中文/😺?tracking=1",
  Some(vec!["https://example.com/%E4%B8%AD%E6%96%87/%F0%9F%98%BA".to_string()])
)]
#[case::without_scheme("example.com/item?tracking=1", Some(vec!["https://example.com/item".to_string()]))]
#[case::unmatched("https://other.example/item?tracking=1", None)]
#[case::plain_text("普通文本", None)]
#[tokio::test]
async fn qr_codes_use_the_configured_dispatcher(
  #[case] input: &str,
  #[case] expected: Option<Vec<String>>,
) -> Result<()> {
  let replacer = configured("[[rules]]\nhosts = ['example.com']\nkeep-query = []")?;
  let qr = QrCode::new(input)?;
  let image = DynamicImage::ImageLuma8(
    qr.render::<image::Luma<u8>>()
      .min_dimensions(400, 400)
      .build(),
  );
  let result = replacer.replace_qrcode(image).await?;
  assert_that!(result.map(|(_, urls)| urls), eq(&expected));
  Ok(())
}
