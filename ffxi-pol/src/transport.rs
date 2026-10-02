//! Sequencing the chat-service key agreement over a byte channel.
//!
//! The layers this drives are byte-exact against the static reading of
//! polcore.dll 73b1864b; this module is the order they run in, read from the
//! chat state machine (`0x100140e0` outbound, `0x10015e80` inbound). It is
//! transport-agnostic: the caller supplies a `ByteChannel`, so the same
//! sequence runs against a live socket or an in-process mock. Nothing here
//! opens a connection or names a host.
//!
//! Scope: this reaches the point where the session Blowfish key is agreed and
//! the stream cipher is on, which is the fully-observed part of the handshake.
//! Producing the lobby authCode additionally needs the profile-service
//! community transaction and its reply assembly, which are the remaining
//! reverse-engineering.

use std::time::Instant;

use crate::authcode::{self, SELECT_PAYLOAD_LEN};
use crate::chat::{self, Greeting, Routing, POL_ADDRESS_LEN, ROUTING_LEN};
use crate::crypto::{PolBlowfish, StreamReset};
use crate::error::{Error, Result};
use crate::profile;
use crate::rng::RandomBytes;
use crate::rsa::{KeyPair, SESSION_KEY_BYTES};

/// A bidirectional byte channel. The chat service is line-oriented and its
/// stream cipher passes CR and LF through in the clear, so a reader can frame
/// on the newline whether or not the cipher is engaged. The profile service is
/// fixed-frame, so it reads exact counts.
pub trait ByteChannel {
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;
    /// Read up to and including the next `\n`.
    fn read_line(&mut self) -> Result<Vec<u8>>;
    /// Read exactly `n` bytes.
    fn read_exact(&mut self, n: usize) -> Result<Vec<u8>>;
}

/// Opens a byte channel to a named host and port. The chat and profile
/// services are separate connections on separate ports, so the login
/// orchestration takes a connector rather than a single channel. No host is
/// built in; the caller supplies it.
pub trait Connector {
    fn connect(&mut self, host: &str, port: u16) -> Result<Box<dyn ByteChannel>>;
}

/// numeric 300, which carries the RSA-encrypted session key.
const NUMERIC_KEY: &str = "300";
/// numeric 422 (ERR_NOMOTD), which the client treats as login complete.
const NUMERIC_REGISTERED: &str = "422";
/// polcore `0x10015e80` treats any numeric above this as a registration
/// failure rather than a stage of it.
const NUMERIC_ERROR_FLOOR: u32 = 399;

/// The outcome of the chat leg: the agreed session key, the enciphering
/// context the profile service inherits, and the two things the greeting and
/// the registration carry that the profile leg needs.
pub struct ChatSession {
    pub session_key: [u8; SESSION_KEY_BYTES],
    pub cipher: PolBlowfish,
    pub greeting: Greeting,
    /// polcore leaves its routing global zeroed until a post-registration
    /// numeric 300 fills it, so an absent one is host index 0, not an error.
    pub routing: Option<Routing>,
    /// When the greeting landed. polcore adopts the clock the greeting
    /// carries and counts from that moment, so anything later that stamps a
    /// time measures from here rather than from when the chat leg finished.
    pub greeting_at: Instant,
}

/// polcore `0x10013ea0` passes this as the USER mode digit for a member login.
pub const USER_MODE_MEMBER: u8 = 8;

