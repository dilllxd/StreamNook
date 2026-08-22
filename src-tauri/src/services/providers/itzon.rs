//! Native itzon chat adapter.
//!
//! itzon exposes the same anonymous IRC-over-WebSocket endpoint used by its web
//! player. The socket address is discovered from `/api/live/ws-config` on each
//! reconnect so an edge move does not require an app update. Incoming IRCv3
//! messages are normalized onto StreamNook's shared provider chat bus.

use super::{
    dec_bridge_users, inc_bridge_users, key, publish_chat_message, publish_frame, ChatProvider,
    SendCapability, SendOutcome,
};
use crate::models::chat_layout::{
    Badge, ChatMessage, LayoutResult, MessageMetadata, MessageSegment, ReplyInfo,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue},
        Message,
    },
};
use url::Url;
use uuid::Uuid;

const WS_CONFIG_URL: &str = "https://itzon.tv/api/live/ws-config";
const CONFIG_TIMEOUT_SECS: u64 = 15;
const CONNECT_TIMEOUT_SECS: u64 = 20;
const READ_TIMEOUT_SECS: u64 = 120;
const MAX_BACKOFF_SECS: u64 = 15;
const RECENT_MESSAGE_LIMIT: usize = 250;
const SEND_CONNECTING: u8 = 0;
const SEND_READY: u8 = 1;
const SEND_UNVERIFIED: u8 = 2;
const SEND_BANNED: u8 = 3;

static FALLBACK_SEQ: AtomicU64 = AtomicU64::new(0);

struct Connection {
    consumers: HashSet<String>,
    task: JoinHandle<()>,
    outgoing: mpsc::Sender<Outgoing>,
    send_state: Arc<AtomicU8>,
}

struct Outgoing {
    text: String,
    reply_to: Option<String>,
    response: oneshot::Sender<SendOutcome>,
}

struct PendingSend {
    text: String,
    reply_to: Option<String>,
    response: oneshot::Sender<SendOutcome>,
    deadline: Instant,
}

pub struct ItzonProvider {
    conns: Mutex<HashMap<String, Connection>>,
    http: reqwest::Client,
}

impl ItzonProvider {
    pub fn new() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ChatProvider for ItzonProvider {
    fn id(&self) -> &'static str {
        "itzon"
    }

    async fn connect(&self, channel: &str, window: &str) -> Result<()> {
        let slug = normalize_channel(channel)?;
        let mut conns = self.conns.lock().await;
        if let Some(conn) = conns.get_mut(&slug) {
            conn.consumers.insert(window.to_string());
            return Ok(());
        }

        let mut consumers = HashSet::new();
        consumers.insert(window.to_string());
        let http = self.http.clone();
        let task_slug = slug.clone();
        let (outgoing, receiver) = mpsc::channel(32);
        let send_state = Arc::new(AtomicU8::new(SEND_CONNECTING));
        let task_send_state = send_state.clone();
        let task = tokio::spawn(async move {
            run_connection(http, task_slug, receiver, task_send_state).await
        });
        conns.insert(
            slug,
            Connection {
                consumers,
                task,
                outgoing,
                send_state,
            },
        );
        inc_bridge_users();
        Ok(())
    }

    async fn disconnect(&self, channel: &str, window: &str) -> Result<()> {
        let slug = normalize_channel(channel)?;
        let mut conns = self.conns.lock().await;
        let should_remove = conns
            .get_mut(&slug)
            .map(|conn| {
                conn.consumers.remove(window);
                conn.consumers.is_empty()
            })
            .unwrap_or(false);
        if should_remove {
            if let Some(conn) = conns.remove(&slug) {
                conn.task.abort();
                dec_bridge_users();
            }
        }
        Ok(())
    }

