mod rules;
mod urls;

pub use rules::ReplacerConfig;

use std::time::Duration;

use anyhow::{bail, Context, Result};
use frankenstein::reqwest::{redirect, Client, Proxy};
use image::{DynamicImage, Rgba};
use qrcode::QrCode;
use url::Url;

use rules::Rules;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/118.0.0.0 Safari/537.36";
const MAX_DISPATCHES: usize = 8;

pub struct Replacer {
  config: ReplacerConfig,
  rules: Rules,
  client: Client,
}

impl Replacer {
  pub fn new(config: &ReplacerConfig, proxy: Option<&str>) -> Result<Self> {
    let rules = Rules::new(config)?;
    let mut client = Client::builder()
      .user_agent(UA)
      .connect_timeout(Duration::from_secs(10))
      .timeout(Duration::from_secs(30))
      .redirect(redirect::Policy::limited(5));
    if let Some(proxy) = proxy {
      client = client.proxy(Proxy::all(proxy).context("Invalid proxy")?);
    }
    Ok(Self {
      config: config.clone(),
      rules,
      client: client.build()?,
    })
  }

  pub async fn replace_all(&self, text: &str) -> Result<String> {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, url) in urls::find_urls(text, &self.rules) {
      output.push_str(&text[cursor..range.start]);
      match self.replace_url(url).await? {
        Some(replaced) => output.push_str(&replaced),
        None => output.push_str(&text[range.clone()]),
      }
      cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
  }

  async fn replace_url(&self, mut url: Url) -> Result<Option<String>> {
    let mut visited = Vec::new();
    for _ in 0..MAX_DISPATCHES {
      let Some((rule, captures)) = self.rules.dispatch(&self.config, &url) else {
        return Ok((!visited.is_empty()).then(|| url.to_string()));
      };
      if visited.contains(&url) {
        bail!("Redirect dispatch loop at {url}");
      }
      visited.push(url.clone());
      let before = url.clone();
      if rule.resolve_redirect {
        url = self
          .client
          .get(url.clone())
          .send()
          .await
          .with_context(|| format!("Failed to resolve redirect: {url}"))?
          .url()
          .clone();
      }
      let redirected = url != before;
      rule.apply(&mut url, captures)?;
      if !rule.resolve_redirect || !redirected {
        return Ok(Some(url.to_string()));
      }
    }
    bail!("Exceeded {MAX_DISPATCHES} redirect dispatches at {url}")
  }

  pub async fn replace_qrcode(
    &self,
    image: DynamicImage,
  ) -> Result<Option<(DynamicImage, Vec<String>)>> {
    let mut rqrr_detect = rqrr::PreparedImage::prepare(image.to_luma8());
    let grids = rqrr_detect.detect_grids();
    let mut new_img = image;
    let mut urls = Vec::new();
    for grid in grids {
      let (top_left, top_right, bottom_left) = (grid.bounds[0], grid.bounds[1], grid.bounds[3]);
      let width = (top_right.x - top_left.x) as i64;
      let height = (bottom_left.y - top_left.y) as i64;
      let (_meta, content) = grid.decode()?;
      let replaced = self
        .replace_all(&content)
        .await
        .context("Failed to replace QR content")?;
      if replaced == content {
        continue;
      }
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
        i64::from(top_left.x) - 5,
        i64::from(top_left.y) - 5,
      );
    }
    Ok((!urls.is_empty()).then_some((new_img, urls)))
  }
}

#[cfg(test)]
mod tests;