/// Run the chat leg to the registered state. `account_id` is the member id as
/// a 41-bit integer, `credential` the account's chat secret, and `token` the
/// third part of the NICK (`chat::nick_token`).
pub fn agree_session_key(
    channel: &mut dyn ByteChannel,
    rng: &mut dyn RandomBytes,
    account_id: u64,
    credential: &[u8],
    token: &str,
    mode: u8,
) -> Result<ChatSession> {
    let key = KeyPair::generate(rng);

    // The greeting arrives in the clear; its third field is the challenge.
    let line = channel.read_line()?;
    let field3 = field(chat::line_body(&line, false), 3)
        .ok_or_else(|| Error::protocol("chat greeting has no challenge field"))?;
    let greeting = Greeting::parse(field3)?;
    let greeting_at = Instant::now();

    channel.write_all(&chat::build_user(&key, mode))?;

    // numeric 300 is still in the clear; its third field is the encrypted key.
    let key_line = channel.read_line()?;
    let key_body = chat::line_body(&key_line, true);
    if numeric(key_body) != Some(NUMERIC_KEY) {
        return Err(Error::protocol("expected numeric 300 after USER"));
    }
    let payload =
        field(key_body, 3).ok_or_else(|| Error::protocol("numeric 300 has no key field"))?;
    let session_key = key.recover_session_key(payload)?;

    // The cipher is on from here, seeded from our own modulus.
    let mut cipher = PolBlowfish::new(&session_key, key.stream_iv());
    let nick = chat::build_nick(account_id, &greeting.challenge, credential, token);
    channel.write_all(&cipher.stream(&nick, StreamReset::Boundary))?;

    // Registration lines are enciphered; each line reciphers from the IV. A
    // numeric 300 here is the routing struct, not another key: the same
    // numeric carries two encodings depending on how far the login has got.
    let mut routing = None;
    loop {
        let raw = channel.read_line()?;
        let plain = cipher.stream(&raw, StreamReset::Boundary);
        let body = chat::line_body(&plain, true);
        match numeric(body) {
            Some(NUMERIC_REGISTERED) => break,
            Some(NUMERIC_KEY) => routing = parse_routing(body),
            Some(n)
                if n.parse::<u32>()
                    .map(|v| v > NUMERIC_ERROR_FLOOR)
                    .unwrap_or(false) =>
            {
                return Err(Error::protocol(format!("chat registration refused: {n}")));
            }
            _ => continue,
        }
    }

    if routing.is_some_and(|r| r.refused) {
        return Err(Error::protocol("the chat service refused the account"));
    }

    Ok(ChatSession {
        session_key,
        cipher,
        greeting,
        routing,
        greeting_at,
    })
}

/// polcore `0x10015e80`: a numeric 300 outside the key agreement carries the
/// routing struct in base32 rather than a key in base64.
fn parse_routing(body: &[u8]) -> Option<Routing> {
    let decoded = chat::b32_decode(field(body, 3)?);
    let wire: &[u8; ROUTING_LEN] = decoded.get(..ROUTING_LEN)?.try_into().ok()?;
    Some(Routing::parse(&chat::routing_host_order(wire)))
}

/// The profile host this session's routing assigns, which is `pp000` when the
/// service sent no routing struct at all.
impl ChatSession {
    pub fn profile_host(&self) -> String {
        profile::host(self.routing.map_or(0, |r| r.host_index))
    }

    /// The service's clock as of now, which is what a request stamps itself
    /// with. The client carries no clock of its own into this: the greeting
    /// supplies the epoch and only the elapsed time since is local.
    pub fn now(&self) -> u64 {
        u64::from(self.greeting.clock()) + self.greeting_at.elapsed().as_secs()
    }
}

/// polcore `0x1001f0f0`: one profile-service connection. The service carries
/// exactly one transaction per connection: connect, a plaintext handshake that
/// yields the connection token, one enciphered request, one enciphered reply,
/// teardown. Nothing is pipelined.
pub struct ProfileConnection {
    channel: Box<dyn ByteChannel>,
    cipher: PolBlowfish,
    token: [u8; TOKEN_LEN],
}

const TOKEN_LEN: usize = 4;
const HANDSHAKE_REQUEST_LEN: usize = 0x28;
const REPLY_HEADER_LEN: usize = 0x18;
/// polcore `0x1001f390`: the family word is written as a literal and the
/// endpoint is copied from POL's own address struct, which begins with the
/// same value.
const ADDRESS_FAMILY: u16 = 1;
const HANDSHAKE_FAMILY_OFFSET: usize = 0x04;
const HANDSHAKE_ENDPOINT_OFFSET: usize = 0x06;
/// Bytes 2..8 of the address struct: the port and the address.
const ADDRESS_ENDPOINT: std::ops::Range<usize> = 0x02..0x08;
const HANDSHAKE_TOKEN_OFFSET: usize = 0x14;
/// polcore `0x1001f800`: an inbound body arrives in chunks of at most this.
const BODY_CHUNK_MAX: usize = 0x7F8;
/// polcore `0x1001fab0`: a counted reply opens with this many bytes, which
/// carry no checksum of their own.
const COUNT_PREAMBLE_LEN: usize = 8;

