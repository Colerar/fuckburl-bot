use std::{
  borrow::{Borrow, Cow},
  ops::Sub,
  str::FromStr,
};

use anyhow::{Context, Result};
use fancy_regex::Regex;
use frankenstein::reqwest::{redirect, Client, ClientBuilder, Url};
use image::{DynamicImage, Rgba};
use log::error;
use once_cell::sync::Lazy;
use qrcode::QrCode;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/118.0.0.0 Safari/537.36";

static CLIENT_REDIRECT_ONCE: Lazy<Client> = Lazy::new(|| {
  let once_redirect = redirect::Policy::custom(|attempt| {
    if attempt.previous().len() > 1 {
      attempt.stop()
    } else {
      attempt.follow()
    }
  });
  ClientBuilder::new()
    .user_agent(UA)
    .redirect(once_redirect)
    .build()
    .expect("Unable to build reqwest client")
});

static BSHORT_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
    r"((https?://|(?<![a-zA-Z])|^)?(b23\.tv|bili2233\.cn)/[0-9a-zA-Z]+/?)\??(?:&?[^=&]*=[^=&]*)*",
  )
  .unwrap()
});

static BVIDEO_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(?P<url>(https?://|(?<![a-zA-Z])|^)(www\.)?bilibili\.com/video/[0-9a-zA-Z]+/?)\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});

static YOUTUBE_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(https?://|(?<![a-zA-Z])|^)(www\.)?(youtube\.com|youtu\.be)/(watch|[a-zA-Z_]+)\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});
static BARTICLE_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(https?://|(?<![a-zA-Z])|^)(www\.)?bilibili\.com/read/mobile/(?P<cvid>[0-9]+)\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});
static BOPUS_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(https?://|(?<![a-zA-Z])|^)((www|m)\.)?bilibili\.com/opus/(?P<opus_id>[0-9]+)\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});
static AMAZON_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"(?P<domain>(https?://|(?<![a-zA-Z])|^)(www\.)?amazon\.(com|co(\.[a-zA-Z]+)?)/)[a-zA-Z0-9%-]+/(?P<path>dp/[0-9a-zA-Z]+/?)\??(?:&?[^=&]*=[^=&]*)*"
  ).unwrap()
});
static AMAZON_SEARCH_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"(?P<domain>(https?://|(?<![a-zA-Z])|^)(www\.)?amazon\.(com|co(\.[a-zA-Z]+)?)/s)(?P<keyword>\?k=[a-zA-Z0-9%+-]+)(?:&?[^=&]*=[^=&]*)*"
)
.unwrap()
});
static TWITTER_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"(https?://|(?<![a-zA-Z])|^)(www|c\.)?(vx)?twitter\.com(?P<path>/[a-zA-Z0-9_]+/status/[0-9]+)\??(?:&?[^=&]*=[^=&]*)*"
)
.unwrap()
});

static TWITTER_X_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"(https?://|(?<![a-zA-Z])|^)(www\.)?(fixup)?x\.com(?P<path>/[a-zA-Z0-9_]+/status/[0-9]+)\??(?:&?[^=&]*=[^=&]*)*"
)
.unwrap()
});

static WEIXIN_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(https?://|(?<![a-zA-Z])|^)mp\.weixin\.qq\.com/s\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});
static JD_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"(?P<url>(https?://|(?<![a-zA-Z])|^)item\.(m\.)?jd\.com/product/[0-9]+\.html)\??(?:&?[^=&]*=[^=&]*)*"
)
.unwrap()
});
static TWITTER_SHORT_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"((https?://|(?<![a-zA-Z])|^)t\.co/[0-9a-zA-Z]+/?)\??(?:&?[^=&]*=[^=&]*)*").unwrap()
});
static TIKTOK_SHARE_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(
  r"((https?://|(?<![a-zA-Z])|^)(vm|vt|www)\.tiktok\.com/(t/)?[0-9a-zA-Z]+/?)\??(?:&?[^=&]*=[^=&]*)*"
).unwrap()
});

pub async fn replace_all(text: &str) -> Result<String> {
  let mut new = text.to_string();
  new = replace_bshort(&new)
    .await
    .context("Failed to replace short url")?;
  new = replace_twitter_short(&new)
    .await
    .context("Failed to replace twitter short url")?;
  new = replace_tiktok_share(&new)
    .await
    .context("Failed to replace tiktok share url")?;
  replace_youtube(&mut new);
  replace_btrack(&mut new);
  new = replace_barticle(&new);
  new = replace_bopus(&new);
  new = replace_twitter(&new);
  new = replace_twitter_x(&new);
  new = replace_amazon(&new);
  new = replace_amazon_search(&new);
  new = replace_weixin(&new);
  new = replace_jd(&new);
  Ok(new)
}

