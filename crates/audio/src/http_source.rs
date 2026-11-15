//! A seekable symphonia `MediaSource` over HTTP Range requests.
//!
//! Reads are served from an in-memory cache of 64 KiB aligned blocks. Misses
//! continue the current sequential response when the gap is small and
//! otherwise open a new `Range: bytes=<block>-` request. Containers that seek
//! for their index (MP4 `moov` at the tail, FLAC seek tables) therefore cost
//! one extra request instead of a full download.
//!
//! The URL carries Subsonic credentials (`t=`, `s=`), so it is never logged.
//! Errors use [`crate::redact_url`].

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use symphonia::core::io::MediaSource;

const BLOCK: u64 = 64 * 1024;
/// Gap (in blocks) worth reading through instead of opening a new request.
const MAX_SKIP_BLOCKS: u64 = 8;

struct Inner {
    client: Client,
    url: String,
    len: Option<u64>,
    pos: u64,
    blocks: HashMap<u64, Box<[u8]>>,
    stream: Option<Response>,
    /// Absolute byte offset of the next byte `stream` will yield.
    stream_pos: u64,
    range_supported: bool,
}

pub struct HttpSource {
    inner: Mutex<Inner>,
}

pub fn http_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(None)
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

fn open(client: &Client, url: &str, from: u64) -> io::Result<(Response, Option<u64>, bool)> {
    let response = client
        .get(url)
        .header(RANGE, format!("bytes={from}-"))
        .send()
        .map_err(|e| io::Error::other(format!("stream request failed: {}", crate::redact_error(&e))))?;
    let status = response.status();
    if !status.is_success() {
        return Err(io::Error::other(format!("stream returned HTTP {}", status.as_u16())));
    }
    let partial = status.as_u16() == 206;
    let total = if partial {
        response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse::<u64>().ok())
    } else {
        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
    };
    Ok((response, total, partial))
}

impl HttpSource {
    pub fn open(client: Client, url: String) -> io::Result<Self> {
        let (stream, len, partial) = open(&client, &url, 0)?;
        Ok(Self {
            inner: Mutex::new(Inner {
                client,
                url,
                len,
                pos: 0,
                blocks: HashMap::new(),
                stream: Some(stream),
                stream_pos: 0,
                range_supported: partial,
            }),
        })
    }
}

impl Inner {
    /// Reads the next whole block from the current stream into the cache.
    fn pull_block(&mut self) -> io::Result<bool> {
        let index = self.stream_pos / BLOCK;
        let Some(stream) = self.stream.as_mut() else {
            return Ok(false);
        };
        let mut block = Vec::with_capacity(BLOCK as usize);
        let mut limited = stream.take(BLOCK);
        limited.read_to_end(&mut block)?;
        if block.is_empty() {
            self.stream = None;
            if self.len.is_none() {
                self.len = Some(self.stream_pos);
            }
            return Ok(false);
        }
        self.stream_pos += block.len() as u64;
        let short = (block.len() as u64) < BLOCK;
        self.blocks.insert(index, block.into_boxed_slice());
        if short {
            self.stream = None;
            if self.len.is_none() {
                self.len = Some(self.stream_pos);
            }
        }
        Ok(true)
    }

    fn ensure_block(&mut self, index: u64) -> io::Result<bool> {
        if self.blocks.contains_key(&index) {
            return Ok(true);
        }
        let start = index * BLOCK;
        let stream_block = self.stream_pos / BLOCK;
        let near = self.stream.is_some()
            && self.stream_pos.is_multiple_of(BLOCK)
            && stream_block <= index
            && index - stream_block <= MAX_SKIP_BLOCKS;
        if !near {
            if !self.range_supported && start < self.stream_pos {
                // No Range support: restart from zero and read forward.
                let (stream, _, _) = open(&self.client, &self.url, 0)?;
                self.stream = Some(stream);
                self.stream_pos = 0;
            } else if self.range_supported {
                let (stream, total, partial) = open(&self.client, &self.url, start)?;
                if !partial {
                    // Server ignored Range for this request; fall back to sequential.
                    self.range_supported = false;
                    self.stream = Some(stream);
                    self.stream_pos = 0;
                } else {
                    if self.len.is_none() {
                        self.len = total;
                    }
                    self.stream = Some(stream);
                    self.stream_pos = start;
                }
            }
        }
        while self.stream_pos / BLOCK <= index {
            if !self.pull_block()? {
                break;
            }
        }
        Ok(self.blocks.contains_key(&index))
    }
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let inner = self.inner.get_mut().map_err(|_| io::Error::other("poisoned"))?;
        if buf.is_empty() {
            return Ok(0);
        }
        if inner.len.is_some_and(|len| inner.pos >= len) {
            return Ok(0);
        }
        let index = inner.pos / BLOCK;
        if !inner.ensure_block(index)? {
            return Ok(0);
        }
        let block = &inner.blocks[&index];
        let offset = (inner.pos % BLOCK) as usize;
        if offset >= block.len() {
            return Ok(0);
        }
        let count = buf.len().min(block.len() - offset);
        buf[..count].copy_from_slice(&block[offset..offset + count]);
        inner.pos += count as u64;
        Ok(count)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let inner = self.inner.get_mut().map_err(|_| io::Error::other("poisoned"))?;
        let next = match to {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => inner.pos as i128 + delta as i128,
            SeekFrom::End(delta) => {
                let len = inner
                    .len
                    .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unknown length"))?;
                len as i128 + delta as i128
            }
        };
        if next < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "negative seek"));
        }
        inner.pos = next as u64;
        Ok(inner.pos)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        self.inner.lock().map(|i| i.len.is_some()).unwrap_or(false)
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.lock().ok().and_then(|i| i.len)
    }
}
