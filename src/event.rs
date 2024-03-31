use std::{fmt::Display, sync::Arc};

use anyhow::{Context, Ok, Result};
use frankenstein::{
  AsyncApi, AsyncTelegramApi, DeleteMessageParams, MessageOrigin, ParseMode, ReplyParameters,
  SendMessageParams, Update, UpdateContent, User,
};
use log::{debug, info};

use crate::{replacer::replace_all, start_time, Config};
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
  api: &AsyncApi,
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

  let text = if let Some(text) = msg.text.clone() {
    text
  } else {
    return Ok(());
  };

  let replaced = replace_all(&text).await.context("Failed to replace text")?;
  if replaced == text {
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

  text.push_str(&v_htmlescape::escape(&replaced).to_string());

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

  let mut send_msg = SendMessageParams::builder()
    .chat_id(msg.chat.id)
    .text(text)
    .parse_mode(ParseMode::Html)
    .build();

  send_msg.reply_parameters = msg
    .reply_to_message
    .map(|i| ReplyParameters::builder().message_id(i.message_id).build());

  let resp = api
    .send_message(&send_msg)
    .await
    .context("Failed to send message...")?;
  debug!("{resp:?}");

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
    };
    f.write_str(str)
  }
}