    async fn send(&self, channel: &str, text: &str, reply_to: Option<&str>) -> Result<SendOutcome> {
        let drop = |reason: &str| SendOutcome {
            message_id: None,
            is_sent: false,
            drop_reason: Some(reason.to_string()),
        };
        let slug = normalize_channel(channel)?;
        let text = text.replace(['\r', '\n'], " ").trim().to_string();
        if text.is_empty() {
            return Ok(drop("Message is empty"));
        }
        if text.chars().count() > 400 {
            return Ok(drop("itzon messages are limited to 400 characters"));
        }
        if let Some(reply_to) = reply_to {
            if !valid_reply_id(reply_to) {
                return Ok(drop("Invalid itzon reply target"));
            }
        }
        if !crate::services::itzon_auth_service::ensure_valid().await {
            return Ok(drop("Connect your itzon account to send"));
        }
        let connection = self.conns.lock().await.get(&slug).map(|connection| {
            (
                connection.outgoing.clone(),
                connection.send_state.load(Ordering::SeqCst),
            )
        });
        let Some((outgoing, send_state)) = connection else {
            return Ok(drop("itzon chat is not connected"));
        };
        if send_state != SEND_READY {
            return Ok(drop(send_state_reason(send_state)));
        }
        let (response, result) = oneshot::channel();
        if outgoing
            .send(Outgoing {
                text,
                reply_to: reply_to.map(String::from),
                response,
            })
            .await
            .is_err()
        {
            return Ok(drop("itzon chat is reconnecting"));
        }
        match timeout(Duration::from_secs(12), result).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => Ok(drop("itzon connection reset before the send was confirmed")),
            Err(_) => Ok(drop("itzon did not confirm the message in time")),
        }
    }

    async fn send_capability(&self, channel: &str) -> SendCapability {
        if !crate::services::itzon_auth_service::ensure_valid().await {
            return SendCapability::NeedsLogin;
        }
        let Ok(slug) = normalize_channel(channel) else {
            return SendCapability::ReadOnly;
        };
        match self.conns.lock().await.get(&slug) {
            Some(connection) if connection.send_state.load(Ordering::SeqCst) == SEND_READY => {
                SendCapability::Sendable
            }
            _ => SendCapability::ReadOnly,
        }
    }
}

fn send_state_reason(state: u8) -> &'static str {
    match state {
        SEND_UNVERIFIED => "Verify your itzon email before sending",
        SEND_BANNED => "You are banned from this itzon channel",
        _ => "itzon chat is reconnecting",
    }
}

fn valid_reply_id(reply_to: &str) -> bool {
    !reply_to.is_empty()
        && reply_to.len() <= 256
        && reply_to
            .chars()
            .all(|ch| !ch.is_control() && !matches!(ch, ' ' | ';' | '=' | '\\'))
}

fn normalize_channel(channel: &str) -> Result<String> {
    let slug = channel.trim().trim_start_matches('#').to_lowercase();
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("invalid itzon channel")
    }
    Ok(slug)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsConfig {
    chat_wss: String,
}

async fn resolve_chat_url(http: &reqwest::Client) -> Result<Url> {
    let config = http
        .get(WS_CONFIG_URL)
        .send()
        .await
        .context("fetch itzon chat configuration")?
        .error_for_status()
        .context("itzon chat configuration response")?
        .json::<WsConfig>()
        .await
        .context("decode itzon chat configuration")?;
    trusted_chat_url(&config.chat_wss)
}