impl ProfileConnection {
    /// Open a connection and run the plaintext handshake. `addr20` is POL's own
    /// address struct, which the chat greeting reported back; only its first
    /// eight bytes reach the wire here.
    pub fn open(
        connector: &mut dyn Connector,
        host: &str,
        cipher: PolBlowfish,
        addr20: &[u8; POL_ADDRESS_LEN],
    ) -> Result<Self> {
        let mut channel = connector.connect(host, profile::PORT)?;

        let mut request = [0u8; HANDSHAKE_REQUEST_LEN];
        request[HANDSHAKE_FAMILY_OFFSET..HANDSHAKE_ENDPOINT_OFFSET]
            .copy_from_slice(&ADDRESS_FAMILY.to_le_bytes());
        request[HANDSHAKE_ENDPOINT_OFFSET..HANDSHAKE_ENDPOINT_OFFSET + ADDRESS_ENDPOINT.len()]
            .copy_from_slice(&addr20[ADDRESS_ENDPOINT]);
        channel.write_all(&request)?;

        let reply = channel.read_exact(REPLY_HEADER_LEN)?;
        let token = reply[HANDSHAKE_TOKEN_OFFSET..HANDSHAKE_TOKEN_OFFSET + TOKEN_LEN]
            .try_into()
            .map_err(|_| Error::protocol("the handshake reply is short"))?;

        Ok(Self {
            channel,
            cipher,
            token,
        })
    }

    /// polcore `0x1001f4d0` / `0x1001f970` / `0x1001f690` / `0x1001f800`: send
    /// one request and read its reply. The header restarts the keystream in
    /// each direction and the body continues from where its header left it, so
    /// the two directions replay the same first bytes.
    pub fn transact(
        &mut self,
        tx: profile::Transaction,
        member_id: &[u8],
        secret: &[u8],
        payload: &[u8],
        reply: ReplyShape,
    ) -> Result<Vec<u8>> {
        // polcore declares zero and sends nothing at all for a body-less
        // request; a declared length always includes the trailer, so zero
        // cannot mean "a checksum and nothing else".
        let declared = if payload.is_empty() {
            0
        } else {
            profile::declared_len(payload.len())
        };
        let digest = profile::authenticator(member_id, secret, self.token)?;
        let header = profile::request_header(tx, declared, digest);
        let enciphered = self.cipher.stream(&header, StreamReset::Boundary);
        self.channel.write_all(&enciphered)?;

        if !payload.is_empty() {
            let body = profile::seal_body(payload);
            let enciphered = self.cipher.stream(&body, StreamReset::Continue);
            self.channel.write_all(&enciphered)?;
        }

        let raw = self.channel.read_exact(REPLY_HEADER_LEN)?;
        let plain = self.cipher.stream(&raw, StreamReset::Boundary);
        let head = profile::parse_reply_header(&plain)?;
        if !head.is_ok() {
            return Err(Error::Status {
                service: "the PlayOnline profile service",
                transaction: tx.name(),
                status: head.status,
                code: head.error_code(),
                meaning: profile::status_meaning(head.status)
                    .unwrap_or("the service gave no reason this crate can name"),
            });
        }

        match reply {
            ReplyShape::None => Ok(Vec::new()),
            ReplyShape::Declared => self.read_body(head.body_len as usize),
            ReplyShape::Counted { record_len } => self.read_counted(record_len),
        }
    }

    fn read_body(&mut self, declared: usize) -> Result<Vec<u8>> {
        if declared == 0 {
            return Ok(Vec::new());
        }
        let raw = self.channel.read_exact(declared)?;
        let plain = self.cipher.stream(&raw, StreamReset::Continue);
        profile::open_body(&plain)
    }

    /// polcore `0x1001fab0` then a chunk loop: an 8-byte count preamble with no
    /// checksum, then `count` records read in chunks, the last one sealed.
    fn read_counted(&mut self, record_len: usize) -> Result<Vec<u8>> {
        let raw = self.channel.read_exact(COUNT_PREAMBLE_LEN)?;
        let preamble = self.cipher.stream(&raw, StreamReset::Continue);
        let count = usize::from(preamble[0]);

        let mut out = Vec::with_capacity(count * record_len);
        let mut left = count * record_len;
        while left > 0 {
            let chunk = left.min(BODY_CHUNK_MAX);
            let final_chunk = chunk == left;
            let wire = if final_chunk {
                chunk + profile::BODY_CHECKSUM_LEN
            } else {
                chunk
            };
            let raw = self.channel.read_exact(wire)?;
            let plain = self.cipher.stream(&raw, StreamReset::Continue);
            if final_chunk {
                out.extend_from_slice(&profile::open_body(&plain)?);
            } else {
                out.extend_from_slice(&plain);
            }
            left -= chunk;
        }
        Ok(out)
    }
}

/// How much of a reply to read, which the caller knows from the transaction.
#[derive(Clone, Copy, Debug)]
pub enum ReplyShape {
    /// No reply body at all.
    None,
    /// One body of the length the reply header declares.
    Declared,
    /// A count preamble followed by that many fixed-size records.
    Counted { record_len: usize },
}