pub async fn replace_qrcode(
  image: DynamicImage,
) -> anyhow::Result<Option<(DynamicImage, Vec<String>)>> {
  // Prepare for detection
  let mut rqrr_detect = rqrr::PreparedImage::prepare(image.to_luma8());
  let mut new_img = image;
  let grids = rqrr_detect.detect_grids();
  if grids.is_empty() {
    return Ok(None);
  }
  let mut urls = Vec::new();
  let mut replaced_cnt = 0;
  for grid in grids {
    let (top_left, top_right, _bottom_right, bottom_left) = (
      grid.bounds[0],
      grid.bounds[1],
      grid.bounds[2],
      grid.bounds[3],
    );
    let width = (top_right.x - top_left.x) as i64;
    let height = (bottom_left.y - top_left.y) as i64;
    let (_meta, content) = grid.decode()?;
    let replaced = replace_all(&content)
      .await
      .context("failed to replace all")?;
    if replaced == content {
      continue;
    }

    // encode the content to qrcode, and put it on the image
    let qr = QrCode::with_error_correction_level(&replaced, qrcode::EcLevel::L)
      .context("Failed to encode to qrcode")?;
    let qrimg = qr
      .render::<Rgba<u8>>()
      .light_color(Rgba([255, 255, 255, 255]))
      .dark_color(Rgba([0, 0, 0, 255]))
      .quiet_zone(false)
      .max_dimensions((width as f64 * 1.1) as u32, (height as f64 * 1.1) as u32)
      .min_dimensions((width as f64 * 0.9) as u32, (height as f64 * 0.9) as u32)
      .build();
    urls.push(replaced);

    image::imageops::overlay(
      &mut new_img,
      &qrimg,
      top_left.x.sub(5).into(),
      top_left.y.sub(5).into(),
    );
    replaced_cnt += 1;
  }
  if replaced_cnt == 0 {
    return Ok(None);
  }
  Ok(Some((new_img, urls)))
}

fn replace_twitter(url: &str) -> String {
  TWITTER_REGEX.replace(url, "https://fixupx.com$path").into()
}

fn replace_twitter_x(url: &str) -> String {
  TWITTER_X_REGEX
    .replace(url, "https://fixupx.com$path")
    .into()
}

