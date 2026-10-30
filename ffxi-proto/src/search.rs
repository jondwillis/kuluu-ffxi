//! FFXI search-server TCP protocol: frame crypto plus the Auction House
//! list/history request and response layouts, per LSB
//! `vendor/server/src/search/search_handler.{h,cpp}` and
//! `vendor/server/src/search/packets/auction_{list,history}.{h,cpp}`.

use crate::{blowfish, md5};

include!(concat!(env!("OUT_DIR"), "/search_handler_table.rs"));

pub const FRAME_LEN_OFFSET: usize = 0x00;
pub const MAGIC_OFFSET: usize = 0x04;
// vendor/server/src/search/search_handler.cpp SearchHandler::encrypt — "IXFF"
pub const MAGIC_IXFF: u32 = 0x4646_5849;
pub const REQUEST_TYPE_OFFSET: usize = 0x0B;

pub const FRAME_HEADER_SIZE: usize = 8;
pub const INTEGRITY_HASH_SIZE: usize = 16;
pub const TRAILER_KEY_SIZE: usize = 4;
// vendor/server/src/search/search_handler.cpp SearchHandler::read_func — `length < 28`
pub const MIN_FRAME_LEN: usize = 28;

// vendor/server/src/search/search_handler.cpp SearchHandler::validatePacket — hash at `length - 0x14`
pub const HASH_FROM_END: usize = 0x14;
// vendor/server/src/search/search_handler.cpp SearchHandler::decrypt — response-key word at `length - 0x18`
pub const SERVER_KEY_FROM_END: usize = 0x18;

// vendor/server/src/search/search_handler.cpp SearchHandler::decrypt — key splice offsets
pub const CLIENT_KEY_SPLICE: usize = 16;
pub const SERVER_KEY_SPLICE: usize = 20;
pub const REQUEST_KEY_HASH_LEN: usize = 20;
pub const FULL_KEY_LEN: usize = 24;

// vendor/server/src/search/search_handler.cpp SearchHandler::decrypt — `(length - 12) / 4` words,
// rounded down to an even count, enciphered from word offset 2
const CIPHER_SKIP: usize = FRAME_HEADER_SIZE + TRAILER_KEY_SIZE;

// vendor/server/src/search/search_handler.cpp SearchHandler::HandleAuctionHouseRequest AHCatID
pub const AH_LIST_SORT_COUNT_OFFSET: usize = 0x12;
pub const AH_LIST_CATEGORY_OFFSET: usize = 0x16;
pub const AH_LIST_PARAMS_OFFSET: usize = 0x18;
pub const AH_LIST_PARAM_STRIDE: usize = 8;

// vendor/server/src/search/search_handler.cpp SearchHandler::HandleAuctionHouseRequest — the only sort ids the
// server acts on (3=race, 4=job, 7=defense, 8=resistance are listed but ignored)
pub const SORT_LEVEL_DESC: u32 = 2;
pub const SORT_DAMAGE_DESC: u32 = 5;
pub const SORT_DELAY_DESC: u32 = 6;
pub const SORT_NAME: u32 = 9;

// vendor/server/src/search/search_handler.cpp SearchHandler::HandleAuctionHouseHistory ItemID
pub const AH_HISTORY_ITEM_ID_OFFSET: usize = 0x12;
pub const AH_HISTORY_STACK_OFFSET: usize = 0x15;
const AH_HISTORY_REQUEST_PAYLOAD_END: usize = 0x18;

pub const RESPONSE_BODY_SIZE_OFFSET: usize = 0x08;
pub const RESPONSE_FLAGS_OFFSET: usize = 0x0A;
// vendor/server/src/search/packets/auction_list.cpp CAHItemsListPacket::SetItemCount — final-packet marker
pub const RESPONSE_FINAL_FLAG: u8 = 0x80;
pub const RESPONSE_TYPE_OFFSET: usize = 0x0B;

// vendor/server/src/search/packets/auction_list.cpp m_offset
pub const AH_LIST_RESPONSE_TYPE: u8 = 0x95;
pub const AH_LIST_TOTAL_COUNT_OFFSET: usize = 0x0E;
pub const AH_LIST_ITEMS_OFFSET: usize = 0x18;
pub const AH_LIST_ITEM_SIZE: usize = 0x0A;
pub const AH_LIST_ITEMS_PER_PACKET: usize = 20;

// vendor/server/src/search/packets/auction_history.cpp CAHHistoryPacket::CAHHistoryPacket
pub const AH_HISTORY_RESPONSE_TYPE: u8 = 0x85;
// vendor/server/src/search/packets/auction_history.cpp CAHHistoryPacket::CAHHistoryPacket ("masked as val & 0x1F")
pub const AH_RESPONSE_TYPE_REQUEST_MASK: u8 = 0x1F;
pub const AH_HISTORY_ITEM_OFFSET: usize = 0x18;
pub const AH_HISTORY_OPEN_LISTINGS_OFFSET: usize = 0x1A;
pub const AH_HISTORY_CATEGORY_OFFSET: usize = 0x1E;
pub const AH_HISTORY_ROWS_OFFSET: usize = 0x20;
pub const AH_HISTORY_ROW_SIZE: usize = 40;
pub const AH_HISTORY_ROW_SELLER_OFFSET: usize = 0x08;
pub const AH_HISTORY_ROW_BUYER_OFFSET: usize = 0x18;
pub const AH_HISTORY_NAME_FIELD_SIZE: usize = 16;
pub const AH_HISTORY_MAX_ROWS: usize = 10;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SearchError {
    #[error("frame is {0} bytes, minimum {MIN_FRAME_LEN}")]
    TooShort(usize),
    #[error("length field {field} does not match frame size {actual}")]
    LengthMismatch { field: u16, actual: usize },
    #[error("integrity hash mismatch")]
    BadHash,
    #[error("magic {0:#010x} is not IXFF")]
    BadMagic(u32),
    #[error("trailer key {got:#010x} does not echo client key {want:#010x}")]
    KeyEchoMismatch { got: u32, want: u32 },
    #[error("response type {got:#04x}, expected {want:#04x}")]
    UnexpectedType { got: u8, want: u8 },
    #[error("body size field {field} is inconsistent with a {actual}-byte frame")]
    BadBodySize { field: u16, actual: usize },
    #[error("search list entry type {0:#04x} unknown (stream desync?)")]
    UnknownSearchEntry(u8),
}

fn rd_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}

fn rd_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn cipher_words(frame: &mut [u8], state: &blowfish::State, decrypt: bool) {
    let mut words = (frame.len() - CIPHER_SKIP) / 4;
    words -= words % 2;
    for w in (0..words).step_by(2) {
        let off = FRAME_HEADER_SIZE + w * 4;
        let mut xl = rd_u32(frame, off);
        let mut xr = rd_u32(frame, off + 4);
        if decrypt {
            blowfish::decipher(&mut xl, &mut xr, &state.p, &state.s);
        } else {
            blowfish::encipher(&mut xl, &mut xr, &state.p, &state.s);
        }
        frame[off..off + 4].copy_from_slice(&xl.to_le_bytes());
        frame[off + 4..off + 8].copy_from_slice(&xr.to_le_bytes());
    }
}