/// What the player supplies. An account carries two identities and the
/// handshake uses both: the legacy PlayOnline pair authenticates the
/// connection, and the Square Enix pair authenticates the member. The Viewer
/// asks for all four on its own login form and keeps them in the member
/// record it saves per account; none of them is derived from another.
pub struct Account {
    /// The eight-character PlayOnline id. It is the chat handle, the profile
    /// identity, and where the profile host index is carried.
    pub playonline_id: [u8; profile::MEMBER_ID_LEN],
    /// The PlayOnline password. The chat NICK digest and the profile
    /// authenticator both take it verbatim, so it is capped at the fifteen
    /// characters the Viewer's own record holds.
    pub playonline_password: String,
    /// The Square Enix id, which the member-login body carries as its name.
    pub square_enix_id: String,
    /// The Square Enix password, which `member_secret` turns into the digest
    /// the member login proves.
    pub square_enix_password: String,
    /// The six characters of a security token, for an account that uses one.
    pub otp: Option<[u8; profile::OTP_LEN]>,
}

/// The lobby session the handshake yields: exactly what FFXiMain reads back
/// out of polcore and presents to the lobby server.
pub struct LobbySession {
    pub value: [u8; authcode::VALUE_LEN],
    pub auth_code: [u8; authcode::AUTHCODE_WIRE_LEN],
}

/// Everything the login produced, including the two replies whose interiors
/// this crate does not yet decode.
pub struct LoginOutcome {
    pub session: LobbySession,
    /// The selection the world select confirmed, which is what the community
    /// request carried.
    pub selection: authcode::SelectReply,
    pub profile_host: String,
}

/// Run the whole account handshake and produce a lobby session.
///
/// The order is polcore's: agree the chat key, take the profile host from the
/// routing the chat service assigns, then one connection per profile
/// transaction -- member login, world select, enter community -- and assemble
/// the reply into the lobby values. `selection` is what the world select asks
/// for, and its reply is what the community request carries;
/// `CommunityRequest::initial` asks the way a client that has never selected
/// anything does.
pub fn login(
    connector: &mut dyn Connector,
    rng: &mut dyn RandomBytes,
    account: &Account,
    chat_host: &str,
    selection: &authcode::CommunityRequest,
) -> Result<LoginOutcome> {
    if account.playonline_password.len() > profile::SECRET_MAX_LEN {
        return Err(Error::protocol(
            "a PlayOnline password is at most 15 characters",
        ));
    }
    let mut chat_channel = connector.connect(chat_host, chat::PORTS[0])?;
    let token = chat::nick_token(&account.playonline_id, &chat::TokenInputs::default());
    let session = agree_session_key(
        chat_channel.as_mut(),
        rng,
        profile::id_decode(&account.playonline_id)?,
        account.playonline_password.as_bytes(),
        &token,
        USER_MODE_MEMBER,
    )?;

    let host = session.profile_host();
    let host_index = session.routing.map_or(0, |r| r.host_index);
    // A refusal later on is hard to read without knowing which profile host
    // the chat service routed to, and whether it routed at all: with no
    // routing struct the host index falls back to zero.
    tracing::info!(
        profile_host = %host,
        routed = session.routing.is_some(),
        region = session.routing.map_or(0, |r| r.region),
        "PlayOnline chat leg complete"
    );
    let addr20 = session.greeting.address();
    let id = &account.playonline_id;
    let secret = account.playonline_password.as_bytes();

    let open = |connector: &mut dyn Connector| {
        ProfileConnection::open(connector, &host, session.cipher.clone(), &addr20)
    };

    // polcore stamps the login with the service's own clock, advanced by the
    // time since the greeting, so a slow handshake still rounds to the minute
    // the server expects.
    let cred = profile::MemberCredential::new(
        account.square_enix_id.clone(),
        &account.square_enix_password,
        account.otp,
    );
    let now = session.now();
    open(connector)?.transact(
        profile::MEMBER_LOGIN,
        id,
        secret,
        &profile::member_login_body(&cred, now)?,
        ReplyShape::None,
    )?;

    // The world select sends the head of the selection buffer and its reply
    // fills the rest in, so the community request is the confirmed selection
    // rather than the one that was asked for.
    let requested = selection.payload();
    let world_select_reply = open(connector)?.transact(
        profile::SELECT_SERVICE,
        id,
        secret,
        &requested[..SELECT_PAYLOAD_LEN],
        ReplyShape::Declared,
    )?;
    let confirmed = authcode::SelectReply::parse(&world_select_reply)?.confirm(selection);

    let community = open(connector)?.transact(
        profile::ENTER_COMMUNITY,
        id,
        secret,
        &confirmed.payload(),
        ReplyShape::Declared,
    )?;
    if community.len() < authcode::VALUE_LEN {
        return Err(Error::protocol("the community reply is short"));
    }

    let (value, auth_code) = authcode::assemble_session(
        &community,
        &authcode::AssemblyInputs {
            addr20: &addr20,
            clock: session.greeting.clock(),
            chat_key: session.session_key,
            host: host_index,
        },
    );

    Ok(LoginOutcome {
        session: LobbySession { value, auth_code },
        selection: authcode::SelectReply::parse(&world_select_reply)?,
        profile_host: host,
    })
}