fn trusted_chat_url(base: &str) -> Result<Url> {
    let mut url = Url::parse(base).context("invalid itzon chat URL")?;
    if !matches!(url.scheme(), "wss" | "https") {
        bail!("itzon chat URL must use TLS")
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("itzon chat URL has no host"))?;
    if host != "itzon.tv" && !host.ends_with(".itzon.tv") {
        bail!("untrusted itzon chat host")
    }
    url.set_scheme("wss")
        .map_err(|_| anyhow!("invalid itzon chat URL scheme"))?;
    url.set_path("/ws/irc");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn run_connection(
    http: reqwest::Client,
    slug: String,
    mut outgoing: mpsc::Receiver<Outgoing>,
    send_state: Arc<AtomicU8>,
) {
    let mut backoff = 1;
    loop {
        send_state.store(SEND_CONNECTING, Ordering::SeqCst);
        let clean_disconnect =
            match connect_and_stream(&http, &slug, &mut outgoing, send_state.clone()).await {
                Ok(()) => {
                    log::info!("[itzon] chat '{}' disconnected", slug);
                    true
                }
                Err(err) => {
                    log::warn!("[itzon] chat '{}' error: {}", slug, err);
                    false
                }
            };
        if clean_disconnect {
            backoff = 1;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        if !clean_disconnect {
            backoff = next_backoff(backoff);
        }
    }
}

fn next_backoff(current: u64) -> u64 {
    current.saturating_mul(2).min(MAX_BACKOFF_SECS)
}

async fn connect_and_stream(
    http: &reqwest::Client,
    slug: &str,
    outgoing: &mut mpsc::Receiver<Outgoing>,
    send_state: Arc<AtomicU8>,
) -> Result<()> {
    // Resolve before JOIN so the server's backlog is parsed with the same emote
    // set as subsequent live messages.
    super::itzon_emotes::refresh(slug).await;
    let url = timeout(
        Duration::from_secs(CONFIG_TIMEOUT_SECS),
        resolve_chat_url(http),
    )
    .await
    .context("itzon chat configuration timed out")??;
    let auth = crate::services::itzon_auth_service::chat_auth().await;
    let has_auth = auth.pass.is_some() || auth.cookie_header.is_some();
    let auth_revision = crate::services::itzon_auth_service::revision();
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert(header::ORIGIN, HeaderValue::from_static("https://itzon.tv"));
    if let Some(cookie) = auth.cookie_header.as_deref() {
        request
            .headers_mut()
            .insert(header::COOKIE, HeaderValue::from_str(cookie)?);
    }
    let (ws, _) = timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .context("itzon chat connection timed out")??;
    let (mut write, mut read) = ws.split();
    let random = Uuid::new_v4().simple().to_string();
    let nick = auth
        .nick
        .unwrap_or_else(|| format!("guest_{}", &random[..8]));

    let mut registration = Vec::new();
    if let Some(pass) = auth.pass {
        // Itzon requires PASS to be the first IRC command for OAuth clients.
        registration.push(format!("PASS {pass}"));
    }
    registration.extend([
        "CAP REQ :message-tags echo-message draft/message-redaction".to_string(),
        "CAP REQ :server-time".to_string(),
        format!("NICK {nick}"),
        format!("USER {nick} 0 * :{nick}"),
    ]);
    for line in registration {
        write.send(Message::text(format!("{line}\r\n"))).await?;
    }

    let channel_key = key::make_key("itzon", slug);
    let wanted_channel = format!("#{slug}");
    let mut cap_replies = 0_u8;
    let mut cap_ended = false;
    let mut join_requested = false;
    let mut joined = false;
    let mut verified = None;
    let mut banned = false;
    let mut recent = RecentMessages::default();
    let mut members = MemberState::default();
    let mut confirmed_nick = nick.clone();
    let mut pending = VecDeque::<PendingSend>::new();
    let mut heartbeat = tokio::time::interval_at(
        Instant::now() + Duration::from_secs(READ_TIMEOUT_SECS),
        Duration::from_secs(READ_TIMEOUT_SECS),
    );
    let mut maintenance = tokio::time::interval(Duration::from_millis(500));
    maintenance.tick().await;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                write.send(Message::Ping(Vec::new().into())).await?;
            }
            _ = maintenance.tick() => {
                if crate::services::itzon_auth_service::revision() != auth_revision {
                    send_state.store(SEND_CONNECTING, Ordering::SeqCst);
                    return Err(anyhow!("itzon account session changed"));
                }
                while pending.front().map(|item| item.deadline <= Instant::now()).unwrap_or(false) {
                    if let Some(expired) = pending.pop_front() {
                        let _ = expired.response.send(SendOutcome {
                            message_id: None,
                            is_sent: false,
                            drop_reason: Some("itzon did not echo the message in time".into()),
                        });
                    }
                }
            }
            outgoing_message = outgoing.recv() => {
                let Some(outgoing_message) = outgoing_message else {
                    return Ok(());
                };
                if !has_auth || !crate::services::itzon_auth_service::is_connected() {
                    let _ = outgoing_message.response.send(SendOutcome {
                        message_id: None,
                        is_sent: false,
                        drop_reason: Some("Connect your itzon account to send".into()),
                    });
                    continue;
                }
                let state = send_state.load(Ordering::SeqCst);
                if state != SEND_READY {
                    let _ = outgoing_message.response.send(SendOutcome {
                        message_id: None,
                        is_sent: false,
                        drop_reason: Some(send_state_reason(state).into()),
                    });
                    continue;
                }
                let tag = outgoing_message
                    .reply_to
                    .as_deref()
                    .map(|id| format!("@+reply={id} "))
                    .unwrap_or_default();
                write
                    .send(Message::text(format!(
                        "{tag}PRIVMSG {wanted_channel} :{}\r\n",
                        outgoing_message.text
                    )))
                    .await?;
                pending.push_back(PendingSend {
                    text: outgoing_message.text,
                    reply_to: outgoing_message.reply_to,
                    response: outgoing_message.response,
                    deadline: Instant::now() + Duration::from_secs(10),
                });
            }
            frame = read.next() => match frame {
            None => return Ok(()),
            Some(Err(err)) => return Err(err.into()),
            Some(Ok(Message::Text(text))) => {
                for raw in text.split(['\r', '\n']).filter(|line| !line.is_empty()) {
                    let Some(line) = IrcLine::parse(raw) else {
                        continue;
                    };
                    match line.command.as_str() {
                        "PING" => {
                            let token = line
                                .trailing
                                .as_deref()
                                .or_else(|| line.params.first().map(String::as_str))
                                .unwrap_or("");
                            write
                                .send(Message::text(format!("PONG :{token}\r\n")))
                                .await?;
                        }
                        "CAP" => {
                            if line.params.iter().any(|p| p == "ACK" || p == "NAK") {
                                cap_replies = cap_replies.saturating_add(1);
                            }
                            if cap_replies >= 2 && !cap_ended {
                                write.send(Message::text("CAP END\r\n")).await?;
                                cap_ended = true;
                            }
                        }
                        "001" if !join_requested => {
                            if let Some(server_nick) = line.params.first() {
                                confirmed_nick = server_nick.clone();
                            }
                            write
                                .send(Message::text(format!("JOIN {wanted_channel}\r\n")))
                                .await?;
                            join_requested = true;
                        }
                        "JOIN" => {
                            let joined_channel = line
                                .params
                                .first()
                                .map(String::as_str)
                                .or(line.trailing.as_deref());
                            let joiner = line
                                .prefix
                                .as_deref()
                                .and_then(|prefix| prefix.split('!').next())
                                .unwrap_or("");
                            members.apply_tags(joiner, &line);
                            if joined_channel == Some(wanted_channel.as_str())
                                && joiner.eq_ignore_ascii_case(&confirmed_nick)
                            {
                                joined = true;
                                update_send_state(
                                    &send_state,
                                    joined,
                                    &confirmed_nick,
                                    has_auth,
                                    verified,
                                    banned,
                                );
                            }
                        }
                        "353" if line.params.get(2) == Some(&wanted_channel) => {
                            for entry in line
                                .trailing
                                .as_deref()
                                .unwrap_or("")
                                .split_whitespace()
                            {
                                members.apply_names_entry(entry);
                                if let Some(is_verified) =
                                    names_entry_verification(entry, &confirmed_nick)
                                {
                                    verified = Some(is_verified);
                                    update_send_state(
                                        &send_state,
                                        joined,
                                        &confirmed_nick,
                                        has_auth,
                                        verified,
                                        banned,
                                    );
                                }
                            }
                        }
                        "VERIFIED" if line.params.first() == Some(&wanted_channel) => {
                            let who = line.params.get(1).map(String::as_str).unwrap_or("");
                            members.set_verified(
                                who,
                                line.params.get(2).map(String::as_str) == Some("1"),
                            );
                            if who.eq_ignore_ascii_case(&confirmed_nick) {
                                verified = Some(line.params.get(2).map(String::as_str) == Some("1"));
                                update_send_state(
                                    &send_state,
                                    joined,
                                    &confirmed_nick,
                                    has_auth,
                                    verified,
                                    banned,
                                );
                            }
                        }
                        "META" if line.params.first() == Some(&wanted_channel) => {
                            if let Some(who) = line.params.get(1) {
                                members.apply_tags(who, &line);
                            }
                        }
                        "SUBBADGE" if line.params.first() == Some(&wanted_channel) => {
                            if let Some(who) = line.params.get(1) {
                                members.set_sub_badge(
                                    who,
                                    line.params.get(2).map(String::as_str).unwrap_or("regular"),
                                );
                            }
                        }
                        "PARTNER" if line.params.first() == Some(&wanted_channel) => {
                            if let Some(who) = line.params.get(1) {
                                members.set_partner(
                                    who,
                                    line.params.get(2).map(String::as_str) == Some("1"),
                                );
                            }
                        }
                        "474" => {
                            banned = true;
                            joined = false;
                            update_send_state(
                                &send_state,
                                joined,
                                &confirmed_nick,
                                has_auth,
                                verified,
                                banned,
                            );
                        }
                        "PRIVMSG" if line.params.first() == Some(&wanted_channel) => {
                            let sender = line
                                .prefix
                                .as_deref()
                                .and_then(|prefix| prefix.split('!').next())
                                .unwrap_or("");
                            members.apply_tags(sender, &line);
                            let body = privmsg_body(&line);
                            let reply = line.tags.get("+reply").map(String::as_str);
                            if sender.eq_ignore_ascii_case(&confirmed_nick) {
                                if let Some(index) = pending_echo_index(&pending, &body, reply) {
                                    if let Some(sent) = pending.remove(index) {
                                        let _ = sent.response.send(SendOutcome {
                                            message_id: line.tags.get("msgid").cloned(),
                                            is_sent: true,
                                            drop_reason: None,
                                        });
                                    }
                                }
                            }
                            if let Some(msg) =
                                normalize_privmsg(&line, &channel_key, &mut recent, &members)
                            {
                                publish_chat_message(&msg).await;
                            }
                        }
                        "NOTICE" | "404" if !pending.is_empty() => {
                            if let Some(failed) = pending.pop_front() {
                                let reason = line
                                    .trailing
                                    .as_deref()
                                    .or_else(|| line.params.last().map(String::as_str))
                                    .unwrap_or("itzon rejected the message");
                                let _ = failed.response.send(SendOutcome {
                                    message_id: None,
                                    is_sent: false,
                                    drop_reason: Some(reason.to_string()),
                                });
                            }
                        }
                        "REDACT" => {
                            if let Some(frame) =
                                normalize_redaction(&line, &wanted_channel, &channel_key)
                            {
                                publish_frame(frame).await;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                write.send(Message::Pong(payload)).await?;
            }
            Some(Ok(Message::Close(frame))) => {
                if let Some(frame) = frame {
                    log::warn!(
                        "[itzon] chat '{}' closed ({}): {}",
                        slug,
                        frame.code,
                        frame.reason
                    );
                }
                return Ok(());
            }
            Some(Ok(_)) => {}
            }
        }
    }
}

fn update_send_state(
    send_state: &AtomicU8,
    joined: bool,
    confirmed_nick: &str,
    has_auth: bool,
    verified: Option<bool>,
    banned: bool,
) {
    let state = if banned {
        SEND_BANNED
    } else if joined && verified == Some(false) {
        SEND_UNVERIFIED
    } else if joined
        && has_auth
        && verified == Some(true)
        && !confirmed_nick.to_ascii_lowercase().starts_with("guest_")
    {
        SEND_READY
    } else {
        SEND_CONNECTING
    };
    send_state.store(state, Ordering::SeqCst);
}

fn names_entry_verification(entry: &str, confirmed_nick: &str) -> Option<bool> {
    let prefix_len = names_prefix_len(entry);
    let name = &entry[prefix_len..];
    name.eq_ignore_ascii_case(confirmed_nick)
        .then(|| !entry[..prefix_len].contains('='))
}

fn pending_echo_index(
    pending: &VecDeque<PendingSend>,
    body: &str,
    reply_to: Option<&str>,
) -> Option<usize> {
    pending
        .iter()
        .position(|item| item.text == body && item.reply_to.as_deref() == reply_to)
}

#[derive(Debug, PartialEq)]
struct IrcLine {
    tags: HashMap<String, String>,
    prefix: Option<String>,
    command: String,
    params: Vec<String>,
    trailing: Option<String>,
}

impl IrcLine {
    fn parse(input: &str) -> Option<Self> {
        let mut rest = input.trim_end();
        let mut tags = HashMap::new();
        if rest.starts_with('@') {
            let (raw_tags, next) = rest[1..].split_once(' ')?;
            rest = next.trim_start();
            for raw in raw_tags.split(';') {
                let (name, value) = raw.split_once('=').unwrap_or((raw, ""));
                tags.insert(name.to_string(), unescape_tag(value));
            }
        }

        let prefix = if rest.starts_with(':') {
            let (raw_prefix, next) = rest[1..].split_once(' ')?;
            rest = next.trim_start();
            Some(raw_prefix.to_string())
        } else {
            None
        };

        let (head, trailing) = if let Some((head, trailing)) = rest.split_once(" :") {
            (head, Some(trailing.to_string()))
        } else {
            (rest, None)
        };
        let mut words = head.split_whitespace();
        let command = words.next()?.to_uppercase();
        let params = words.map(String::from).collect();
        Some(Self {
            tags,
            prefix,
            command,
            params,
            trailing,
        })
    }
}

fn unescape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[derive(Clone)]
struct RecentMessage {
    display_name: String,
    body: String,
    user_id: String,
    username: String,
}

#[derive(Default)]
struct RecentMessages {
    by_id: HashMap<String, RecentMessage>,
    order: VecDeque<String>,
}

#[derive(Default)]
struct MemberState {
    roles: HashMap<String, &'static str>,
    sub_badges: HashMap<String, String>,
    partners: HashSet<String>,
    unverified: HashSet<String>,
}

impl MemberState {
    fn apply_names_entry(&mut self, entry: &str) {
        let prefix_len = names_prefix_len(entry);
        let prefixes = &entry[..prefix_len];
        let nick = entry[prefix_len..].to_lowercase();
        if nick.is_empty() {
            return;
        }

        let role = if prefixes.contains('%') {
            Some("staff")
        } else if prefixes.contains('@') {
            Some("op")
        } else if prefixes.contains('&') {
            Some("bot")
        } else if prefixes.contains('+') {
            Some("mod")
        } else if prefixes.contains('~') {
            Some("vip")
        } else {
            None
        };
        if let Some(role) = role {
            self.roles.insert(nick.clone(), role);
        } else {
            self.roles.remove(&nick);
        }
        self.set_verified(&nick, !prefixes.contains('='));
    }

    fn apply_tags(&mut self, nick: &str, line: &IrcLine) {
        if let Some(name) = line.tags.get("sub-badge") {
            if name.is_empty() {
                self.sub_badges.remove(&nick.to_lowercase());
            } else {
                self.set_sub_badge(nick, name);
            }
        }
        if let Some(value) = line.tags.get("partner") {
            self.set_partner(nick, value == "1");
        }
    }

    fn set_sub_badge(&mut self, nick: &str, name: &str) {
        self.sub_badges
            .insert(nick.to_lowercase(), sanitize_badge_name(name));
    }

    fn set_partner(&mut self, nick: &str, enabled: bool) {
        let nick = nick.to_lowercase();
        if enabled {
            self.partners.insert(nick);
        } else {
            self.partners.remove(&nick);
        }
    }

    fn set_verified(&mut self, nick: &str, verified: bool) {
        let nick = nick.to_lowercase();
        if verified {
            self.unverified.remove(&nick);
        } else {
            self.unverified.insert(nick);
        }
    }

    fn badges_for(&self, nick: &str) -> Vec<Badge> {
        let nick = nick.to_lowercase();
        let mut badges = Vec::new();
        if let Some(role) = self.roles.get(&nick) {
            badges.push(itzon_badge(role, role_badge_title(role)));
        }
        if self.partners.contains(&nick) {
            badges.push(itzon_badge("partner", "Partner"));
        }
        if let Some(name) = self.sub_badges.get(&nick) {
            badges.push(itzon_badge(name, subscriber_badge_title(name)));
        }
        if self.unverified.contains(&nick) {
            badges.push(itzon_badge("unverified", "Unverified"));
        }
        badges
    }
}

impl RecentMessages {
    fn insert(&mut self, id: String, message: RecentMessage) {
        if self.by_id.insert(id.clone(), message).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > RECENT_MESSAGE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
    }
}

fn normalize_privmsg(
    line: &IrcLine,
    channel_key: &str,
    recent: &mut RecentMessages,
    members: &MemberState,
) -> Option<ChatMessage> {
    let prefix = line.prefix.as_deref()?;
    let display_name = prefix.split('!').next().unwrap_or(prefix).to_string();
    let username = display_name.to_lowercase();
    let mut content = privmsg_body(line);
    let is_action = content.starts_with("\u{1}ACTION ") && content.ends_with('\u{1}');
    if is_action {
        content = content[8..content.len() - 1].to_string();
    }
    let id = line
        .tags
        .get("msgid")
        .filter(|id| !id.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("itzon-{}", FALLBACK_SEQ.fetch_add(1, Ordering::Relaxed)));
    let user_id = line.tags.get("user-id").cloned().unwrap_or_default();
    let timestamp = line.tags.get("time").cloned().unwrap_or_default();
    let color = line.tags.get("color").filter(|v| !v.is_empty()).cloned();

    let mut tags = line.tags.clone();
    tags.insert("display-name".into(), display_name.clone());
    tags.insert("id".into(), id.clone());
    if user_id.chars().all(|c| c.is_ascii_digit()) && !user_id.is_empty() {
        if let Some(ext) = line
            .tags
            .get("avatar")
            .filter(|ext| matches!(ext.as_str(), "jpg" | "png" | "gif"))
        {
            tags.insert(
                "avatar".into(),
                format!("https://itzon.tv/api/live/avatar/{user_id}.{ext}"),
            );
        }
    }

    let badges = members.badges_for(&username);

    let reply_info = line
        .tags
        .get("+reply")
        .filter(|id| !id.is_empty())
        .map(|parent_id| {
            let parent = recent.by_id.get(parent_id);
            ReplyInfo {
                parent_msg_id: parent_id.clone(),
                parent_display_name: parent.map(|p| p.display_name.clone()).unwrap_or_default(),
                parent_msg_body: parent.map(|p| p.body.clone()).unwrap_or_default(),
                parent_user_id: parent.map(|p| p.user_id.clone()).unwrap_or_default(),
                parent_user_login: parent.map(|p| p.username.clone()).unwrap_or_default(),
            }
        });
    let has_reply = reply_info.is_some();

    let msg = ChatMessage {
        id: id.clone(),
        user_id: user_id.clone(),
        username: username.clone(),
        display_name: display_name.clone(),
        color,
        badges,
        timestamp,
        content: content.clone(),
        provider: "itzon".into(),
        channel: channel_key.into(),
        emotes: Vec::new(),
        tags,
        layout: LayoutResult {
            height: 0.0,
            width: 0.0,
            has_reply,
            is_first_message: false,
        },
        segments: super::itzon_emotes::parse_segments(
            channel_key.strip_prefix("itzon:").unwrap_or(channel_key),
            &content,
        ),
        metadata: MessageMetadata {
            is_action,
            reply_info,
            ..Default::default()
        },
    };
    recent.insert(
        id,
        RecentMessage {
            display_name,
            body: content,
            user_id,
            username,
        },
    );
    Some(msg)
}

/// Itzon's backlog omits IRC's leading `:` for some one-token messages (for
/// example `PRIVMSG #grandpachang boom`). In that shape the parser stores the
/// body as the second middle parameter instead of `trailing`. Accept both forms
/// so single-word text/emotes render and authenticated echoes still match.
fn privmsg_body(line: &IrcLine) -> String {
    line.trailing
        .clone()
        .or_else(|| line.params.get(1).cloned())
        .unwrap_or_default()
}

fn sanitize_badge_name(name: &str) -> String {
    let name = name.trim().to_lowercase();
    if !name.is_empty()
        && name.len() <= 24
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        name
    } else {
        "regular".into()
    }
}

fn role_badge_title(name: &str) -> &'static str {
    match name {
        "op" => "Owner",
        "staff" => "Staff",
        "bot" => "Bot",
        "mod" => "Moderator",
        "vip" => "VIP",
        "unverified" => "Unverified",
        _ => "itzon badge",
    }
}

fn subscriber_badge_title(name: &str) -> &str {
    match name {
        "regular" => "Regular",
        "ambassador" => "Ambassador - one of the first",
        "bounty" => "Bug bounty - found a critical bug",
        "invite" => "Recruiter - invited a friend",
        "lucky" => "Lucky - one in a million",
        "partner" => "Partner",
        _ => name,
    }
}

fn itzon_badge(name: &str, title: &str) -> Badge {
    let name = sanitize_badge_name(name);
    let asset = name.replace('_', "-");
    let url = format!("https://itzon.tv/static/img/badge-{asset}.svg");
    Badge {
        name,
        version: "1".into(),
        image_url_1x: Some(url.clone()),
        image_url_2x: Some(url.clone()),
        image_url_4x: Some(url),
        title: Some(title.to_string()),
        description: None,
    }
}

fn names_prefix_len(entry: &str) -> usize {
    entry
        .chars()
        .take_while(|ch| matches!(ch, '@' | '%' | '&' | '+' | '~' | '*' | '=' | '?'))
        .map(char::len_utf8)
        .sum()
}

fn normalize_redaction(line: &IrcLine, wanted_channel: &str, channel_key: &str) -> Option<String> {
    if line.params.first().map(String::as_str) != Some(wanted_channel) {
        return None;
    }
    let target = line
        .params
        .get(1)
        .cloned()
        .or_else(|| line.trailing.clone())?;
    Some(
        json!({
            "type": "CLEARMSG",
            "provider": "itzon",
            "channel": channel_key,
            "target_msg_id": target,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_only_trusted_chat_urls() {
        assert_eq!(
            trusted_chat_url("wss://ca-media.itzon.tv:8442")
                .unwrap()
                .as_str(),
            "wss://ca-media.itzon.tv:8442/ws/irc"
        );
        assert!(trusted_chat_url("ws://ca-media.itzon.tv:8442").is_err());
        assert!(trusted_chat_url("wss://itzon.tv.evil.example").is_err());
    }

    #[test]
    fn parses_ircv3_tags_and_escapes() {
        let line = IrcLine::parse(
            "@msgid=abc;color=#ff282e;note=hello\\sworld\\:ok :hawolt!guest@itzon.tv PRIVMSG #steqian :was geht",
        )
        .unwrap();
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.tags.get("note").unwrap(), "hello world;ok");
        assert_eq!(line.trailing.as_deref(), Some("was geht"));
    }

    #[test]
    fn normalizes_live_privmsg_metadata() {
        let line = IrcLine::parse("@time=2026-08-19T15:55:04.248Z;msgid=aa61;color=#ff282e;sub-badge=ambassador;partner=1;user-id=2;avatar=gif :Hawolt!guest@itzon.tv PRIVMSG #steqian :was geht").unwrap();
        let mut members = MemberState::default();
        members.apply_tags("Hawolt", &line);
        let msg = normalize_privmsg(
            &line,
            "itzon:steqian",
            &mut RecentMessages::default(),
            &members,
        )
        .unwrap();
        assert_eq!(msg.provider, "itzon");
        assert_eq!(msg.channel, "itzon:steqian");
        assert_eq!(msg.display_name, "Hawolt");
        assert_eq!(
            msg.tags.get("avatar").unwrap(),
            "https://itzon.tv/api/live/avatar/2.gif"
        );
        assert_eq!(msg.badges.len(), 2);
        assert_eq!(
            msg.badges[0].image_url_1x.as_deref(),
            Some("https://itzon.tv/static/img/badge-partner.svg")
        );
        assert_eq!(
            msg.badges[1].image_url_1x.as_deref(),
            Some("https://itzon.tv/static/img/badge-ambassador.svg")
        );
        assert_eq!(msg.content, "was geht");
    }

    #[test]
    fn normalizes_itzon_single_token_privmsg_without_trailing_marker() {
        let line = IrcLine::parse(
            "@msgid=one;user-id=1017 :GrandpaChang!guest@itzon.tv PRIVMSG #grandpachang boom",
        )
        .unwrap();
        assert!(line.trailing.is_none());
        assert_eq!(privmsg_body(&line), "boom");

        let msg = normalize_privmsg(
            &line,
            "itzon:grandpachang",
            &mut RecentMessages::default(),
            &MemberState::default(),
        )
        .unwrap();
        assert_eq!(msg.content, "boom");
    }

    #[test]
    fn maps_names_roles_and_avatarless_members_to_official_badges() {
        let mut members = MemberState::default();
        members.apply_names_entry("@Owner");
        members.apply_names_entry("+ModUser");
        members.apply_names_entry("=UnverifiedUser");

        let owner = members.badges_for("owner");
        assert_eq!(owner[0].name, "op");
        assert_eq!(
            owner[0].image_url_1x.as_deref(),
            Some("https://itzon.tv/static/img/badge-op.svg")
        );
        assert_eq!(members.badges_for("moduser")[0].name, "mod");
        assert_eq!(members.badges_for("unverifieduser")[0].name, "unverified");
    }

    #[test]
    fn enriches_replies_from_join_backlog() {
        let mut recent = RecentMessages::default();
        let parent =
            IrcLine::parse("@msgid=one;user-id=2 :Alpha!a@itzon.tv PRIVMSG #room :first").unwrap();
        let members = MemberState::default();
        normalize_privmsg(&parent, "itzon:room", &mut recent, &members).unwrap();
        let reply = IrcLine::parse(
            "@msgid=two;user-id=3;+reply=one :Beta!b@itzon.tv PRIVMSG #room :second",
        )
        .unwrap();
        let msg = normalize_privmsg(&reply, "itzon:room", &mut recent, &members).unwrap();
        let info = msg.metadata.reply_info.unwrap();
        assert_eq!(info.parent_display_name, "Alpha");
        assert_eq!(info.parent_msg_body, "first");
    }

    #[test]
    fn maps_redactions_to_shared_clearmsg_frame() {
        let line = IrcLine::parse(":go-irc REDACT #steqian abc123").unwrap();
        let frame = normalize_redaction(&line, "#steqian", "itzon:steqian").unwrap();
        assert!(frame.contains("\"target_msg_id\":\"abc123\""));
    }

    #[test]
    fn reconnect_backoff_caps() {
        let mut delay = 1;
        for expected in [2, 4, 8, 15, 15] {
            delay = next_backoff(delay);
            assert_eq!(delay, expected);
        }
    }

    #[test]
    fn itzon_matches_send_confirmation_by_body_and_reply_target() {
        let (response, _result) = oneshot::channel();
        let pending = VecDeque::from([PendingSend {
            text: "hello".into(),
            reply_to: Some("parent-1".into()),
            response,
            deadline: Instant::now() + Duration::from_secs(1),
        }]);
        assert_eq!(
            pending_echo_index(&pending, "hello", Some("parent-1")),
            Some(0)
        );
        assert_eq!(pending_echo_index(&pending, "hello", None), None);
        assert_eq!(
            pending_echo_index(&pending, "different", Some("parent-1")),
            None
        );
    }

    #[test]
    fn itzon_rejects_reply_targets_that_can_break_irc_tags() {
        assert!(valid_reply_id("01KACB0J87CD7RRGXPEN475KQ8"));
        assert!(valid_reply_id("parent-1.2:3"));
        assert!(!valid_reply_id(""));
        assert!(!valid_reply_id("parent id"));
        assert!(!valid_reply_id("parent;admin=1"));
        assert!(!valid_reply_id("parent\\sadmin"));
        assert!(!valid_reply_id("parent\r\nPRIVMSG #other :oops"));
    }

    #[test]
    fn itzon_channel_send_state_requires_account_join_auth_and_verification() {
        let state = AtomicU8::new(SEND_CONNECTING);

        update_send_state(&state, true, "guest_deadbeef", true, Some(true), false);
        assert_eq!(state.load(Ordering::SeqCst), SEND_CONNECTING);

        update_send_state(&state, true, "AccountName", false, Some(true), false);
        assert_eq!(state.load(Ordering::SeqCst), SEND_CONNECTING);

        update_send_state(&state, true, "AccountName", true, None, false);
        assert_eq!(state.load(Ordering::SeqCst), SEND_CONNECTING);

        update_send_state(&state, true, "AccountName", true, Some(false), false);
        assert_eq!(state.load(Ordering::SeqCst), SEND_UNVERIFIED);
        assert_eq!(
            send_state_reason(state.load(Ordering::SeqCst)),
            "Verify your itzon email before sending"
        );

        update_send_state(&state, true, "AccountName", true, Some(true), false);
        assert_eq!(state.load(Ordering::SeqCst), SEND_READY);

        update_send_state(&state, false, "AccountName", true, Some(true), true);
        assert_eq!(state.load(Ordering::SeqCst), SEND_BANNED);
        assert_eq!(
            send_state_reason(state.load(Ordering::SeqCst)),
            "You are banned from this itzon channel"
        );
    }

    #[test]
    fn itzon_names_snapshot_controls_email_verification() {
        assert_eq!(
            names_entry_verification("@~*AccountName", "accountname"),
            Some(true)
        );
        assert_eq!(
            names_entry_verification("@=*AccountName", "AccountName"),
            Some(false)
        );
        assert_eq!(
            names_entry_verification("?guest_deadbeef", "AccountName"),
            None
        );
    }
}
