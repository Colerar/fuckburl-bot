use std::{fmt::Display, sync::Arc};

use anyhow::{Context, Ok, Result};
use frankenstein::{
  client_reqwest::Bot, input_file::{FileUpload, InputFile}, methods::{DeleteMessageParams, GetFileParams, SendMessageParams, SendPhotoParams}, types::{MessageOrigin, PhotoSize, ReplyParameters, User}, updates::{Update, UpdateContent}, AsyncTelegramApi, ParseMode
};
use log::{debug, info};

use crate::{
  replacer::{replace_all, replace_qrcode},
  start_time, Config,
};
use std::fmt::Write;

fn write_user(text: &mut String, user: &User) {
  match user.username {
    Some(ref at) => {
      write!(text, "@{at}").unwrap();
    },
    None => {
      write!(text, r#"<a href="tg://user?id={}">"#, user.id).unwrap();
      text.push_str(&v_htmlescape::escape(&user.first_name).to_string());
      if let Some(ref last) = user.last_name {
        write!(text, " {}", v_htmlescape::escape(last)).unwrap();
      }
      text.push_str("</a>");
    },
  }
}

pub(crate) async fn process_update(
  api: &Bot,
  config: Arc<Config>,
  update: Update,
) -> Result<()> {
  let UpdateContent::Message(msg) = update.content else {
    info!("Unsupported message type: {}", MessageType(update.content));
    return Ok(());
  };

  if msg.date < start_time() {
    return Ok(());
  }
  let contains_id = config.enabled_chats.contains(&msg.chat.id.to_string());
  let contains_username = msg
    .chat
    .username
    .is_some_and(|usr| config.enabled_chats.contains(&usr));
  if !contains_id && !contains_username {
    return Ok(());
  };

  debug!("Message id: {}/{}", msg.chat.id, msg.message_id);

  let mut replaced_image: Option<(tempfile::NamedTempFile, Vec<String>)> = None;

  if let Some(photos) = msg.photo {
    let photo_meta = photos.iter().max_by(|a, b| a.file_size.cmp(&b.file_size));
    if let Some(photo_meta) = photo_meta {
      replaced_image = try_replace_photo(api, config.clone(), photo_meta).await?;
    }
  }

  let changed_text = if let Some(text) = msg.text.clone() {
    if text.contains("@ignoreme") {
      return Ok(());
    }
    let replaced = replace_all(&text).await.context("Failed to replace text")?;
    if replaced != text {
      Some(replaced)
    } else {
      None
    }
  } else {
    None
  };

  if changed_text.is_none() && replaced_image.is_none() {
    return Ok(());
  }

  info!("Replacing message {}/{}", msg.chat.id, msg.message_id);

  let mut text = String::with_capacity(128);
  write!(text, "Send by ").unwrap();
  match msg.from {
    Some(user) => write_user(&mut text, &user),
    None => {
      write!(text, "Unknown").unwrap();
    },
  }

  writeln!(text, ":\n").unwrap();

  if let Some(changed_text) = changed_text {
    text.push_str(&v_htmlescape::escape(&changed_text).to_string());
  }

  if let Some((_, ref urls)) = replaced_image {
    if !urls.is_empty() {
      text.push_str("\n<i>URLs:</i>");
    }
    for url in urls {
      text.push_str(&format!(
        "\n<a href=\"{url}\">{}</a>",
        v_htmlescape::escape(&url)
      ));
    }
  }

  if let Some(reply_origin) = msg.forward_origin {
    use MessageOrigin as MO;
    match *reply_origin {
      MO::User(user) => {
        text.push_str("\n\n<i>forwarded from ");
        write_user(&mut text, &user.sender_user);
        text.push_str("</i>");
      },
      MO::HiddenUser(user) => {
        text.push_str("\n\n<i>forwarded from ");
        text.push_str(&v_htmlescape::escape(&user.sender_user_name).to_string());
        text.push_str("</i>");
      },
      MO::Chat(_chat) => {
        return Ok(());
      },
      MO::Channel(_channel) => {
        return Ok(());
      },
    }
  }

  let reply_parameters = msg
    .reply_to_message
    .map(|i| ReplyParameters::builder().message_id(i.message_id).build());

  match replaced_image {
    Some((img, _urls)) => {
      let mut send_msg = SendPhotoParams::builder()
        .chat_id(msg.chat.id)
        .photo(FileUpload::InputFile(InputFile {
          path: img.path().to_path_buf(),
        }))
        .caption(text)
        .parse_mode(ParseMode::Html)
        .build();
      send_msg.reply_parameters = reply_parameters;
      let resp = api
        .send_photo(&send_msg)
        .await
        .context("Failed to send photo...")?;
      debug!("{resp:?}");
    },
    None => {
      let mut send_msg = SendMessageParams::builder()
        .chat_id(msg.chat.id)
        .text(text)
        .parse_mode(ParseMode::Html)
        .build();

      send_msg.reply_parameters = reply_parameters;

      let resp = api
        .send_message(&send_msg)
        .await
        .context("Failed to send message...")?;
      debug!("{resp:?}");
    },
  }
  let resp = api
    .delete_message(
      &DeleteMessageParams::builder()
        .chat_id(msg.chat.id)
        .message_id(msg.message_id)
        .build(),
    )
    .await
    .context("Failed to delete message...")?;
  debug!("{resp:?}",);

  Ok(())
}

async fn try_replace_photo(
  api: &Bot,
  config: Arc<Config>,
  meta: &PhotoSize,
) -> Result<Option<(tempfile::NamedTempFile, Vec<String>)>> {
  if meta.file_size.unwrap_or(u64::MAX) > 1024 * 1024 * 1 {
    return Ok(None);
  }
  let resp = api
    .get_file(
      &GetFileParams::builder()
        .file_id(meta.file_id.clone())
        .build(),
    )
    .await
    .context("Failed to get file")?;
  let file_path = resp.result.file_path.context("File path is not found")?;
  let file_url = format!(
    "https://api.telegram.org/file/bot{}/{}",
    config.telegram_token, file_path
  );
  let file_data = api
    .client
    .get(file_url)
    .send()
    .await
    .context("Failed to get file")?;
  let file_bytes = file_data.bytes().await.context("Failed to read file")?;
  let image = image::load_from_memory(&file_bytes).context("Failed to load image")?;
  let replaced = replace_qrcode(image)
    .await
    .context("Failed to replace qrcode")?;
  let Some((replaced, urls)) = replaced else {
    return Ok(None);
  };
  let file = tempfile::NamedTempFile::new()?;
  replaced.save_with_format(file.path(), image::ImageFormat::Png)?;
  Ok(Some((file, urls)))
}

struct MessageType(UpdateContent);

impl Display for MessageType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let str = match self.0 {
      UpdateContent::Message(_) => "Message",
      UpdateContent::EditedMessage(_) => "EditedMessage",
      UpdateContent::ChannelPost(_) => "ChannelPost",
      UpdateContent::EditedChannelPost(_) => "EditedChannelPost",
      UpdateContent::InlineQuery(_) => "InlineQuery",
      UpdateContent::ChosenInlineResult(_) => "ChosenInlineResult",
      UpdateContent::CallbackQuery(_) => "CallbackQuery",
      UpdateContent::ShippingQuery(_) => "ShippingQuery",
      UpdateContent::PreCheckoutQuery(_) => "PreCheckoutQuery",
      UpdateContent::Poll(_) => "Poll",
      UpdateContent::PollAnswer(_) => "PollAnswer",
      UpdateContent::MyChatMember(_) => "MyChatMember",
      UpdateContent::ChatMember(_) => "ChatMember",
      UpdateContent::ChatJoinRequest(_) => "ChatJoinRequest",
      UpdateContent::MessageReaction(_) => "MessageReaction",
      UpdateContent::MessageReactionCount(_) => "MessageReactionCount",
      UpdateContent::ChatBoost(_) => "ChatBoost",
      UpdateContent::RemovedChatBoost(_) => "RemovedChatBoost",
      UpdateContent::BusinessConnection(_) => "BusinessConnection",
      UpdateContent::BusinessMessage(_) => "BusinessMessage",
      UpdateContent::EditedBusinessMessage(_) => "EditedBusinessMessage",
      UpdateContent::DeletedBusinessMessages(_) => "DeletedBusinessMessages",
      UpdateContent::PurchasedPaidMedia(_) => "PurchasedPaidMedia",
    };
    f.write_str(str)
  }
}