fn replace_weixin(text: &str) -> String {
  let mut new_str = text.to_string();
  for i in WEIXIN_REGEX.find_iter(text) {
    let i = match i {
      Ok(i) => i,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let mut url = if let Ok(url) = Url::from_str(i.as_str()) {
      url
    } else {
      continue;
    };
    const KEYS: Cow<[&str]> = Cow::Borrowed(&["__biz", "mid", "idx", "sn"]);
    url.keep_pairs_only_in(KEYS);
    new_str.replace_range(i.range(), url.to_string().as_str());
  }
  new_str
}

fn replace_jd(url: &str) -> String {
  JD_REGEX.replace_all(url, "$url").into()
}

fn replace_amazon(url: &str) -> String {
  AMAZON_REGEX.replace_all(url, "$domain$path").into()
}

fn replace_amazon_search(url: &str) -> String {
  AMAZON_SEARCH_REGEX
    .replace_all(url, "$domain$keyword")
    .into()
}

fn trim_youtube_link(url: &mut Url) {
  const KEYS: Cow<[&str]> = Cow::Borrowed(&["v", "list", "index", "t"]);
  url.keep_pairs_only_in(KEYS);
}

fn replace_youtube(text: &mut String) {
  let mut replaces = Vec::new();
  for i in YOUTUBE_REGEX.find_iter(text) {
    let i = match i {
      Ok(i) => i,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let Ok(mut url) = Url::from_str(i.as_str()) else {
      continue;
    };
    trim_youtube_link(&mut url);
    replaces.push((i.range(), url.to_string()));
  }
  for (range, str) in replaces {
    text.replace_range(range, str.as_str());
  }
}

fn trim_bili_link(url: &mut Url) {
  const KEYS: Cow<[&str]> = Cow::Borrowed(&["p", "t"]);
  url.keep_pairs_only_in(KEYS);
}

fn replace_btrack(text: &mut String) {
  let mut replaces = Vec::new();
  for i in BVIDEO_REGEX.find_iter(text) {
    let i = match i {
      Ok(i) => i,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let Ok(mut url) = Url::from_str(i.as_str()) else {
      continue;
    };
    trim_bili_link(&mut url);
    replaces.push((i.range(), url.to_string()));
  }
  for (range, str) in replaces {
    text.replace_range(range, str.as_str());
  }
}

async fn replace_bshort(str: &str) -> Result<String> {
  let mut new_str = str.to_string();
  let matches: Vec<_> = BSHORT_REGEX.find_iter(str).collect();
  for x in matches.iter() {
    let x = match x {
      Ok(x) => x,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let mut url = get_redirect_url(x.as_str()).await?;
    trim_bili_link(&mut url);
    new_str.replace_range(x.range(), url.to_string().as_str());
  }
  Ok(new_str)
}

async fn replace_twitter_short(str: &str) -> Result<String> {
  let mut new_str = str.to_string();
  let matches: Vec<_> = TWITTER_SHORT_REGEX.find_iter(str).collect();
  for x in matches.iter() {
    let x = match x {
      Ok(x) => x,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let url = get_redirect_url(x.as_str()).await?;
    new_str.replace_range(x.range(), url.to_string().as_str());
  }
  Ok(new_str)
}

async fn replace_tiktok_share(str: &str) -> Result<String> {
  let mut new_str = str.to_string();
  let matches: Vec<_> = TIKTOK_SHARE_REGEX.find_iter(str).collect();
  for x in matches.iter() {
    let x = match x {
      Ok(x) => x,
      Err(err) => {
        error!("Failed to find_iter: {err}");
        continue;
      },
    };
    let mut url = get_redirect_url(x.as_str()).await?;
    url.set_query(None);
    new_str.replace_range(x.range(), url.to_string().as_str());
  }
  Ok(new_str)
}

fn replace_barticle(str: &str) -> String {
  BARTICLE_REGEX
    .replace_all(str, "https://www.bilibili.com/read/cv$cvid")
    .into()
}

fn replace_bopus(str: &str) -> String {
  BOPUS_REGEX
    .replace_all(str, "https://t.bilibili.com/$opus_id")
    .into()
}

async fn get_redirect_url(url: &str) -> Result<Url> {
  let resp = CLIENT_REDIRECT_ONCE
    .get(url)
    .send()
    .await
    .with_context(|| format!("Failed to get url {url}"))?;
  Ok(resp.url().clone())
}

trait RemovePairsIf {
  fn remove_pairs_if_key<P>(&mut self, predicate: P)
  where
    Self: Sized,
    P: Fn(&str) -> bool;

  #[inline]
  fn keep_pairs_only_in(&mut self, vec: Cow<[&str]>)
  where
    Self: Sized,
  {
    self.remove_pairs_if_key(|k| !vec.contains(&k));
  }
}

impl RemovePairsIf for Url {
  #[inline]
  fn remove_pairs_if_key<P>(&mut self, predicate: P)
  where
    Self: Sized,
    P: Fn(&str) -> bool,
  {
    let buf = String::new();
    let mut ser = form_urlencoded::Serializer::new(buf);
    self.query_pairs().into_iter().for_each(|(k, v)| {
      if !predicate(k.borrow()) {
        ser.append_pair(k.borrow(), v.borrow());
      }
    });

    self.set_query(match &*ser.finish() {
      "" => None,
      query => Some(query),
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn remove_all() {
    let mut text = "https://www.bilibili.com/video/BV1Hg411T7fT/?spm_id_from=333.788.recommend_more_video.1&vd_source=425ad7d352481d80617a03327da07da0".to_string();
    replace_btrack(&mut text);
    assert_eq!("https://www.bilibili.com/video/BV1Hg411T7fT/", text);
  }

  #[test]
  fn keep_certain_params() {
    {
      let mut text =
        "https://www.bilibili.com/video/BV114514/?t=123&p=1&spm=1.2212.22321".to_string();
      replace_btrack(&mut text);
      assert_eq!("https://www.bilibili.com/video/BV114514/?t=123&p=1", text);
    }
    {
      let mut text = "https://www.bilibili.com/video/BV114514/?t=123&spm=1.2212.22321".to_string();
      replace_btrack(&mut text);
      assert_eq!("https://www.bilibili.com/video/BV114514/?t=123", text);
    }
  }

  #[tokio::test]
  async fn bshort() {
    let text = "https://b23.tv/lBI8Ov3".to_string();
    let result = replace_bshort(&text).await.unwrap();
    assert_eq!("https://www.bilibili.com/video/BV1se4y177g9/?t=100", result);
  }

  #[test]
  fn amazon() {
    assert_eq!(
      "https://www.amazon.com/dp/B00NLZUM36/",
      replace_amazon("https://www.amazon.com/Redragon-S101-Keyboard-Ergonomic-Programmable/dp/B00NLZUM36/ref=sr_1_1?keywords=gaming+keyboard&pd_rd_r=89c237af-e7f2-4af6-b9c4&pd_rd_w=0aaaD&pd_rd_wg=KZWal&pf_rd_p=112312321&pf_rd_r=1233&qid=234231231&qu=eyJxc2MiOinFzcCI6IjYuMjAifQ%3D%3D&sr=8-1"),
    );
    assert_eq!(
      "https://www.amazon.co.jp/dp/B00NLZUM36/",
      replace_amazon("https://www.amazon.co.jp/Redragon-S101-Keyboard-Ergonomic-Programmable/dp/B00NLZUM36/ref=sr_1_1?keywords=gaming+keyboard&pd_rd_r=89c237af-e7f2-4af6-b9c4&pd_rd_w=0aaaD&pd_rd_wg=KZWal&pf_rd_p=112312321&pf_rd_r=1233&qid=234231231&qu=eyJxc2MiOinFzcCI6IjYuMjAifQ%3D%3D&sr=8-1"),
    );
  }

  #[test]
  fn amazon_search() {
    assert_eq!(
      "https://www.amazon.com/s?k=%E4%BD%A0%E5%A5%BD%26+%2B",
      replace_amazon_search("https://www.amazon.com/s?k=%E4%BD%A0%E5%A5%BD%26+%2B&crid=1SHSKHE0RZCED&sprefix=%E4%BD%A0%E5%A5%BD%26+%2B%2Caps%2C1307&ref=nb_sb_noss_2")
    )
  }

  #[test]
  fn replace_barticle_test() {
    assert_eq!(
      "https://www.bilibili.com/read/cv19172625",
      replace_barticle("https://www.bilibili.com/read/mobile/19172625?xxx=114514&asdfasdf=32394239ADSAD-12312aASDASD")
    )
  }

  #[test]
  fn replace_twitter_test() {
    assert_eq!(
      "https://c.vxtwitter.com/Penny_0571/status/1587323246506528769",
      replace_twitter(
        "https://fixupx.com/Penny_0571/status/1587323246506528769?s=20&t=0Mzx3uLKTD-kygDQmaXvFq"
      )
    )
  }

  #[test]
  fn replace_weixin_test() {
    let text = "https://mp.weixin.qq.com/s?__biz=MzIzzMwNjc1NzU==&mid=2650309&idx=114514&sn=2fd9d2a3b0b544a6da&chksm=e8de3b77dfa9b2612b676b21f34a75a79994bfcd4a4#rd";
    assert_eq!(
      "https://mp.weixin.qq.com/s?__biz=MzIzzMwNjc1NzU%3D%3D&mid=2650309&idx=114514&sn=2fd9d2a3b0b544a6da#rd",
      replace_weixin(
        text
      )
    )
  }

  #[test]
  fn replace_jd_test() {
    assert_eq!(
      "https://item.m.jd.com/product/100026923531.html",
      replace_jd("https://item.m.jd.com/product/100026923531.html?&utm_source=iosapp&utm_medium=appshare&utm_campaign=114514&utm_term=CopyURL&ad_od=share&gx=T2nEPztRx6NTRa30RpDCM")
    )
  }

  #[tokio::test]
  async fn replace_twitter_short_test() {
    let text = "https://t.co/jqpeEFD8Nz".to_string();
    let result = replace_twitter_short(&text).await.unwrap();
    assert_eq!("https://sdl.moe/", result)
  }

  #[tokio::test]
  async fn replace_tiktok_share_test() {
    let text_1 = "https://www.tiktok.com/t/ZSLLFK1V4/?t=1".to_string();
    let result_1 = replace_tiktok_share(&text_1).await.unwrap();
    assert_eq!(
      "https://www.tiktok.com/@omi_kim/video/7145033030191549697",
      result_1
    );

    let text_2 = "https://vt.tiktok.com/ZSLd5tSKG/".to_string();
    let result_2 = replace_tiktok_share(&text_2).await.unwrap();

    assert_eq!(
      "https://www.tiktok.com/@zaki_tuber/video/7234942299489291522",
      result_2
    );

    let text_3 = "https://vm.tiktok.com/ZSeNPcNM2/".to_string();
    let result_3 = replace_tiktok_share(&text_3).await.unwrap();

    assert_eq!(
      "https://www.tiktok.com/@kabyi_lame/video/7013423699755896070",
      result_3
    );
  }
}