/// The nth whitespace-separated field of a line, skipping an `:prefix` when
/// present. Fields are 1-based to match the protocol's own numbering.
fn field(line: &[u8], n: usize) -> Option<&[u8]> {
    let start = if line.first() == Some(&b':') {
        line.iter().position(|&b| b == b' ').map(|i| i + 1)?
    } else {
        0
    };
    line[start..]
        .split(|&b| b == b' ')
        .filter(|f| !f.is_empty())
        .nth(n - 1)
}

/// The command token of a line: field 1 after any prefix.
fn numeric(line: &[u8]) -> Option<&str> {
    field(line, 1).and_then(|f| std::str::from_utf8(f).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RandomBytes;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    struct SeqRng(u64);
    impl RandomBytes for SeqRng {
        fn fill(&mut self, out: &mut [u8]) {
            for b in out {
                self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
                *b = (self.0 >> 33) as u8;
            }
        }
    }

    const MOCK_HOST_INDEX: u8 = 7;
    const MOCK_REGION: u16 = 0x0102;
    const MOCK_TOKEN: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
    const MOCK_CLOCK: u32 = 0x5FED_C0DE;
    const MOCK_CLIENT_ADDR: [u8; 4] = [10, 0, 0, 9];
    const MOCK_CLIENT_PORT: u16 = 0x9A5B;
    const MOCK_CONTENT_INDEX: u8 = 0x1C;
    const MOCK_SERVICE_INDEX: u8 = 3;
    const MOCK_WORLD_INDEX: u8 = 4;

    // An in-process PlayOnline that speaks enough of both services to carry
    // the whole handshake. It exercises the client's path end to end; it is
    // not the retail server and proves nothing about interoperating with it.
    struct Mock {
        server_key: [u8; 8],
        member_id: Vec<u8>,
        secret: Vec<u8>,
        dialled: Vec<String>,
        seen: Vec<(u8, u8)>,
        login_payload: Vec<u8>,
        select_payload: Vec<u8>,
        community_payload: Vec<u8>,
        community_reply: [u8; authcode::REPLY_LEN - profile::BODY_CHECKSUM_LEN],
        profile_cipher: Option<PolBlowfish>,
    }

    impl Mock {
        fn greeting_record() -> [u8; chat::GREETING_DECODED_LEN] {
            let mut rec = [0u8; chat::GREETING_DECODED_LEN];
            rec[0x00..0x04].copy_from_slice(&MOCK_CLOCK.to_be_bytes());
            rec[0x04..0x08].copy_from_slice(&MOCK_CLIENT_ADDR);
            rec[0x14..0x16].copy_from_slice(&MOCK_CLIENT_PORT.to_be_bytes());
            rec
        }

        fn routing_record() -> [u8; chat::ROUTING_LEN] {
            let mut host_order = [0u8; chat::ROUTING_LEN];
            host_order[0..2].copy_from_slice(&MOCK_REGION.to_le_bytes());
            host_order[2] = MOCK_HOST_INDEX;
            // The transform is its own inverse, so it makes the wire form too.
            chat::routing_host_order(&host_order)
        }
    }

    impl Mock {
        fn new(server_key: [u8; 8], account: &Account) -> Self {
            Self {
                server_key,
                member_id: account.playonline_id.to_vec(),
                secret: account.playonline_password.as_bytes().to_vec(),
                dialled: Vec::new(),
                seen: Vec::new(),
                login_payload: Vec::new(),
                select_payload: Vec::new(),
                community_payload: Vec::new(),
                community_reply: [0x33; authcode::REPLY_LEN - profile::BODY_CHECKSUM_LEN],
                profile_cipher: None,
            }
        }
    }

    struct MockConnector(Rc<RefCell<Mock>>);

    impl Connector for MockConnector {
        fn connect(&mut self, host: &str, port: u16) -> Result<Box<dyn ByteChannel>> {
            self.0.borrow_mut().dialled.push(format!("{host}:{port}"));
            if port == chat::PORTS[0] {
                Ok(Box::new(ChatPipe {
                    mock: Rc::clone(&self.0),
                    outbound: VecDeque::from([chat::frame_line(&format!(
                        "020 target {}",
                        chat::b32_encode(&Mock::greeting_record())
                    ))]),
                    cipher: None,
                }))
            } else {
                Ok(Box::new(ProfilePipe {
                    mock: Rc::clone(&self.0),
                    outbound: Vec::new(),
                    cipher: None,
                    writes: 0,
                    request: None,
                }))
            }
        }
    }

    struct ChatPipe {
        mock: Rc<RefCell<Mock>>,
        outbound: VecDeque<Vec<u8>>,
        cipher: Option<PolBlowfish>,
    }

    impl ByteChannel for ChatPipe {
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            let body = chat::line_body(buf, true);
            if body.starts_with(b"USER ") {
                let colon = body.iter().position(|&b| b == b':').unwrap();
                let realname = &body[colon + 1..];
                let modulus_le = crate::rsa::b64_decode(realname, realname.len());
                let key = self.mock.borrow().server_key;
                let mut rng = SeqRng(0xFEED);
                let payload = crate::rsa::encrypt_session_key(&key, &modulus_le, &mut rng);
                self.outbound
                    .push_back(chat::frame_line(&format!("300 target {payload}")));
                let iv = crate::rsa::stream_iv_from_modulus_le(&modulus_le);
                let cipher = PolBlowfish::new(&key, iv);
                self.mock.borrow_mut().profile_cipher = Some(cipher.clone());
                self.cipher = Some(cipher);
            } else {
                // The NICK registers the member; the routing struct rides a
                // second numeric 300 before the 422 that ends the login.
                let routing = chat::frame_line(&format!(
                    "300 target {}",
                    chat::b32_encode(&Mock::routing_record())
                ));
                let registered = chat::frame_line("422 target :MOTD File is missing");
                for line in [routing, registered] {
                    let enciphered = self
                        .cipher
                        .as_mut()
                        .map(|c| c.stream(&line, StreamReset::Boundary))
                        .unwrap_or(line);
                    self.outbound.push_back(enciphered);
                }
            }
            Ok(())
        }

        fn read_line(&mut self) -> Result<Vec<u8>> {
            self.outbound
                .pop_front()
                .ok_or_else(|| Error::protocol("the chat mock has nothing to send"))
        }

        fn read_exact(&mut self, _n: usize) -> Result<Vec<u8>> {
            Err(Error::protocol("the chat mock is line-oriented"))
        }
    }

    struct ProfilePipe {
        mock: Rc<RefCell<Mock>>,
        outbound: Vec<u8>,
        cipher: Option<PolBlowfish>,
        writes: usize,
        request: Option<profile::Transaction>,
    }

    impl ProfilePipe {
        fn reply(&mut self, body: Vec<u8>) {
            let cipher = self.cipher.as_mut().unwrap();
            let mut head = [0u8; 0x18];
            head[0x04..0x08].copy_from_slice(&(body.len() as u32).to_le_bytes());
            self.outbound
                .extend_from_slice(&cipher.stream(&head, StreamReset::Boundary));
            if !body.is_empty() {
                self.outbound
                    .extend_from_slice(&cipher.stream(&body, StreamReset::Continue));
            }
        }
    }

    impl ByteChannel for ProfilePipe {
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.writes += 1;
            match self.writes {
                // The connect handshake is plaintext in both directions and
                // is the only place the token is issued.
                1 => {
                    assert_eq!(u16::from_le_bytes(buf[0x04..0x06].try_into().unwrap()), 1);
                    let mut head = [0u8; 0x18];
                    head[0x14..0x18].copy_from_slice(&MOCK_TOKEN);
                    self.outbound.extend_from_slice(&head);
                    self.cipher = self.mock.borrow().profile_cipher.clone();
                }
                _ => {
                    let cipher = self.cipher.as_mut().unwrap();
                    match self.request.take() {
                        None => {
                            let plain = cipher.stream(buf, StreamReset::Boundary);
                            let tx = profile::Transaction::new(plain[0x01], plain[0x02]);
                            let mock = self.mock.borrow();
                            let want =
                                profile::authenticator(&mock.member_id, &mock.secret, MOCK_TOKEN)
                                    .unwrap();
                            assert_eq!(&plain[0x18..0x28], &want, "authenticator for {tx:?}");
                            drop(mock);
                            self.mock.borrow_mut().seen.push((tx.category, tx.opcode));
                            let declared =
                                u32::from_le_bytes(plain[0x04..0x08].try_into().unwrap());
                            if declared == 0 {
                                self.serve(tx, Vec::new());
                            } else {
                                self.request = Some(tx);
                            }
                        }
                        Some(tx) => {
                            let plain = cipher.stream(buf, StreamReset::Continue);
                            let payload = profile::open_body(&plain)?;
                            self.serve(tx, payload);
                        }
                    }
                }
            }
            Ok(())
        }

        fn read_line(&mut self) -> Result<Vec<u8>> {
            Err(Error::protocol("the profile mock is frame-oriented"))
        }

        fn read_exact(&mut self, n: usize) -> Result<Vec<u8>> {
            if self.outbound.len() < n {
                return Err(Error::protocol("the profile mock has nothing to send"));
            }
            Ok(self.outbound.drain(..n).collect())
        }
    }

    impl ProfilePipe {
        fn serve(&mut self, tx: profile::Transaction, payload: Vec<u8>) {
            match tx {
                profile::MEMBER_LOGIN => {
                    self.mock.borrow_mut().login_payload = payload;
                    self.reply(Vec::new());
                }
                profile::SELECT_SERVICE => {
                    self.mock.borrow_mut().select_payload = payload;
                    // The mock answers with a wire body of the width polcore
                    // reads, so the payload under the trailer is what the
                    // client actually parses.
                    let mut reply = [0u8; authcode::SELECT_REPLY_LEN];
                    reply[0x00] = MOCK_CONTENT_INDEX;
                    reply[0x01] = 1;
                    reply[0x02] = MOCK_SERVICE_INDEX;
                    reply[0x03] = 1;
                    reply[0x04..0x06].copy_from_slice(&MOCK_REGION.to_le_bytes());
                    reply[0x06] = MOCK_WORLD_INDEX + 1;
                    self.reply(profile::seal_body(&reply));
                }
                profile::ENTER_COMMUNITY => {
                    let reply = self.mock.borrow().community_reply;
                    self.mock.borrow_mut().community_payload = payload;
                    self.reply(profile::seal_body(&reply));
                }
                other => panic!("the mock was asked for an unexpected transaction {other:?}"),
            }
        }
    }

    fn mock_account() -> Account {
        Account {
            playonline_id: *b"EFKAOYMJ",
            playonline_password: "polpass".to_string(),
            square_enix_id: "TESTMEMBER".to_string(),
            square_enix_password: "hunter2".to_string(),
            otp: None,
        }
    }

    #[test]
    fn the_client_and_a_mock_server_agree_on_the_same_session_key() {
        let server_key = [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18];
        let account = mock_account();
        let mock = Rc::new(RefCell::new(Mock {
            server_key,
            member_id: account.playonline_id.to_vec(),
            secret: account.playonline_password.as_bytes().to_vec(),
            dialled: Vec::new(),
            seen: Vec::new(),
            login_payload: Vec::new(),
            select_payload: Vec::new(),
            community_payload: Vec::new(),
            community_reply: [0x33; authcode::REPLY_LEN - profile::BODY_CHECKSUM_LEN],
            profile_cipher: None,
        }));
        let mut connector = MockConnector(Rc::clone(&mock));
        let mut channel = connector
            .connect(crate::hosts::CHAT_HOST, chat::PORTS[0])
            .unwrap();
        let mut rng = SeqRng(0x1234_5678);
        let session = agree_session_key(
            channel.as_mut(),
            &mut rng,
            0x1_2345,
            b"secret",
            "",
            USER_MODE_MEMBER,
        )
        .unwrap();
        assert_eq!(session.session_key, server_key);
        assert_eq!(session.greeting.clock(), MOCK_CLOCK);
        assert_eq!(session.routing.unwrap().host_index, MOCK_HOST_INDEX);
        assert_eq!(session.routing.unwrap().region, MOCK_REGION);
        assert!(!session.routing.unwrap().refused);
        assert_eq!(session.profile_host(), "pp007.pol.com");
    }

    #[test]
    fn the_session_clock_starts_at_the_greeting_not_at_the_end_of_the_chat_leg() {
        let server_key = [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18];
        let account = mock_account();
        let mock = Rc::new(RefCell::new(Mock::new(server_key, &account)));
        let mut connector = MockConnector(Rc::clone(&mock));
        let mut channel = connector
            .connect(crate::hosts::CHAT_HOST, chat::PORTS[0])
            .unwrap();
        let mut rng = SeqRng(0x1234_5678);
        let session = agree_session_key(
            channel.as_mut(),
            &mut rng,
            0x1_2345,
            b"secret",
            "",
            USER_MODE_MEMBER,
        )
        .unwrap();
        // The epoch is the service's, so the only local part is the elapsed
        // time, which is still inside the same second here.
        assert_eq!(session.now(), u64::from(MOCK_CLOCK));
    }

    #[test]
    fn the_whole_handshake_runs_against_a_mock_and_yields_a_lobby_session() {
        let server_key = [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18];
        let account = mock_account();
        let community_reply = [0x33u8; authcode::REPLY_LEN - profile::BODY_CHECKSUM_LEN];
        let mock = Rc::new(RefCell::new(Mock {
            server_key,
            member_id: account.playonline_id.to_vec(),
            secret: account.playonline_password.as_bytes().to_vec(),
            dialled: Vec::new(),
            seen: Vec::new(),
            login_payload: Vec::new(),
            select_payload: Vec::new(),
            community_payload: Vec::new(),
            community_reply,
            profile_cipher: None,
        }));
        let mut connector = MockConnector(Rc::clone(&mock));
        let mut rng = SeqRng(0x1234_5678);
        let selection = authcode::CommunityRequest {
            content_index: 3,
            service_available: true,
            service_index: 1,
            context_unset: false,
            region: MOCK_REGION,
            world_index_plus1: 4,
            service_flag: false,
            cached: true,
        };

        let outcome = login(
            &mut connector,
            &mut rng,
            &account,
            crate::hosts::CHAT_HOST,
            &selection,
        )
        .unwrap();

        let m = mock.borrow();
        // One connection for the chat leg, then one per profile transaction,
        // all to the host the chat service routed the account to.
        assert_eq!(
            m.dialled,
            vec![
                format!("{}:{}", crate::hosts::CHAT_HOST, chat::PORTS[0]),
                format!("pp007.pol.com:{}", profile::PORT),
                format!("pp007.pol.com:{}", profile::PORT),
                format!("pp007.pol.com:{}", profile::PORT),
            ]
        );
        assert_eq!(
            m.seen,
            vec![
                (profile::MEMBER_LOGIN.category, profile::MEMBER_LOGIN.opcode),
                (
                    profile::SELECT_SERVICE.category,
                    profile::SELECT_SERVICE.opcode
                ),
                (
                    profile::ENTER_COMMUNITY.category,
                    profile::ENTER_COMMUNITY.opcode
                ),
            ]
        );

        // The login carries the Square Enix id and a digest over the secret
        // the Square Enix password derives, not the PlayOnline password.
        assert_eq!(&m.login_payload[1..11], b"TESTMEMBER");
        let want = profile::member_login_body(
            &profile::MemberCredential::new("TESTMEMBER", "hunter2", None),
            u64::from(MOCK_CLOCK),
        )
        .unwrap();
        assert_eq!(m.login_payload, want);

        // The world select sends the head of the same buffer the community
        // request sends in full.
        assert_eq!(m.select_payload, selection.payload()[..SELECT_PAYLOAD_LEN]);
        // The community request carries what the world select confirmed, not
        // what was asked for: a different content entry and world index.
        assert_ne!(m.community_payload, selection.payload());
        assert_eq!(m.community_payload[0x10], MOCK_CONTENT_INDEX);
        assert_eq!(m.community_payload[0x12], MOCK_SERVICE_INDEX);
        assert_eq!(m.community_payload[0x16], MOCK_WORLD_INDEX + 1);

        assert_eq!(outcome.selection.content_index, Some(MOCK_CONTENT_INDEX));
        assert_eq!(outcome.selection.world_index, MOCK_WORLD_INDEX);
        assert_eq!(outcome.profile_host, "pp007.pol.com");

        let mut addr20 = [0u8; chat::POL_ADDRESS_LEN];
        addr20[0x00..0x02].copy_from_slice(&1u16.to_le_bytes());
        addr20[0x02..0x04].copy_from_slice(&MOCK_CLIENT_PORT.to_le_bytes());
        addr20[0x04..0x08].copy_from_slice(&u32::from_be_bytes(MOCK_CLIENT_ADDR).to_le_bytes());
        let (value, auth_code) = authcode::assemble_session(
            &community_reply,
            &authcode::AssemblyInputs {
                addr20: &addr20,
                clock: MOCK_CLOCK,
                chat_key: server_key,
                host: MOCK_HOST_INDEX,
            },
        );
        assert_eq!(outcome.session.value, value);
        assert_eq!(outcome.session.auth_code, auth_code);
        assert_ne!(
            outcome.session.auth_code,
            [0u8; authcode::AUTHCODE_WIRE_LEN]
        );
    }
}