/// Per-connection key state. Mirrors the server's `SearchHandler::key` evolution
/// (vendor/server/src/search/search_handler.cpp SearchHandler::decrypt): each request splices the
/// client trailer key at [16..20) and the request's `len-0x18` word at [20..24),
/// and the response is enciphered under MD5 of the full 24 bytes.
pub struct SearchCrypto {
    key: [u8; FULL_KEY_LEN],
}

impl Default for SearchCrypto {
    fn default() -> Self {
        Self {
            key: SEARCH_BASE_KEY,
        }
    }
}

impl SearchCrypto {
    pub fn new() -> Self {
        Self::default()
    }

    fn encode_request(
        &mut self,
        len: usize,
        request_type: u8,
        client_key: u32,
        write_fields: impl FnOnce(&mut [u8]),
    ) -> Vec<u8> {
        debug_assert!(len >= MIN_FRAME_LEN);
        let mut frame = vec![0u8; len];
        frame[FRAME_LEN_OFFSET..FRAME_LEN_OFFSET + 2].copy_from_slice(&(len as u16).to_le_bytes());
        frame[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(&MAGIC_IXFF.to_le_bytes());
        frame[REQUEST_TYPE_OFFSET] = request_type;
        write_fields(&mut frame);
        // LSB only requires the plaintext `len-0x18` word to agree with what we
        // splice at key[20..24) — reuse the client key as that entropy word.
        let seed = client_key;
        frame[len - SERVER_KEY_FROM_END..len - SERVER_KEY_FROM_END + 4]
            .copy_from_slice(&seed.to_le_bytes());
        let hash = md5::md5(&frame[FRAME_HEADER_SIZE..len - HASH_FROM_END]);
        frame[len - HASH_FROM_END..len - HASH_FROM_END + INTEGRITY_HASH_SIZE]
            .copy_from_slice(&hash);

        self.key[CLIENT_KEY_SPLICE..CLIENT_KEY_SPLICE + 4]
            .copy_from_slice(&client_key.to_le_bytes());
        self.key[SERVER_KEY_SPLICE..SERVER_KEY_SPLICE + 4].copy_from_slice(&seed.to_le_bytes());

        let bf_key = md5::md5(&self.key[..REQUEST_KEY_HASH_LEN]);
        let state = blowfish::State::new(&bf_key);
        cipher_words(&mut frame, &state, false);
        frame[len - TRAILER_KEY_SIZE..].copy_from_slice(&client_key.to_le_bytes());
        frame
    }

    pub fn encode_ah_list_request(
        &mut self,
        category: u8,
        sorts: &[u32],
        client_key: u32,
    ) -> Vec<u8> {
        self.encode_ah_list(TCP_AH_REQUEST, category, sorts, client_key)
    }

    /// The follow-up page request. Retail asks for the catalog one 20-item page
    /// at a time; a server that pages on demand sends nothing further until
    /// this arrives (observation record
    /// .agents/skills/retail-observe/references/auction-house.md). Body is the
    /// original request's — `HandleAuctionHouseRequest` re-reads category and
    /// sort params for both types (search_handler.cpp SearchHandler::read_func TCP_AH_REQUEST).
    pub fn encode_ah_list_more_request(
        &mut self,
        category: u8,
        sorts: &[u32],
        client_key: u32,
    ) -> Vec<u8> {
        self.encode_ah_list(TCP_AH_REQUEST_MORE, category, sorts, client_key)
    }

    fn encode_ah_list(
        &mut self,
        request_type: u8,
        category: u8,
        sorts: &[u32],
        client_key: u32,
    ) -> Vec<u8> {
        let payload_end = AH_LIST_PARAMS_OFFSET + AH_LIST_PARAM_STRIDE * sorts.len();
        let len = payload_end + SERVER_KEY_FROM_END;
        self.encode_request(len, request_type, client_key, |frame| {
            frame[AH_LIST_SORT_COUNT_OFFSET] = sorts.len() as u8;
            frame[AH_LIST_CATEGORY_OFFSET] = category;
            for (i, sort) in sorts.iter().enumerate() {
                let off = AH_LIST_PARAMS_OFFSET + AH_LIST_PARAM_STRIDE * i;
                frame[off..off + 4].copy_from_slice(&sort.to_le_bytes());
            }
        })
    }

    pub fn encode_ah_history_request(
        &mut self,
        item_id: u16,
        stack: bool,
        client_key: u32,
    ) -> Vec<u8> {
        let request_type = if stack {
            TCP_AH_HISTORY_STACK
        } else {
            TCP_AH_HISTORY_SINGLE
        };
        let len = AH_HISTORY_REQUEST_PAYLOAD_END + SERVER_KEY_FROM_END;
        self.encode_request(len, request_type, client_key, |frame| {
            frame[AH_HISTORY_ITEM_ID_OFFSET..AH_HISTORY_ITEM_ID_OFFSET + 2]
                .copy_from_slice(&item_id.to_le_bytes());
            frame[AH_HISTORY_STACK_OFFSET] = stack as u8;
        })
    }

    pub fn decrypt_response(&self, frame: &[u8]) -> Result<Vec<u8>, SearchError> {
        let len = frame.len();
        if len < MIN_FRAME_LEN {
            return Err(SearchError::TooShort(len));
        }
        let field = rd_u16(frame, FRAME_LEN_OFFSET);
        if field as usize != len {
            return Err(SearchError::LengthMismatch { field, actual: len });
        }

        let mut body = frame.to_vec();
        let bf_key = md5::md5(&self.key);
        let state = blowfish::State::new(&bf_key);
        cipher_words(&mut body, &state, true);

        let hash = md5::md5(&body[FRAME_HEADER_SIZE..len - HASH_FROM_END]);
        if hash != body[len - HASH_FROM_END..len - HASH_FROM_END + INTEGRITY_HASH_SIZE] {
            return Err(SearchError::BadHash);
        }
        let magic = rd_u32(&body, MAGIC_OFFSET);
        if magic != MAGIC_IXFF {
            return Err(SearchError::BadMagic(magic));
        }
        let echoed = rd_u32(&body, len - TRAILER_KEY_SIZE);
        let client_key = rd_u32(&self.key, CLIENT_KEY_SPLICE);
        if echoed != client_key {
            return Err(SearchError::KeyEchoMismatch {
                got: echoed,
                want: client_key,
            });
        }
        Ok(body)
    }
}

/// The item ctor overwrites StackAmount for `stackSize == 1` items
/// (vendor/server/src/search/data_loader.cpp GetAHItemsToCategory).
pub const AH_NOT_STACKABLE: u32 = u32::MAX;

/// Open-listing COUNTS, not prices — the retail catalog's bracketed `[N]`
/// stock numbers (data_loader.cpp GetAHItemsToCategory:
/// SingleAmount = COUNT(*)-SUM(stack), StackAmount = SUM(stack) over
/// unsold rows). Prices appear only in sale history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AhListing {
    pub item_id: u16,
    /// Singles currently up for sale; 0 = none listed
    pub singles_for_sale: u32,
    /// Stacks currently up for sale; `None` = item is not stackable
    pub stacks_for_sale: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AhListPage {
    pub total_count: u16,
    pub listings: Vec<AhListing>,
    pub is_final: bool,
}

pub fn parse_ah_list(body: &[u8]) -> Result<AhListPage, SearchError> {
    if body.len() < AH_LIST_ITEMS_OFFSET {
        return Err(SearchError::TooShort(body.len()));
    }
    let got = body[RESPONSE_TYPE_OFFSET];
    if got != AH_LIST_RESPONSE_TYPE {
        return Err(SearchError::UnexpectedType {
            got,
            want: AH_LIST_RESPONSE_TYPE,
        });
    }
    let body_size = rd_u16(body, RESPONSE_BODY_SIZE_OFFSET);
    let items_bytes = (body_size as usize)
        .checked_sub(AH_LIST_ITEMS_OFFSET)
        .ok_or(SearchError::BadBodySize {
            field: body_size,
            actual: body.len(),
        })?;
    let count = items_bytes / AH_LIST_ITEM_SIZE;
    if !items_bytes.is_multiple_of(AH_LIST_ITEM_SIZE)
        || AH_LIST_ITEMS_OFFSET + count * AH_LIST_ITEM_SIZE > body.len()
    {
        return Err(SearchError::BadBodySize {
            field: body_size,
            actual: body.len(),
        });
    }
    let listings = (0..count)
        .map(|i| {
            let off = AH_LIST_ITEMS_OFFSET + AH_LIST_ITEM_SIZE * i;
            let stacks = rd_u32(body, off + 6);
            AhListing {
                item_id: rd_u16(body, off),
                singles_for_sale: rd_u32(body, off + 2),
                stacks_for_sale: (stacks != AH_NOT_STACKABLE).then_some(stacks),
            }
        })
        .collect();
    Ok(AhListPage {
        total_count: rd_u16(body, AH_LIST_TOTAL_COUNT_OFFSET),
        listings,
        is_final: body[RESPONSE_FLAGS_OFFSET] & RESPONSE_FINAL_FLAG != 0,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AhSale {
    pub price: u32,
    /// Unix timestamp (vendor/server/src/search/data_loader.cpp `sell_date` → `ahHistory::Data`)
    pub sell_date: u32,
    pub seller: String,
    pub buyer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AhHistory {
    pub item_id: u16,
    /// Count of open listings of the requested form (single vs stack), not a
    /// price (auction_history.cpp ctor takes GetAHItemFromItemID's
    /// Single/StackAmount; the not-stackable override does not apply here)
    pub open_listings: u32,
    pub category: u16,
    pub sales: Vec<AhSale>,
}

fn name_field(body: &[u8], off: usize) -> String {
    let raw = &body[off..off + AH_HISTORY_NAME_FIELD_SIZE];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

pub fn parse_ah_history(body: &[u8]) -> Result<AhHistory, SearchError> {
    if body.len() < AH_HISTORY_ROWS_OFFSET {
        return Err(SearchError::TooShort(body.len()));
    }
    let got = body[RESPONSE_TYPE_OFFSET];
    if got != AH_HISTORY_RESPONSE_TYPE {
        return Err(SearchError::UnexpectedType {
            got,
            want: AH_HISTORY_RESPONSE_TYPE,
        });
    }
    let body_size = rd_u16(body, RESPONSE_BODY_SIZE_OFFSET);
    // vendor/server/src/search/packets/auction_history.cpp CAHHistoryPacket::AddItem — the size field is
    // only written once a row is added; a zero-sale response leaves it 0.
    let rows_bytes = (body_size as usize).saturating_sub(AH_HISTORY_ROWS_OFFSET);
    let count = rows_bytes / AH_HISTORY_ROW_SIZE;
    if !rows_bytes.is_multiple_of(AH_HISTORY_ROW_SIZE)
        || count > AH_HISTORY_MAX_ROWS
        || AH_HISTORY_ROWS_OFFSET + count * AH_HISTORY_ROW_SIZE > body.len()
    {
        return Err(SearchError::BadBodySize {
            field: body_size,
            actual: body.len(),
        });
    }
    let sales = (0..count)
        .map(|i| {
            let off = AH_HISTORY_ROWS_OFFSET + AH_HISTORY_ROW_SIZE * i;
            AhSale {
                price: rd_u32(body, off),
                sell_date: rd_u32(body, off + 4),
                seller: name_field(body, off + AH_HISTORY_ROW_SELLER_OFFSET),
                buyer: name_field(body, off + AH_HISTORY_ROW_BUYER_OFFSET),
            }
        })
        .collect();
    Ok(AhHistory {
        item_id: rd_u16(body, AH_HISTORY_ITEM_OFFSET),
        open_listings: rd_u32(body, AH_HISTORY_OPEN_LISTINGS_OFFSET),
        category: rd_u16(body, AH_HISTORY_CATEGORY_OFFSET),
        sales,
    })
}

// ---- Player search (`searchma` row): request TLV + CSearchListPacket reply ----

// vendor/server/src/search/search_handler.h TCP_SEARCH / TCP_SEARCH_ALL dispatch
pub const SEARCH_LIST_RESPONSE_TYPE: u8 = 0x80;
// vendor/server/src/search/packets/search_list.cpp CSearchListPacket ctor — m_offset(192)
pub const SEARCH_LIST_ENTRIES_BIT_OFFSET: usize = 192;
// vendor/server/src/search/packets/search_list.cpp AddPlayer — "A maximum of 20 characters can be added to a single packet."
pub const SEARCH_LIST_PLAYERS_PER_PACKET_MAX: usize = 20;
/// Byte count of the TLV entry blob starting at `SEARCH_REQUEST_ENTRIES_OFFSET`;
/// vendor/server/src/search/search_handler.cpp _HandleSearchRequest reads it as
/// `size * 8` workload bits.
pub const SEARCH_REQUEST_ENTRY_BYTES_OFFSET: usize = 0x10;
pub const SEARCH_REQUEST_ENTRIES_OFFSET: usize = 0x11;
// vendor/server/src/search/packets/search_list.cpp ctor — total found across all frames
pub const SEARCH_LIST_TOTAL_COUNT_OFFSET: usize = 0x0E;

// vendor/server/src/search/enums/search_type.h SearchType
pub mod search_type {
    pub const NAME: u8 = 0x00;
    pub const AREA: u8 = 0x01;
    pub const NATION: u8 = 0x02;
    pub const JOB: u8 = 0x03;
    pub const LEVEL: u8 = 0x04;
    pub const RACE: u8 = 0x05;
    pub const FLAGS1: u8 = 0x06;
    pub const ID: u8 = 0x08;
    pub const PARTY: u8 = 0x0A;
    pub const LINKSHELL: u8 = 0x0B;
    pub const FRIEND: u8 = 0x0C;
    pub const LINKSHELL_RANK: u8 = 0x0D;
    pub const UNKNOWN_0E: u8 = 0x0E;
    pub const RANK: u8 = 0x10;
    pub const COMMENT: u8 = 0x11;
    pub const LINKSHELL2: u8 = 0x13;
    pub const FLAGS2: u8 = 0x16;
    pub const LANGUAGE: u8 = 0x17;
}

use crate::bitstream::{pack_bits_le, unpack_bits_le};

// The bit codecs live in crate::bitstream (shared with the linkshell-name codec);
// they are ports of packBitsLE/unpackBitsLE from vendor/server/src/common/utils.cpp.

/// Sequential writer over the request TLV blob: one [`pack`](Self::pack) call per
/// bit field, mirroring how search_handler.cpp walks its `unpackBitsLE` cursor.
pub struct SearchBitWriter {
    pub buf: Vec<u8>,
    pub bits: usize,
}

impl Default for SearchBitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchBitWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            bits: 0,
        }
    }
    pub fn pack(&mut self, value: u64, len: u32) {
        let need_bytes = (self.bits + len as usize + 15) / 8;
        if self.buf.len() < need_bytes {
            self.buf.resize(need_bytes, 0);
        }
        self.bits = pack_bits_le(&mut self.buf, value, self.bits, len);
    }
}

/// One `searchma` filter as the retail form exposes them (vault main_menu.md
/// section 26). Each becomes one TLV entry; areas append the list-end marker
/// (_HandleSearchRequest: `isPresent == 0` ends the Area run).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchFilter {
    Name(String),
    Level { from: u8, to: u8 },
    Job(u8),
    Race(u8),
    Nation(u8),
    Rank { min: u8, max: u8 },
    Areas(Vec<u16>),
}

impl SearchCrypto {
    /// Builds the TCP_SEARCH (or TCP_SEARCH_ALL when `all_pages`) request whose
    /// TLV blob LSB's `_HandleSearchRequest` consumes. Entry order follows the
    /// retail form's reading order as encoded by our caller.
    pub fn encode_player_search_request(
        &mut self,
        filters: &[SearchFilter],
        all_pages: bool,
        client_key: u32,
    ) -> Vec<u8> {
        let mut w = SearchBitWriter::new();
        for filter in filters {
            match filter {
                // Name entry: {type:5}{sort:1}{present:1}{len:5}{chars ×7}
                SearchFilter::Name(name) => {
                    let chars: Vec<u8> = name.bytes().take(31).collect();
                    w.pack(u64::from(search_type::NAME), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    // LSB clamps rawNameLen to 15 but consumes every char the length says.
                    w.pack(chars.len() as u64, 5);
                    for c in chars {
                        w.pack(c as u64, 7);
                    }
                }
                SearchFilter::Level { from, to } => {
                    w.pack(u64::from(search_type::LEVEL), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    w.pack(*from as u64, 8);
                    w.pack(*to as u64, 8);
                }
                SearchFilter::Job(job) => {
                    w.pack(u64::from(search_type::JOB), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    w.pack(*job as u64, 5);
                }
                SearchFilter::Race(race) => {
                    w.pack(u64::from(search_type::RACE), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    w.pack(*race as u64, 4);
                }
                SearchFilter::Nation(nation) => {
                    w.pack(u64::from(search_type::NATION), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    w.pack(*nation as u64 & 0x3, 2);
                }
                SearchFilter::Rank { min, max } => {
                    w.pack(u64::from(search_type::RANK), 5);
                    w.pack(0, 1);
                    w.pack(1, 1);
                    w.pack(*min as u64, 8);
                    w.pack(*max as u64, 8);
                }
                SearchFilter::Areas(zones) => {
                    for zone in zones {
                        w.pack(u64::from(search_type::AREA), 5);
                        w.pack(0, 1);
                        w.pack(1, 1);
                        w.pack(*zone as u64 & 0x3FF, 10);
                    }
                    // list end: isPresent = 0 (search_handler.cpp "Area List End found.")
                    w.pack(u64::from(search_type::AREA), 5);
                    w.pack(0, 1);
                    w.pack(0, 1);
                }
            }
        }
        let entry_bytes = w.bits.div_ceil(8);
        let len = SEARCH_REQUEST_ENTRIES_OFFSET + entry_bytes + SERVER_KEY_FROM_END;
        self.encode_request(
            len,
            if all_pages {
                TCP_SEARCH_ALL
            } else {
                TCP_SEARCH
            },
            client_key,
            |frame| {
                frame[SEARCH_REQUEST_ENTRY_BYTES_OFFSET] = entry_bytes as u8;
                frame[SEARCH_REQUEST_ENTRIES_OFFSET..][..entry_bytes]
                    .copy_from_slice(&w.buf[..entry_bytes]);
            },
        )
    }
}

/// One player row of a `CSearchListPacket` (vendor/server/src/search/packets/search_list.cpp AddPlayer).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchHit {
    pub name: String,
    pub zone: u16,
    /// Absent while flags1 & 0x4000 — the hidden/GM-shape entry skip (AddPlayer).
    pub nation: Option<u8>,
    pub main_job: Option<u8>,
    pub sub_job: Option<u8>,
    pub main_level: Option<u8>,
    pub sub_level: Option<u8>,
    pub race: Option<u8>,
    pub rank: Option<u8>,
    pub flags1: u16,
    /// Packed 20 bits wide.
    pub id: u32,
    /// seacom_type, present only when non-zero (AddPlayer).
    pub comment_type: Option<u32>,
    pub flags2: u32,
    pub languages: u16,
}

/// One decrypted results frame of a player search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchListFrame {
    pub is_final: bool,
    /// Total characters found across all frames (may exceed what this frame carries).
    pub total_count: u16,
    pub players: Vec<SearchHit>,
}

struct BitCursor<'a> {
    buf: &'a [u8],
    bits: usize,
}

impl<'a> BitCursor<'a> {
    fn new(buf: &'a [u8], bits: usize) -> Self {
        Self { buf, bits }
    }
    fn take(&mut self, len: u32) -> u64 {
        let v = unpack_bits_le(self.buf, self.bits, len);
        self.bits += len as usize;
        v
    }
}

/// Parses one decrypted `CSearchListPacket` body (response type 0x80).
/// Entries live in bytes [24, body_size) with a per-entity size byte preceding each
/// bit-packed blob; frames chain until the final flag (search_handler.cpp
/// HandleSearchRequest emits up to 20 players per frame and flags only the last).
pub fn parse_search_list(body: &[u8]) -> Result<SearchListFrame, SearchError> {
    if body.len() < SEARCH_LIST_ENTRIES_BIT_OFFSET / 8 {
        return Err(SearchError::TooShort(body.len()));
    }
    let got = body[RESPONSE_TYPE_OFFSET];
    if got != SEARCH_LIST_RESPONSE_TYPE {
        return Err(SearchError::UnexpectedType {
            got,
            want: SEARCH_LIST_RESPONSE_TYPE,
        });
    }
    // search_list.cpp ctor memsets m_data, so a zero-player frame leaves the size field 0.
    let data_size = (rd_u16(body, RESPONSE_BODY_SIZE_OFFSET) as usize).min(body.len());
    let mut players = Vec::new();
    let mut pos = SEARCH_LIST_ENTRIES_BIT_OFFSET / 8;
    while pos < data_size {
        // One entity's entries occupy exactly `size_bytes` bytes after the size
        // byte (search_list.cpp: `ref<uint8>(data, size_offset) = m_offset / 8 - size_offset - 1`).
        let size_bytes = body[pos] as usize;
        let end_bit = (pos + 1 + size_bytes) * 8;
        let mut cur = BitCursor::new(body, (pos + 1) * 8);
        let mut hit = SearchHit::default();
        // The smallest response entry — Name with a zero-length field — is 9 bits,
        // so anything shorter than that past the cursor is AddPlayer's byte-alignment pad.
        while end_bit.saturating_sub(cur.bits) >= 9 {
            match cur.take(5) as u8 {
                search_type::NAME => {
                    let len = (cur.take(4) as usize).min(15);
                    let mut s = String::new();
                    for _ in 0..len {
                        s.push(((cur.take(7) & 0x7F) as u8).into());
                    }
                    hit.name = s;
                }
                search_type::AREA => hit.zone = cur.take(10) as u16,
                search_type::NATION => hit.nation = Some(cur.take(2) as u8),
                search_type::JOB => {
                    hit.main_job = Some(cur.take(5) as u8);
                    hit.sub_job = Some(cur.take(5) as u8);
                }
                search_type::LEVEL => {
                    hit.main_level = Some(cur.take(8) as u8);
                    hit.sub_level = Some(cur.take(8) as u8);
                }
                search_type::RACE => hit.race = Some(cur.take(4) as u8),
                search_type::RANK => hit.rank = Some(cur.take(8) as u8),
                search_type::FLAGS1 => hit.flags1 = cur.take(16) as u16,
                search_type::ID => hit.id = cur.take(20) as u32,
                // The packer always emits this zero entry; its meaning is unknown upstream.
                search_type::UNKNOWN_0E => {
                    cur.take(32);
                }
                search_type::COMMENT => hit.comment_type = Some(cur.take(32) as u32),
                search_type::FLAGS2 => hit.flags2 = cur.take(32) as u32,
                search_type::LANGUAGE => hit.languages = cur.take(16) as u16,
                other => return Err(SearchError::UnknownSearchEntry(other)),
            }
        }
        players.push(hit);
        pos += size_bytes + 1;
    }
    Ok(SearchListFrame {
        is_final: body[RESPONSE_FLAGS_OFFSET] & RESPONSE_FINAL_FLAG != 0,
        total_count: rd_u16(body, SEARCH_LIST_TOTAL_COUNT_OFFSET),
        players,
    })
}

#[cfg(test)]
mod player_search_tests {
    use super::*;

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn bit_codec_matches_lsb_packvectors_at_nasty_offsets() {
        // Generated from a line-by-line port of utils.cpp packBitsLE, kept here so the
        // Rust mirror is pinned to the upstream algorithm rather than a guess.
        let cases = [
            (4usize, 12u32, 0xAB1u64, "0ab10000"),
            (7, 3, 0x5, "01400000"),
            (5, 16, 0xF00D, "07806800"),
            (0, 32, 0xDEAD_BEEF, "deadbeef"),
            (6, 10, 0x2AB, "02ab0000"),
            (1, 7, 0x4E, "4e000000"),
            (3, 20, 0xABCDE, "1579bc00"),
            (2, 8, 0xFF, "3fc00000"),
        ];
        for (bo, len, value, want) in cases {
            let mut buf = vec![0u8; 4];
            let next = pack_bits_le(&mut buf, value, bo, len);
            assert_eq!(bytes_to_hex(&buf), want, "pack {value:#x} @{bo}/{len}");
            assert_eq!(next, bo + len as usize);
            assert_eq!(unpack_bits_le(&buf, bo, len), value, "unpack mirror");
        }
    }

    #[test]
    fn sequential_streams_round_trip_through_the_lsb_pair() {
        // FIXTURE A: widths the search TLV actually uses, packed back to back.
        let stream: [(u64, u32); 9] = [
            (0x04, 5),
            (0, 1),
            (1, 1),
            (255, 8),
            (99, 8),
            (0x17, 5),
            (1, 1),
            (0, 1),
            (0xBEEF, 16),
        ];
        let mut w = SearchBitWriter::new();
        for (v, l) in stream {
            w.pack(v, l);
        }
        assert_eq!(w.bits, 46);
        // The reference buffer was an 8-byte zero pad; the writer stops at the last
        // touched byte — the trailing padding nibble agrees either way.
        assert_eq!(bytes_to_hex(&w.buf), "23fec77afbbc00");
        let mut pos = 0;
        for (v, l) in stream {
            assert_eq!(unpack_bits_le(&w.buf, pos, l), v);
            pos += l as usize;
        }
    }

    fn bytes_to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    const CLIENT_KEY: u32 = 0xA1B2_C3D4;

    /// SearchHandler::decrypt (vendor/server/src/search/search_handler.cpp) — splice the trailer
    /// key, decipher with MD5(key[..20]), then splice the plaintext `len-0x18` word.
    fn lsb_decrypt_request(frame: &mut [u8]) {
        let len = frame.len();
        let mut key = SEARCH_BASE_KEY;
        key[16..20].copy_from_slice(&frame[len - 4..]);
        let state = blowfish::State::new(&md5::md5(&key[..20]));
        let mut words = (len - 12) / 4;
        words -= words % 2;
        for i in (0..words).step_by(2) {
            let a = 4 * (i + 2);
            let b = 4 * (i + 3);
            let mut xl = u32::from_le_bytes(frame[a..a + 4].try_into().unwrap());
            let mut xr = u32::from_le_bytes(frame[b..b + 4].try_into().unwrap());
            blowfish::decipher(&mut xl, &mut xr, &state.p, &state.s);
            frame[a..a + 4].copy_from_slice(&xl.to_le_bytes());
            frame[b..b + 4].copy_from_slice(&xr.to_le_bytes());
        }
        key[20..24].copy_from_slice(&frame[len - 0x18..len - 0x18 + 4]);
        assert_eq!(
            md5::md5(&frame[8..len - 0x14]),
            frame[len - 0x14..len - 0x14 + 16],
            "server-side request hash"
        );
    }

    #[test]
    fn request_entries_land_exactly_where_lsb_parses_them() {
        // FIXTURE D: Name("bob"), Level 10-99, Job 5, Area[103]+list end — the exact
        // byte blob _HandleSearchRequest consumes after frame[0x10] says 12 bytes.
        let mut crypto = SearchCrypto::new();
        let mut frame = crypto.encode_player_search_request(
            &[
                SearchFilter::Name("bob".into()),
                SearchFilter::Level { from: 10, to: 99 },
                SearchFilter::Job(5),
                SearchFilter::Areas(vec![103]),
            ],
            false,
            CLIENT_KEY,
        );
        lsb_decrypt_request(&mut frame);
        let entries = frame[SEARCH_REQUEST_ENTRIES_OFFSET..][..12].to_vec();
        assert_eq!(frame[REQUEST_TYPE_OFFSET], TCP_SEARCH);
        assert_eq!(frame[SEARCH_REQUEST_ENTRY_BYTES_OFFSET], 12);
        assert_eq!(bytes_to_hex(&entries), "023c5bf1110a631a50a33840");
    }

    #[test]
    fn search_list_frames_parse_the_addplayer_law() {
        // FIXTURE E: a single-frame body for two players (second hidden via
        // flags1 & 0x4000, carrying a seacom comment), packed by the utils.cpp port.
        let mut body = hex_bytes("00000000000000005c0080800000000000000000000000002001e2df8823389194c44b2529400cc00848123457000000005800000002e0006022054199bf7679d876e5b01000c8000800063700000000440000001ec0001bd5b70001");
        assert_eq!(body.len(), 0x5C);
        body[SEARCH_LIST_TOTAL_COUNT_OFFSET..][..2].copy_from_slice(&2u16.to_le_bytes());
        let frame = parse_search_list(&body).unwrap();
        assert!(frame.is_final);
        assert_eq!(frame.total_count, 2);
        assert_eq!(frame.players.len(), 2);
        let first = &frame.players[0];
        assert_eq!(first.name, "bob");
        assert_eq!(first.zone, 103);
        assert_eq!(first.nation, Some(1));
        assert_eq!((first.main_job, first.sub_job), (Some(5), Some(6)));
        assert_eq!((first.main_level, first.sub_level), (Some(75), Some(37)));
        assert_eq!((first.race, first.rank), (Some(2), Some(3)));
        assert_eq!(first.flags1, 0x42);
        assert_eq!(first.id, 0x1_2345);
        assert_eq!(first.comment_type, None);
        assert_eq!((first.flags2, first.languages), (0, 3));
        let second = &frame.players[1];
        assert_eq!(second.name, "ALongNameX");
        // The hidden shape drops nation/job/level/race/rank entries entirely.
        assert_eq!(second.nation, None);
        assert_eq!((second.main_job, second.main_level), (None, None));
        assert_eq!((second.race, second.rank), (None, None));
        assert_eq!(second.flags1, 0x4000);
        assert_eq!(second.id, 99);
        assert_eq!(second.comment_type, Some(7));
        assert_eq!((second.flags2, second.languages), (0xDEAD, 1));

        body[RESPONSE_FLAGS_OFFSET] = 0;
        let more = parse_search_list(&body).unwrap();
        assert!(!more.is_final, "final bit read through from [0x0A]");
    }

    #[test]
    fn zero_result_frame_parses_empty() {
        let mut body = vec![0u8; SEARCH_LIST_ENTRIES_BIT_OFFSET / 8];
        body[RESPONSE_TYPE_OFFSET] = SEARCH_LIST_RESPONSE_TYPE;
        body[RESPONSE_FLAGS_OFFSET] = RESPONSE_FINAL_FLAG;
        let frame = parse_search_list(&body).unwrap();
        assert!(frame.is_final);
        assert_eq!(frame.total_count, 0);
        assert!(frame.players.is_empty());
    }

    #[test]
    fn unknown_entry_type_is_a_hard_error_not_a_desync() {
        let mut body = vec![0u8; 40];
        body[RESPONSE_TYPE_OFFSET] = SEARCH_LIST_RESPONSE_TYPE;
        body[RESPONSE_BODY_SIZE_OFFSET..][..2].copy_from_slice(&30u16.to_le_bytes());
        // Two-byte entity region so the entry loop actually starts (min entry 9 bits).
        body[SEARCH_LIST_ENTRIES_BIT_OFFSET / 8] = 2;
        pack_bits_le(&mut body, 0x1F, SEARCH_LIST_ENTRIES_BIT_OFFSET + 8, 5);
        assert_eq!(
            parse_search_list(&body),
            Err(SearchError::UnknownSearchEntry(0x1F))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent reimplementation of SearchHandler::decrypt/encrypt/validatePacket
    // (vendor/server/src/search/search_handler.cpp SearchHandler::decrypt) so the client-side
    // codec is checked against the server algorithm, not against itself.
    struct LsbServer {
        key: [u8; 24],
    }

    impl LsbServer {
        fn new() -> Self {
            Self {
                key: SEARCH_BASE_KEY,
            }
        }

        fn cipher(&self, frame: &mut [u8], key_bytes: usize, decrypt: bool) {
            let hash = md5::md5(&self.key[..key_bytes]);
            let state = blowfish::State::new(&hash);
            let length = frame.len();
            let mut tmp = (length - 12) / 4;
            tmp -= tmp % 2;
            for i in (0..tmp).step_by(2) {
                let a = 4 * (i + 2);
                let b = 4 * (i + 3);
                let mut xl = u32::from_le_bytes(frame[a..a + 4].try_into().unwrap());
                let mut xr = u32::from_le_bytes(frame[b..b + 4].try_into().unwrap());
                if decrypt {
                    blowfish::decipher(&mut xl, &mut xr, &state.p, &state.s);
                } else {
                    blowfish::encipher(&mut xl, &mut xr, &state.p, &state.s);
                }
                frame[a..a + 4].copy_from_slice(&xl.to_le_bytes());
                frame[b..b + 4].copy_from_slice(&xr.to_le_bytes());
            }
        }

        fn decrypt(&mut self, frame: &mut [u8]) {
            let length = frame.len();
            let trailer: [u8; 4] = frame[length - 4..].try_into().unwrap();
            self.key[16..20].copy_from_slice(&trailer);
            self.cipher(frame, 20, true);
            let word: [u8; 4] = frame[length - 0x18..length - 0x18 + 4].try_into().unwrap();
            self.key[20..24].copy_from_slice(&word);
        }

        fn validate(&self, frame: &[u8]) -> bool {
            let length = frame.len();
            let to_hash = length - 0x08 - 0x10 - 0x04;
            let hash = md5::md5(&frame[8..8 + to_hash]);
            frame[length - 0x14..length - 0x14 + 16] == hash
        }

        fn encrypt(&self, frame: &mut [u8]) {
            let length = frame.len();
            frame[0..2].copy_from_slice(&(length as u16).to_le_bytes());
            frame[4..8].copy_from_slice(&0x46465849u32.to_le_bytes());
            let hash_off = length - 0x18 + 0x04;
            let body_hash = md5::md5(&frame[8..8 + (length - 0x18 - 0x04)]);
            frame[hash_off..hash_off + 16].copy_from_slice(&body_hash);
            self.cipher(frame, 24, false);
            let echo: [u8; 4] = self.key[16..20].try_into().unwrap();
            frame[length - 4..].copy_from_slice(&echo);
        }
    }

    const CLIENT_KEY: u32 = 0xA1B2_C3D4;

    #[test]
    fn ah_list_request_roundtrips_through_lsb_decrypt() {
        let mut client = SearchCrypto::new();
        let mut frame = client.encode_ah_list_request(7, &[SORT_LEVEL_DESC, SORT_NAME], CLIENT_KEY);
        assert_eq!(
            rd_u16(&frame, FRAME_LEN_OFFSET) as usize,
            frame.len(),
            "length field"
        );
        assert!(frame.len() >= MIN_FRAME_LEN);

        let mut server = LsbServer::new();
        server.decrypt(&mut frame);
        assert!(server.validate(&frame), "server-side MD5 validation");

        assert_eq!(frame[REQUEST_TYPE_OFFSET], TCP_AH_REQUEST);
        assert_eq!(frame[AH_LIST_SORT_COUNT_OFFSET], 2);
        assert_eq!(frame[AH_LIST_CATEGORY_OFFSET], 7);
        assert_eq!(rd_u32(&frame, AH_LIST_PARAMS_OFFSET), SORT_LEVEL_DESC);
        assert_eq!(
            rd_u32(&frame, AH_LIST_PARAMS_OFFSET + AH_LIST_PARAM_STRIDE),
            SORT_NAME
        );
        assert_eq!(rd_u32(&frame, MAGIC_OFFSET), MAGIC_IXFF);
        assert_eq!(
            server.key, client.key,
            "key evolution must stay in lockstep"
        );
    }

    /// The follow-up page request differs from the first only in its type byte:
    /// HandleAuctionHouseRequest reads category/sorts from the same offsets for
    /// both (search_handler.cpp SearchHandler::read_func TCP_AH_REQUEST).
    #[test]
    fn ah_list_more_request_repeats_the_body_under_the_more_type() {
        let mut client = SearchCrypto::new();
        let first = client.encode_ah_list_request(18, &[SORT_NAME], CLIENT_KEY);
        let mut more = client.encode_ah_list_more_request(18, &[SORT_NAME], CLIENT_KEY);
        assert_eq!(
            more.len(),
            first.len(),
            "same payload shape, so the same frame length"
        );

        let mut server = LsbServer::new();
        server.decrypt(&mut more);
        assert!(server.validate(&more), "server-side MD5 validation");
        assert_eq!(more[REQUEST_TYPE_OFFSET], TCP_AH_REQUEST_MORE);
        assert_eq!(more[AH_LIST_SORT_COUNT_OFFSET], 1);
        assert_eq!(more[AH_LIST_CATEGORY_OFFSET], 18);
        assert_eq!(rd_u32(&more, AH_LIST_PARAMS_OFFSET), SORT_NAME);
    }

    /// LSB evolves `SearchHandler::key` in place across a connection
    /// (`vendor/server/src/search/search_handler.cpp` `SearchHandler::decrypt`);
    /// splicing a fresh key into a follow-up page made the next response fail
    /// the client-side integrity hash check (`SearchError::BadHash` here;
    /// `.agents/skills/retail-observe/references/auction-house.md`,
    /// "One client key per connection").
    #[test]
    fn repeated_requests_under_one_client_key_hold_key_state_steady() {
        let mut client = SearchCrypto::new();
        let mut server = LsbServer::new();

        let mut first = client.encode_ah_list_request(18, &[], CLIENT_KEY);
        server.decrypt(&mut first);
        assert!(server.validate(&first));
        let after_first = client.key;

        let mut more = client.encode_ah_list_more_request(18, &[], CLIENT_KEY);
        server.decrypt(&mut more);
        assert!(server.validate(&more));
        assert_eq!(
            client.key, after_first,
            "key unchanged across the page pull"
        );
        assert_eq!(server.key, client.key, "and still in lockstep with LSB");
    }

    #[test]
    fn ah_history_request_roundtrips_through_lsb_decrypt() {
        for (stack, want_type) in [(false, TCP_AH_HISTORY_SINGLE), (true, TCP_AH_HISTORY_STACK)] {
            let mut client = SearchCrypto::new();
            let mut frame = client.encode_ah_history_request(4096, stack, CLIENT_KEY);

            let mut server = LsbServer::new();
            server.decrypt(&mut frame);
            assert!(server.validate(&frame));

            assert_eq!(frame[REQUEST_TYPE_OFFSET], want_type);
            assert_eq!(rd_u16(&frame, AH_HISTORY_ITEM_ID_OFFSET), 4096);
            assert_eq!(frame[AH_HISTORY_STACK_OFFSET], stack as u8);
            assert_eq!(server.key, client.key);
        }
    }

    /// The `len-0x18` seed word sits inside the MD5-hashed span — the subtlest
    /// part of the framing (a hash starting after it would still roundtrip).
    #[test]
    fn seed_word_is_inside_hash_coverage() {
        let mut client = SearchCrypto::new();
        let mut frame = client.encode_ah_list_request(1, &[], CLIENT_KEY);
        let seed_off = frame.len() - SERVER_KEY_FROM_END;
        let mut server = LsbServer::new();
        server.decrypt(&mut frame);
        assert!(server.validate(&frame));
        frame[seed_off] ^= 0xFF;
        assert!(!server.validate(&frame));
    }

    #[test]
    fn tampered_request_fails_lsb_validation() {
        let mut client = SearchCrypto::new();
        let mut frame = client.encode_ah_list_request(1, &[], CLIENT_KEY);
        frame[AH_LIST_CATEGORY_OFFSET] ^= 0xFF;
        let mut server = LsbServer::new();
        server.decrypt(&mut frame);
        assert!(!server.validate(&frame));
    }

    // Response body per auction_list.cpp: total size = body-size field + 28
    // (CAHItemsListPacket::GetSize, auction_list.cpp).
    fn build_list_response(
        total: u16,
        offset: usize,
        items: &[(u16, u32, u32)],
        is_final: bool,
    ) -> Vec<u8> {
        assert!(items.len() <= AH_LIST_ITEMS_PER_PACKET);
        let body_size = AH_LIST_ITEMS_OFFSET + AH_LIST_ITEM_SIZE * items.len();
        let mut frame = vec![0u8; body_size + 28];
        frame[RESPONSE_TYPE_OFFSET] = AH_LIST_RESPONSE_TYPE;
        frame[AH_LIST_TOTAL_COUNT_OFFSET..AH_LIST_TOTAL_COUNT_OFFSET + 2]
            .copy_from_slice(&total.to_le_bytes());
        if is_final {
            assert_eq!(total as usize - offset, items.len());
            frame[RESPONSE_FLAGS_OFFSET] = RESPONSE_FINAL_FLAG;
        }
        frame[RESPONSE_BODY_SIZE_OFFSET..RESPONSE_BODY_SIZE_OFFSET + 2]
            .copy_from_slice(&(body_size as u16).to_le_bytes());
        for (i, &(id, single, stack)) in items.iter().enumerate() {
            let off = AH_LIST_ITEMS_OFFSET + AH_LIST_ITEM_SIZE * i;
            frame[off..off + 2].copy_from_slice(&id.to_le_bytes());
            frame[off + 2..off + 6].copy_from_slice(&single.to_le_bytes());
            frame[off + 6..off + 10].copy_from_slice(&stack.to_le_bytes());
        }
        frame
    }

    #[test]
    fn encrypted_list_response_roundtrips_and_parses() {
        let mut client = SearchCrypto::new();
        let mut request = client.encode_ah_list_request(2, &[SORT_NAME], CLIENT_KEY);
        let mut server = LsbServer::new();
        server.decrypt(&mut request);
        assert!(server.validate(&request));

        let items = [(4096u16, 120u32, 1100u32), (4097, 0, 900)];
        let mut response = build_list_response(2, 0, &items, true);
        server.encrypt(&mut response);

        let body = client.decrypt_response(&response).expect("decrypt");
        let page = parse_ah_list(&body).expect("parse");
        assert_eq!(page.total_count, 2);
        assert!(page.is_final);
        assert_eq!(
            page.listings,
            vec![
                AhListing {
                    item_id: 4096,
                    singles_for_sale: 120,
                    stacks_for_sale: Some(1100)
                },
                AhListing {
                    item_id: 4097,
                    singles_for_sale: 0,
                    stacks_for_sale: Some(900)
                },
            ]
        );
    }

    #[test]
    fn tampered_response_fails_integrity_check() {
        let mut client = SearchCrypto::new();
        let mut request = client.encode_ah_history_request(640, false, CLIENT_KEY);
        let mut server = LsbServer::new();
        server.decrypt(&mut request);

        let mut response = build_list_response(1, 0, &[(640, 10, 0)], true);
        server.encrypt(&mut response);
        response[AH_LIST_ITEMS_OFFSET] ^= 0x01;
        assert_eq!(
            client.decrypt_response(&response),
            Err(SearchError::BadHash)
        );
    }

    #[test]
    fn stale_key_state_cannot_decrypt_response() {
        let mut client = SearchCrypto::new();
        let mut request = client.encode_ah_list_request(2, &[], CLIENT_KEY);
        let mut server = LsbServer::new();
        server.decrypt(&mut request);

        let mut response = build_list_response(0, 0, &[], true);
        server.encrypt(&mut response);

        let fresh = SearchCrypto::new();
        assert!(fresh.decrypt_response(&response).is_err());
        assert!(client.decrypt_response(&response).is_ok());
    }

    #[test]
    fn multi_packet_list_final_marker() {
        let total: u16 = 25;
        let page1_items: Vec<(u16, u32, u32)> =
            (0..20).map(|i| (100 + i as u16, i * 10, i * 11)).collect();
        let page1 = build_list_response(total, 0, &page1_items, false);
        let parsed1 = parse_ah_list(&page1).unwrap();
        assert_eq!(parsed1.total_count, 25);
        assert!(!parsed1.is_final);
        assert_eq!(parsed1.listings.len(), AH_LIST_ITEMS_PER_PACKET);
        assert_eq!(
            rd_u16(&page1, RESPONSE_BODY_SIZE_OFFSET) as usize,
            AH_LIST_ITEMS_OFFSET + AH_LIST_ITEM_SIZE * AH_LIST_ITEMS_PER_PACKET
        );

        let page2_items: Vec<(u16, u32, u32)> =
            (20..25).map(|i| (100 + i as u16, i * 10, i * 11)).collect();
        let page2 = build_list_response(total, 20, &page2_items, true);
        let parsed2 = parse_ah_list(&page2).unwrap();
        assert!(parsed2.is_final);
        assert_eq!(parsed2.listings.len(), 5);
        assert_eq!(parsed2.listings[4].item_id, 124);

        let empty = build_list_response(0, 0, &[], true);
        let parsed = parse_ah_list(&empty).unwrap();
        assert_eq!(parsed.total_count, 0);
        assert!(parsed.is_final);
        assert!(parsed.listings.is_empty());
    }

    fn build_history_response(
        item_id: u16,
        price: u32,
        category: u16,
        rows: &[(u32, u32, &str, &str)],
    ) -> Vec<u8> {
        assert!(rows.len() <= AH_HISTORY_MAX_ROWS);
        let body_size = AH_HISTORY_ROWS_OFFSET + AH_HISTORY_ROW_SIZE * rows.len();
        let mut frame = vec![0u8; body_size + 28];
        frame[RESPONSE_FLAGS_OFFSET] = RESPONSE_FINAL_FLAG;
        frame[RESPONSE_TYPE_OFFSET] = AH_HISTORY_RESPONSE_TYPE;
        frame[0x10..0x12].copy_from_slice(&item_id.to_le_bytes());
        frame[AH_HISTORY_ITEM_OFFSET..AH_HISTORY_ITEM_OFFSET + 2]
            .copy_from_slice(&item_id.to_le_bytes());
        frame[AH_HISTORY_OPEN_LISTINGS_OFFSET..AH_HISTORY_OPEN_LISTINGS_OFFSET + 4]
            .copy_from_slice(&price.to_le_bytes());
        frame[AH_HISTORY_CATEGORY_OFFSET..AH_HISTORY_CATEGORY_OFFSET + 2]
            .copy_from_slice(&category.to_le_bytes());
        if !rows.is_empty() {
            frame[RESPONSE_BODY_SIZE_OFFSET..RESPONSE_BODY_SIZE_OFFSET + 2]
                .copy_from_slice(&(body_size as u16).to_le_bytes());
        }
        for (i, &(sale, date, seller, buyer)) in rows.iter().enumerate() {
            let off = AH_HISTORY_ROWS_OFFSET + AH_HISTORY_ROW_SIZE * i;
            frame[off..off + 4].copy_from_slice(&sale.to_le_bytes());
            frame[off + 4..off + 8].copy_from_slice(&date.to_le_bytes());
            frame[off + AH_HISTORY_ROW_SELLER_OFFSET..][..seller.len()]
                .copy_from_slice(seller.as_bytes());
            frame[off + AH_HISTORY_ROW_BUYER_OFFSET..][..buyer.len()]
                .copy_from_slice(buyer.as_bytes());
        }
        frame
    }

    #[test]
    fn history_response_parses() {
        let rows = [
            (1180u32, 1_754_000_000u32, "Sellera", "Buyerb"),
            (2670, 1_754_100_000, "Atti", "Verilight"),
        ];
        let frame = build_history_response(4096, 120, 12, &rows);
        let history = parse_ah_history(&frame).unwrap();
        assert_eq!(history.item_id, 4096);
        assert_eq!(history.open_listings, 120);
        assert_eq!(history.category, 12);
        assert_eq!(history.sales.len(), 2);
        assert_eq!(
            history.sales[0],
            AhSale {
                price: 1180,
                sell_date: 1_754_000_000,
                seller: "Sellera".into(),
                buyer: "Buyerb".into(),
            }
        );
        assert_eq!(history.sales[1].buyer, "Verilight");
    }

    #[test]
    fn history_response_with_no_sales_parses_empty() {
        let frame = build_history_response(4096, 0, 12, &[]);
        let history = parse_ah_history(&frame).unwrap();
        assert!(history.sales.is_empty());
        assert_eq!(history.open_listings, 0);
    }

    #[test]
    fn encrypted_history_response_roundtrips() {
        let mut client = SearchCrypto::new();
        let mut request = client.encode_ah_history_request(4096, true, CLIENT_KEY);
        let mut server = LsbServer::new();
        server.decrypt(&mut request);
        assert!(server.validate(&request));

        let mut response =
            build_history_response(4096, 1100, 12, &[(1100, 1_754_000_000, "Atti", "Buyerb")]);
        server.encrypt(&mut response);

        let body = client.decrypt_response(&response).expect("decrypt");
        let history = parse_ah_history(&body).expect("parse");
        assert_eq!(history.item_id, 4096);
        assert_eq!(history.sales.len(), 1);
        assert_eq!(history.sales[0].seller, "Atti");
    }

    #[test]
    fn scraped_key_and_request_types_have_expected_shape() {
        assert_eq!(SEARCH_BASE_KEY.len(), FULL_KEY_LEN);
        assert_eq!(&SEARCH_BASE_KEY[CLIENT_KEY_SPLICE..], &[0u8; 8]);
        assert_ne!(SEARCH_BASE_KEY[0], 0);
        assert_ne!(TCP_AH_REQUEST, TCP_AH_REQUEST_MORE);
        assert_ne!(TCP_AH_HISTORY_SINGLE, TCP_AH_HISTORY_STACK);
        // Retail's response type is the request type with the top bit set
        // (auction_list.cpp m_offset vs TCP_AH_REQUEST; auction_history.cpp CAHHistoryPacket::CAHHistoryPacket vs
        // TCP_AH_HISTORY_SINGLE, "masked as val & 0x1F" per atom0s).
        assert_eq!(
            AH_LIST_RESPONSE_TYPE & AH_RESPONSE_TYPE_REQUEST_MASK,
            TCP_AH_REQUEST
        );
        assert_eq!(
            AH_HISTORY_RESPONSE_TYPE & AH_RESPONSE_TYPE_REQUEST_MASK,
            TCP_AH_HISTORY_SINGLE
        );
    }
}
