//! CONTRACTS.md 1.2, 2.1-2.3: R2 key layout, pack entry codec, SQLite index, MemFind,
//! Bucket (coalesced range reads), PackWriter / RawWriter (multipart), Index.

use std::collections::HashMap;

use gix_hash::ObjectId;
use gix_object::Kind;
use serde::{Deserialize, Serialize};
use worker::{MultipartUpload, SqlStorage, SqlStorageValue as V, UploadedPart};

use crate::error::Error;
use crate::platform;
use crate::ReqBudget;

pub const PART: usize = 8 << 20; // 6.4: every part exactly 8 MiB except the last
pub const MAX_OBJECT: u64 = 16 << 20; // A7: single-object cap, inflated

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RepoId(pub String);
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackId(pub String);
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PushId(pub String);

impl PackId {
    pub fn random() -> Result<Self, Error> {
        Ok(Self(platform::hex16()?))
    }
}
impl PushId {
    pub fn random() -> Result<Self, Error> {
        Ok(Self(platform::hex16()?))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ObjLoc {
    pub pack: PackId,
    pub idx: u32,
    pub offset: u64,
    pub len: u32,
    #[serde(with = "kind_serde")]
    pub kind: Kind,
    pub size: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ObjRow {
    pub sha: ObjectId,
    pub idx: u32,
    pub offset: u64,
    pub len: u32,
    #[serde(with = "kind_serde")]
    pub kind: Kind,
    pub size: u64,
}

mod kind_serde {
    use gix_object::Kind;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(k: &Kind, s: S) -> Result<S::Ok, S::Error> {
        let n: u8 = match k {
            Kind::Commit => 1,
            Kind::Tree => 2,
            Kind::Blob => 3,
            Kind::Tag => 4,
        };
        n.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Kind, D::Error> {
        match u8::deserialize(d)? {
            1 => Ok(Kind::Commit),
            2 => Ok(Kind::Tree),
            3 => Ok(Kind::Blob),
            4 => Ok(Kind::Tag),
            _ => Err(serde::de::Error::custom("bad kind")),
        }
    }
}

pub fn git_kind(k: Kind) -> u8 {
    match k {
        Kind::Commit => 1,
        Kind::Tree => 2,
        Kind::Blob => 3,
        Kind::Tag => 4,
    }
}
pub fn kind_of(k: u8) -> Result<Kind, Error> {
    match k {
        1 => Ok(Kind::Commit),
        2 => Ok(Kind::Tree),
        3 => Ok(Kind::Blob),
        4 => Ok(Kind::Tag),
        k => Err(Error::Internal(format!("bad kind {k} in objects"))),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PackState {
    Ingesting,
    Live,
    Dead,
}
impl PackState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackState::Ingesting => "ingesting",
            PackState::Live => "live",
            PackState::Dead => "dead",
        }
    }
}

#[derive(Clone)]
pub struct PackMeta {
    pub pack: PackId,
    pub push_id: Option<String>,
    pub count: u32,
    pub bytes: u64,
    pub commit_lo: u64,
    pub commit_hi: u64,
    pub created_at: i64,
}
impl PackMeta {
    /// Posted before the first part is uploaded so a completed pack never lacks a row (two-phase-push).
    pub const EMPTY: PackMeta = PackMeta {
        pack: PackId(String::new()),
        push_id: None,
        count: 0,
        bytes: 0,
        commit_lo: u64::MAX,
        commit_hi: 0,
        created_at: 0,
    };
}

pub mod keys {
    use super::{PackId, PushId, RepoId};
    pub fn pack(repo: &RepoId, pack: &PackId) -> String {
        format!("r/{}/packs/{}.pack", repo.0, pack.0)
    }
    pub fn pending(repo: &RepoId, push: &PushId) -> String {
        format!("r/{}/pending/{}.pack", repo.0, push.0)
    }
}

/// Sync, no `worker` imports (1.2).
pub mod codec {
    use gix_hash::Kind as HashKind;
    use gix_object::Kind;
    use gix_pack::data::{entry::Header, Entry};
    use std::io::Write;

    use super::MAX_OBJECT;
    use crate::error::Error;

    /// One full (non-delta) entry: varint(kind,size) + zlib(data) (2.1).
    pub fn encode_entry(kind: Kind, data: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        let hdr = match kind {
            Kind::Commit => Header::Commit,
            Kind::Tree => Header::Tree,
            Kind::Blob => Header::Blob,
            Kind::Tag => Header::Tag,
        };
        let size = u64::try_from(data.len()).map_err(|_| Error::Internal("size".into()))?;
        hdr.write_to(size, out).map_err(|e| Error::Internal(format!("entry header: {e}")))?;
        let mut z = gix_zlib::stream::deflate::Write::new(out, gix_zlib::Compression::DEFAULT);
        z.write_all(data)
            .and_then(|_| std::io::Write::flush(&mut z))
            .map(|_| ())
            .map_err(|e| Error::Internal(format!("deflate: {e}")))
    }
    /// (kind, inflated size, header length) of the entry at bytes[0]. A delta header breaks the
    /// normalized-pack invariant: Error::Internal, not a client error (section 10).
    pub fn entry_header(bytes: &[u8]) -> Result<(Kind, u64, usize), Error> {
        let e = Entry::from_bytes(bytes, 0, HashKind::Sha1)
            .map_err(|e| Error::Storage(format!("entry: {e}")))?;
        let kind = e
            .header
            .as_kind()
            .ok_or_else(|| Error::Internal("delta entry in a normalized pack".into()))?;
        Ok((kind, e.decompressed_size, e.header_size()))
    }
    pub fn decode_entry(bytes: &[u8]) -> Result<(Kind, Vec<u8>), Error> {
        let (kind, size, hlen) = entry_header(bytes)?;
        if size > MAX_OBJECT {
            return Err(Error::Limit("object too large (16 MiB max)".into()));
        }
        let z = bytes.get(hlen..).ok_or_else(|| Error::Storage("truncated entry".into()))?;
        let mut out = vec![0u8; usize::try_from(size).map_err(|_| Error::Limit("size".into()))?];
        let (status, _in, n) = gix_zlib::Inflate::default()
            .once(z, &mut out)
            .map_err(|e| Error::Storage(format!("inflate: {e}")))?;
        if status != gix_zlib::Status::StreamEnd || n != out.len() {
            return Err(Error::Storage("entry size does not match its header".into()));
        }
        Ok((kind, out))
    }
}

#[derive(Default)]
pub struct MemFind {
    objs: HashMap<ObjectId, (Kind, Vec<u8>)>,
    pub bytes: usize,
}
impl MemFind {
    pub fn insert(&mut self, id: ObjectId, kind: Kind, data: Vec<u8>) {
        self.bytes = self.bytes.saturating_add(data.len());
        self.objs.insert(id, (kind, data));
    }
    pub fn clear(&mut self) {
        self.objs.clear();
        self.bytes = 0;
    }
    pub fn get(&self, id: &ObjectId) -> Option<(Kind, &[u8])> {
        self.objs.get(id).map(|(k, d)| (*k, d.as_slice()))
    }
    pub fn exists(&self, id: &ObjectId) -> bool {
        self.objs.contains_key(id)
    }
}
impl gix_object::Find for MemFind {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> Result<Option<gix_object::Data<'a>>, gix_object::find::Error> {
        match self.objs.get(id) {
            Some((kind, data)) => {
                buffer.clear();
                buffer.extend_from_slice(data);
                Ok(Some(gix_object::Data {
                    kind: *kind,
                    object_hash: gix_hash::Kind::Sha1,
                    data: buffer.as_slice(),
                }))
            }
            None => Ok(None),
        }
    }
}
impl gix_object::Exists for MemFind {
    fn exists(&self, id: &gix_hash::oid) -> bool {
        self.objs.contains_key(id)
    }
}

pub struct Bucket {
    pub inner: worker::Bucket,
    pub repo: RepoId,
}
impl Bucket {
    pub fn new(inner: worker::Bucket, repo: RepoId) -> Self {
        Self { inner, repo }
    }
    /// One subrequest (7.1).
    pub async fn read_range(
        &self,
        key: &str,
        offset: u64,
        len: u64,
        budget: &mut ReqBudget,
    ) -> Result<Vec<u8>, Error> {
        budget.charge(1)?;
        let obj = self
            .inner
            .get(key)
            .range(worker::Range::OffsetWithLength { offset, length: len })
            .execute()
            .await?
            .ok_or_else(|| Error::Storage(format!("missing {key}")))?;
        let bytes = obj
            .body()
            .ok_or_else(|| Error::Storage(format!("no body for {key}")))?
            .bytes()
            .await?;
        if u64::try_from(bytes.len()).ok() != Some(len) {
            return Err(Error::Storage(format!("short read on {key}")));
        }
        Ok(bytes)
    }
    /// Coalesced reads (7.2): sort by (pack, offset), merge gaps < 256 KiB, split at 8 MiB.
    pub async fn read_entries(
        &self,
        locs: &[(ObjectId, ObjLoc)],
        budget: &mut ReqBudget,
    ) -> Result<Vec<(ObjectId, Vec<u8>)>, Error> {
        const GAP: u64 = 256 * 1024;
        const SPAN: u64 = 8 << 20;
        const MAX_READ: u64 = 48 << 20; // a single call must fit inside the isolate
        let mut sorted: Vec<&(ObjectId, ObjLoc)> = locs.iter().collect();
        sorted.sort_by(|a, b| (&a.1.pack.0, a.1.offset).cmp(&(&b.1.pack.0, b.1.offset)));
        let (mut out, mut i, mut total) = (Vec::with_capacity(locs.len()), 0usize, 0u64);
        while let Some(first) = sorted.get(i) {
            let (start, mut end, mut j) =
                (first.1.offset, first.1.offset.saturating_add(u64::from(first.1.len)), i + 1);
            while let Some(n) = sorted.get(j) {
                let n_end = n.1.offset.saturating_add(u64::from(n.1.len));
                if n.1.pack.0 != first.1.pack.0
                    || n.1.offset.saturating_sub(end) >= GAP
                    || n_end.saturating_sub(start) > SPAN
                {
                    break;
                }
                end = end.max(n_end);
                j += 1;
            }
            total = total.saturating_add(end.saturating_sub(start));
            if total > MAX_READ {
                return Err(Error::Limit("read batch exceeds memory budget".into()));
            }
            let key = keys::pack(&self.repo, &first.1.pack);
            let bytes = self.read_range(&key, start, end.saturating_sub(start), budget).await?;
            for e in sorted.iter().take(j).skip(i) {
                let lo = usize::try_from(e.1.offset.saturating_sub(start))
                    .map_err(|_| Error::Internal("offset".into()))?;
                let hi = lo
                    .checked_add(usize::try_from(e.1.len).map_err(|_| Error::Internal("len".into()))?)
                    .ok_or_else(|| Error::Internal("len overflow".into()))?;
                let entry = bytes
                    .get(lo..hi)
                    .ok_or_else(|| Error::Storage(format!("entry outside span in {key}")))?;
                out.push((e.0, entry.to_vec()));
            }
            i = j;
        }
        Ok(out)
    }
    /// <= 1000 keys per call (R2 delete_multiple).
    pub async fn delete(&self, keys: &[String]) -> Result<(), Error> {
        for chunk in keys.chunks(1_000) {
            let v: Vec<&str> = chunk.iter().map(String::as_str).collect();
            self.inner.delete_multiple(v).await?;
        }
        Ok(())
    }
    /// Raw multipart put, no pack framing (used by pass A for `pending/`).
    pub async fn create_multipart_upload(&self, key: &str) -> Result<MultipartUpload, Error> {
        Ok(self.inner.create_multipart_upload(key).execute().await?)
    }
}

/// Raw byte writer over one multipart upload (no pack header, no trailer hasher).
/// Used by pass A for `pending/<push>.pack`: bytes exactly as received.
pub struct RawWriter {
    mpu: MultipartUpload,
    part: Vec<u8>,
    parts: Vec<UploadedPart>,
}
impl RawWriter {
    pub async fn create(bucket: &Bucket, key: String, budget: &mut ReqBudget) -> Result<Self, Error> {
        budget.charge(1)?;
        let mpu = bucket.inner.create_multipart_upload(&key).execute().await?;
        Ok(Self { mpu, part: Vec::new(), parts: Vec::new() })
    }
    pub fn append(&mut self, bytes: &[u8]) {
        self.part.extend_from_slice(bytes);
    }
    pub async fn flush_if_full(&mut self, budget: &mut ReqBudget) -> Result<(), Error> {
        while self.part.len() >= PART {
            let chunk: Vec<u8> = self.part.drain(..PART).collect();
            let n = u16::try_from(self.parts.len().saturating_add(1))
                .map_err(|_| Error::Limit("too many parts".into()))?;
            budget.charge(1)?;
            self.parts.push(self.mpu.upload_part(n, chunk).await?);
        }
        Ok(())
    }
    pub async fn finish(mut self, budget: &mut ReqBudget) -> Result<(), Error> {
        if let Err(e) = self.finish_inner(budget).await {
            // a dropped MultipartUpload leaves the upload and its parts orphaned in
            // R2 — abort on any mid-finish failure. A failed complete() consumes the
            // handle; that residual upload expires server-side.
            let _ = self.mpu.abort().await;
            return Err(e);
        }
        budget.charge(1)?;
        self.mpu
            .complete(std::mem::take(&mut self.parts))
            .await
            .map(|_| ())
            .map_err(Error::from)
    }
    async fn finish_inner(&mut self, budget: &mut ReqBudget) -> Result<(), Error> {
        if !self.part.is_empty() {
            let last = std::mem::take(&mut self.part);
            let n = u16::try_from(self.parts.len().saturating_add(1))
                .map_err(|_| Error::Limit("too many parts".into()))?;
            budget.charge(1)?;
            self.parts.push(self.mpu.upload_part(n, last).await?);
        }
        Ok(())
    }
    pub async fn abort(self) {
        let _ = self.mpu.abort().await;
    }
}

/// One normalized pack: one multipart upload, running SHA-1 for the trailer (1.2, 6.4).
pub struct PackWriter {
    mpu: MultipartUpload,
    pack: PackId,
    part: Vec<u8>,
    parts: Vec<UploadedPart>,
    offset: u64,
    count: u32,
    expected: u32,
    sha1: CkptSha1,
    commit_lo: u64,
    commit_hi: u64,
    created_at: i64,
}
impl PackWriter {
    /// `expected` is pass A's verified entry count (2.4); the header is final from byte 0.
    pub async fn create(
        bucket: &Bucket,
        key: &str,
        expected: u32,
        budget: &mut ReqBudget,
    ) -> Result<Self, Error> {
        let name = key
            .rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".pack"))
            .ok_or_else(|| Error::Internal(format!("not a pack key: {key}")))?;
        let created_at = platform::now_ms();
        let meta: HashMap<String, String> = [
            ("repo", bucket.repo.0.clone()),
            ("pack", name.to_string()),
            ("count", expected.to_string()),
            ("created_at", created_at.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        budget.charge(1)?;
        let mpu = bucket.inner.create_multipart_upload(key).custom_metadata(meta).execute().await?;
        let header = gix_pack::data::header::encode(gix_pack::data::Version::V2, expected);
        let mut sha1 = CkptSha1::new();
        sha1.update(&header);
        Ok(Self {
            mpu,
            pack: PackId(name.to_string()),
            part: header.to_vec(),
            parts: Vec::new(),
            offset: 12,
            count: 0,
            expected,
            sha1,
            commit_lo: u64::MAX,
            commit_hi: 0,
            created_at,
        })
    }
    /// Sync; buffers. Returns (offset, len): the `objects` row and the range a reader asks for.
    pub fn append_entry(&mut self, kind: Kind, data: &[u8]) -> Result<(u64, u32), Error> {
        let start = self.part.len();
        codec::encode_entry(kind, data, &mut self.part)?;
        let entry = self.part.get(start..).ok_or_else(|| Error::Internal("part buffer".into()))?;
        let len = u32::try_from(entry.len()).map_err(|_| Error::Limit("entry too long".into()))?;
        self.sha1.update(entry);
        let offset = self.offset;
        self.offset = offset.saturating_add(u64::from(len));
        if kind == Kind::Commit {
            self.commit_lo = self.commit_lo.min(offset);
            self.commit_hi = self.offset;
        }
        self.count = self.count.saturating_add(1);
        Ok((offset, len))
    }
    /// Uploads whole 8 MiB parts while the buffer holds that much.
    pub async fn flush_if_full(&mut self, budget: &mut ReqBudget) -> Result<(), Error> {
        while self.part.len() >= PART {
            let chunk: Vec<u8> = self.part.drain(..PART).collect();
            let n = u16::try_from(self.parts.len().saturating_add(1))
                .map_err(|_| Error::Limit("too many parts".into()))?;
            budget.charge(1)?;
            self.parts.push(self.mpu.upload_part(n, chunk).await?);
        }
        Ok(())
    }
    /// Trailer, last part, complete. When this returns the pack is durable: section 3 step 1.
    pub async fn finish(mut self, budget: &mut ReqBudget) -> Result<PackMeta, Error> {
        if let Err(e) = self.finish_inner(budget).await {
            // a dropped MultipartUpload leaves the upload and its parts orphaned in
            // R2 — abort on any mid-finish failure. A failed complete() consumes the
            // handle; that residual upload expires server-side.
            let _ = self.mpu.abort().await;
            return Err(e);
        }
        let bytes = self.offset.saturating_add(20);
        budget.charge(1)?;
        let obj = self
            .mpu
            .complete(std::mem::take(&mut self.parts))
            .await
            .map_err(Error::from)?;
        if obj.size() != bytes {
            return Err(Error::Storage(format!("R2 holds {} bytes, wrote {bytes}", obj.size())));
        }
        Ok(PackMeta {
            pack: self.pack,
            push_id: None,
            count: self.count,
            bytes,
            commit_lo: self.commit_lo,
            commit_hi: self.commit_hi,
            created_at: self.created_at,
        })
    }
    async fn finish_inner(&mut self, budget: &mut ReqBudget) -> Result<(), Error> {
        if self.count != self.expected {
            return Err(Error::Internal(format!(
                "wrote {} entries, header says {}",
                self.count, self.expected
            )));
        }
        let trailer = self.sha1.clone().fin();
        self.part.extend_from_slice(&trailer);
        self.flush_if_full(budget).await?;
        if !self.part.is_empty() {
            let last = std::mem::take(&mut self.part);
            let n = u16::try_from(self.parts.len().saturating_add(1))
                .map_err(|_| Error::Limit("too many parts".into()))?;
            budget.charge(1)?;
            self.parts.push(self.mpu.upload_part(n, last).await?);
        }
        Ok(())
    }
    pub async fn abort(self) {
        let _ = self.mpu.abort().await;
    }

    // ---- GC build support (section 5.2): resume + checkpoint across slices ----

    pub async fn upload_id(&self) -> String {
        self.mpu.upload_id().await
    }

    /// Append a stored entry verbatim (no re-inflate/deflate): section 5.2's verbatim copy.
    /// `entry` is `varint header + zlib body` exactly as read from another pack.
    pub fn append_stored(&mut self, entry: &[u8]) -> Result<(u64, u32), Error> {
        let (kind, _size, _hlen) = codec::entry_header(entry)?;
        let len = u32::try_from(entry.len()).map_err(|_| Error::Limit("entry too long".into()))?;
        self.part.extend_from_slice(entry);
        self.sha1.update(entry);
        let offset = self.offset;
        self.offset = offset.saturating_add(u64::from(len));
        if kind == Kind::Commit {
            self.commit_lo = self.commit_lo.min(offset);
            self.commit_hi = self.offset;
        }
        self.count = self.count.saturating_add(1);
        Ok((offset, len))
    }

    /// Force-upload the buffered part as the next part when it is non-empty (callers guarantee
    /// the >= 5 MiB rule for non-final parts), and export a serde checkpoint: the trailer hasher
    /// state (gix_hash::Hasher cannot be exported, hence CkptSha1), offset and count.
    /// Returns (part_number, etag, checkpoint) — UploadedPart is not Clone.
    pub async fn checkpoint(
        &mut self,
        budget: &mut ReqBudget,
    ) -> Result<Option<(u16, String, WriterCkpt)>, Error> {
        if self.part.is_empty() {
            return Ok(None);
        }
        let chunk = std::mem::take(&mut self.part);
        let n = u16::try_from(self.parts.len().saturating_add(1))
            .map_err(|_| Error::Limit("too many parts".into()))?;
        budget.charge(1)?;
        let part = self.mpu.upload_part(n, chunk).await?;
        let rec = (part.part_number(), part.etag());
        self.parts.push(part);
        Ok(Some((
            rec.0,
            rec.1,
            WriterCkpt {
                pos: self.offset,
                sha: self.sha1.clone(),
                count: self.count,
                expected: self.expected,
                commit_lo: self.commit_lo,
                commit_hi: self.commit_hi,
            },
        )))
    }

    /// Rebind to an existing multipart upload and resume at a checkpoint (5.2).
    /// `etags` are the parts already uploaded, in order.
    pub async fn resume(
        bucket: &Bucket,
        key: &str,
        upload_id: &str,
        etags: &[String],
        state: &WriterCkpt,
        _budget: &mut ReqBudget,
    ) -> Result<Self, Error> {
        let mpu = bucket.inner.resume_multipart_upload(key, upload_id)?;
        let parts: Vec<UploadedPart> = etags
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                u16::try_from(i.saturating_add(1)).ok().map(|n| UploadedPart::new(n, e.clone()))
            })
            .collect();
        let created_at = platform::now_ms();
        Ok(Self {
            mpu,
            pack: PackId(
                key.rsplit('/').next().and_then(|f| f.strip_suffix(".pack")).unwrap_or_default().to_string(),
            ),
            part: Vec::new(),
            parts,
            offset: state.pos,
            count: state.count,
            expected: state.expected,
            sha1: state.sha.clone(),
            commit_lo: state.commit_lo,
            commit_hi: state.commit_hi,
            created_at,
        })
    }
}

fn i(n: u64) -> Result<V, Error> {
    Ok(V::from(i64::try_from(n).map_err(|_| Error::Internal("u64 to i64".into()))?))
}

/// Sync SQLite index, used only inside RepoDo and jobs (1.2).
pub struct Index<'s>(pub &'s SqlStorage);
impl<'s> Index<'s> {
    fn exec(&self, q: &str, args: Vec<V>) -> Result<worker::SqlCursor, Error> {
        self.0.exec(q, Some(args)).map_err(|e| Error::Storage(e.to_string()))
    }
    /// The reader query of 2.3, verbatim. Rows of ingesting/dead packs are invisible.
    /// Batches `IN` lists at 90 bound params (A6).
    pub fn lookup(&self, ids: &[ObjectId]) -> Result<Vec<Option<ObjLoc>>, Error> {
        #[derive(Deserialize)]
        struct R {
            sha: String,
            pack_id: String,
            idx: u32,
            offset: u64,
            len: u32,
            kind: u8,
            size: u64,
        }
        let mut by_sha: HashMap<String, ObjLoc> = HashMap::new();
        for chunk in ids.chunks(90) {
            let marks = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let args: Vec<V> = chunk.iter().map(|id| V::from(id.to_string().as_str())).collect();
            for r in self
                .exec(
                    &format!(
                        "SELECT o.sha, o.pack_id, o.idx, o.offset, o.len, o.kind, o.size FROM objects o \
                         JOIN packs p ON p.id = o.pack_id WHERE o.sha IN ({marks}) AND p.state = 'live' \
                         GROUP BY o.sha"
                    ),
                    args,
                )?
                .to_array::<R>()?
            {
                by_sha.entry(r.sha).or_insert(ObjLoc {
                    pack: PackId(r.pack_id),
                    idx: r.idx,
                    offset: r.offset,
                    len: r.len,
                    kind: kind_of(r.kind)?,
                    size: r.size,
                });
            }
        }
        Ok(ids.iter().map(|id| by_sha.get(&id.to_string()).cloned()).collect())
    }
    /// Presence in one named (typically `ingesting`) pack — the 2.5 self-subtraction.
    pub fn lookup_in_pack(&self, id: &ObjectId, pack: &PackId) -> Result<Option<ObjLoc>, Error> {
        #[derive(Deserialize)]
        struct R {
            idx: u32,
            offset: u64,
            len: u32,
            kind: u8,
            size: u64,
        }
        let row = self
            .exec(
                "SELECT idx, offset, len, kind, size FROM objects WHERE sha=? AND pack_id=? LIMIT 1",
                vec![V::from(id.to_string().as_str()), V::from(pack.0.as_str())],
            )?
            .to_array::<R>()?
            .into_iter()
            .next();
        Ok(row.map(|r| ObjLoc {
            pack: pack.clone(),
            idx: r.idx,
            offset: r.offset,
            len: r.len,
            kind: kind_of(r.kind).unwrap_or(Kind::Blob),
            size: r.size,
        }))
    }
    /// (count, bytes) for SendSet::mark's bitmap sizing.
    pub fn pack_meta(&self, pack: &PackId) -> Result<(u32, u64), Error> {
        #[derive(Deserialize)]
        struct R {
            count: i64,
            bytes: i64,
        }
        let r = self
            .exec(
                "SELECT count, bytes FROM packs WHERE id=? AND state='live'",
                vec![V::from(pack.0.as_str())],
            )?
            .to_array::<R>()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Storage(format!("pack {} not live", pack.0)))?;
        Ok((
            u32::try_from(r.count).map_err(|_| Error::Internal("count".into()))?,
            u64::try_from(r.bytes).map_err(|_| Error::Internal("bytes".into()))?,
        ))
    }
    /// Marked entries of one pack, offset order (plan_reads). `bitmap` has one bit per idx.
    pub fn entries_of(&self, pack: &PackId, bitmap: &[u8]) -> Result<Vec<ObjLoc>, Error> {
        #[derive(Deserialize)]
        struct R {
            idx: u32,
            offset: u64,
            len: u32,
            kind: u8,
            size: u64,
        }
        let mut out = Vec::new();
        let mut after: i64 = -1;
        loop {
            let rows = self
                .exec(
                    "SELECT idx, offset, len, kind, size FROM objects WHERE pack_id=? AND idx>? \
                     ORDER BY idx LIMIT 5000",
                    vec![V::from(pack.0.as_str()), V::from(after)],
                )?
                .to_array::<R>()?;
            let n = rows.len();
            for r in rows {
                after = i64::from(r.idx);
                let byte = usize::try_from(r.idx / 8).map_err(|_| Error::Internal("idx".into()))?;
                if bitmap.get(byte).map(|b| b & (1u8 << (r.idx % 8)) != 0) == Some(true) {
                    out.push(ObjLoc {
                        pack: pack.clone(),
                        idx: r.idx,
                        offset: r.offset,
                        len: r.len,
                        kind: kind_of(r.kind)?,
                        size: r.size,
                    });
                }
            }
            if n < 5000 {
                break;
            }
        }
        // idx order != offset order when a forward REF_DELTA was resolved late: the
        // object kept its original idx but was appended at the pack's tail
        out.sort_by_key(|l| l.offset);
        Ok(out)
    }
    /// Commit-kind entries of one pack in the byte range [lo, hi) — 7.4 prefetch:
    /// the commit walk's level reads extend into region reads so a linear history
    /// costs one range read per window, not one per BFS level.
    pub fn commits_in_range(
        &self,
        pack: &PackId,
        lo: u64,
        hi: u64,
    ) -> Result<Vec<(ObjectId, ObjLoc)>, Error> {
        #[derive(Deserialize)]
        struct R {
            sha: String,
            idx: u32,
            offset: u64,
            len: u32,
            size: u64,
        }
        let rows = self
            .exec(
                "SELECT o.sha, o.idx, o.offset, o.len, o.size FROM objects o \
                 JOIN packs p ON p.id = o.pack_id \
                 WHERE o.pack_id=? AND o.kind=1 AND o.offset>=? AND o.offset<? \
                 AND p.state='live' ORDER BY o.offset LIMIT 100000",
                vec![V::from(pack.0.as_str()), i(lo)?, i(hi)?],
            )?
            .to_array::<R>()?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let id = ObjectId::from_hex(r.sha.as_bytes())
                .map_err(|_| Error::Internal("bad sha in objects".into()))?;
            out.push((
                id,
                ObjLoc {
                    pack: pack.clone(),
                    idx: r.idx,
                    offset: r.offset,
                    len: r.len,
                    kind: Kind::Commit,
                    size: r.size,
                },
            ));
        }
        Ok(out)
    }
    /// Safe to repeat: ON CONFLICT DO NOTHING per row (a retried /_do/push/index is a no-op).
    pub fn insert_objects(&self, pack: &PackId, rows: &[ObjRow]) -> Result<(), Error> {
        if rows.len() > 10_000 {
            return Err(Error::Protocol("more than 10,000 rows".into()));
        }
        for r in rows {
            self.exec(
                "INSERT INTO objects(sha,pack_id,idx,offset,len,kind,size) VALUES(?,?,?,?,?,?,?) \
                 ON CONFLICT(sha,pack_id) DO NOTHING",
                vec![
                    V::from(r.sha.to_string().as_str()),
                    V::from(pack.0.as_str()),
                    V::from(i64::from(r.idx)),
                    i(r.offset)?,
                    V::from(i64::from(r.len)),
                    V::from(i64::from(git_kind(r.kind))),
                    i(r.size)?,
                ],
            )?;
        }
        Ok(())
    }
    pub fn set_pack_state(&self, pack: &PackId, state: PackState) -> Result<(), Error> {
        self.exec(
            "UPDATE packs SET state=? WHERE id=?",
            vec![V::from(state.as_str()), V::from(pack.0.as_str())],
        )?;
        Ok(())
    }
}

/// The complete schema (2.3, 3, 4, 8 + GC tables of section 5 / gc-and-repack-alarm's registry).
pub mod schema {
    use super::V;
    use crate::error::Error;
    use worker::SqlStorage;

    const DDL: &[&str] = &[
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS refs (name TEXT PRIMARY KEY, target TEXT NOT NULL, peeled TEXT, updated_at INTEGER NOT NULL) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS reflog (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, old TEXT NOT NULL, new TEXT NOT NULL, push_id TEXT NOT NULL, principal TEXT NOT NULL, at INTEGER NOT NULL)",
        "CREATE INDEX IF NOT EXISTS reflog_at ON reflog(at)",
        "CREATE TABLE IF NOT EXISTS pushes (id TEXT PRIMARY KEY, state TEXT NOT NULL, pack_id TEXT, principal TEXT NOT NULL, began_at INTEGER NOT NULL, ended_at INTEGER, gc_epoch INTEGER NOT NULL, result TEXT, swept_at INTEGER) WITHOUT ROWID",
        "CREATE INDEX IF NOT EXISTS pushes_state ON pushes(state)",
        "CREATE TABLE IF NOT EXISTS packs (id TEXT PRIMARY KEY, state TEXT NOT NULL, count INTEGER NOT NULL, bytes INTEGER NOT NULL, commit_lo INTEGER NOT NULL, commit_hi INTEGER NOT NULL, push_id TEXT, created_at INTEGER NOT NULL, dead_at INTEGER) WITHOUT ROWID",
        "CREATE INDEX IF NOT EXISTS packs_state ON packs(state)",
        "CREATE TABLE IF NOT EXISTS objects (sha TEXT NOT NULL, pack_id TEXT NOT NULL, idx INTEGER NOT NULL, offset INTEGER NOT NULL, len INTEGER NOT NULL, kind INTEGER NOT NULL, size INTEGER NOT NULL, PRIMARY KEY (sha, pack_id)) WITHOUT ROWID",
        "CREATE INDEX IF NOT EXISTS objects_pack ON objects(pack_id, idx)",
        "CREATE INDEX IF NOT EXISTS objects_pack_off ON objects(pack_id, offset)",
        "CREATE TABLE IF NOT EXISTS jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, run_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, cursor TEXT, payload TEXT NOT NULL DEFAULT '{}', state TEXT NOT NULL DEFAULT 'queued', last_error TEXT, started_at INTEGER, lease TEXT)",
        "CREATE TABLE IF NOT EXISTS marked (pack_id TEXT PRIMARY KEY, bitmap BLOB NOT NULL) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS gc_frontier (sha TEXT PRIMARY KEY) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS gc_seen (sha TEXT PRIMARY KEY) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS gc_parts (part_no INTEGER PRIMARY KEY, etag TEXT NOT NULL)",
    ];
    /// Columns added after first deploy. CREATE TABLE IF NOT EXISTS never updates an
    /// existing table, so DOs booted under an older schema need ALTER TABLE — SQLite
    /// does that only if the column is missing (checked via PRAGMA table_info).
    const LATE_COLS: &[(&str, &str, &str)] = &[
        ("pushes", "swept_at", "swept_at INTEGER"),
        ("packs", "dead_at", "dead_at INTEGER"),
        ("jobs", "started_at", "started_at INTEGER"),
        ("jobs", "lease", "lease TEXT"),
    ];
    pub fn migrate(sql: &SqlStorage) -> Result<(), Error> {
        for q in DDL {
            sql.exec(q, Some(Vec::<V>::new())).map_err(|e| Error::Storage(e.to_string()))?;
        }
        #[derive(serde::Deserialize)]
        struct Col {
            name: String,
        }
        for (table, col, decl) in LATE_COLS {
            let cols = sql
                .exec(&format!("PRAGMA table_info({table})"), Some(Vec::<V>::new()))
                .map_err(|e| Error::Storage(e.to_string()))?
                .to_array::<Col>()
                .map_err(|e| Error::Storage(e.to_string()))?;
            if !cols.iter().any(|c| c.name == *col) {
                sql.exec(&format!("ALTER TABLE {table} ADD COLUMN {decl}"), Some(Vec::<V>::new()))
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }
        }
        Ok(())
    }
}

/// Resumable build state for GC (5.2): carried in `gc.pos` across slices.
/// (Named WriterCkpt to keep PackState for the packs.state lifecycle.)
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct WriterCkpt {
    pub pos: u64,
    pub sha: CkptSha1,
    pub count: u32,
    pub expected: u32,
    pub commit_lo: u64,
    pub commit_hi: u64,
}

/// FIPS 180-1 SHA-1 with serde state — `gix_hash::Hasher` state cannot be exported, so a
/// resumable PackWriter carries this instead (gc-and-repack-alarm).
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct CkptSha1 {
    h: [u32; 5],
    len: u64,
    buf: Vec<u8>, // buf.len() < 64
}
impl CkptSha1 {
    pub fn new() -> Self {
        Self {
            h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            ..Self::default()
        }
    }
    pub fn update(&mut self, mut d: &[u8]) {
        self.len = self.len.wrapping_add(d.len() as u64);
        while !d.is_empty() {
            let n = (64usize).saturating_sub(self.buf.len()).min(d.len());
            let (a, b) = d.split_at(n);
            self.buf.extend_from_slice(a);
            d = b;
            if self.buf.len() == 64 {
                let mut x = [0u8; 64];
                x.copy_from_slice(&self.buf);
                self.block(&x);
                self.buf.clear();
            }
        }
    }
    fn block(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let mut s = self.h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((s[1] & s[2]) | (!s[1] & s[3]), 0x5A827999u32),
                20..=39 => (s[1] ^ s[2] ^ s[3], 0x6ED9EBA1),
                40..=59 => ((s[1] & s[2]) | (s[1] & s[3]) | (s[2] & s[3]), 0x8F1BBCDC),
                _ => (s[1] ^ s[2] ^ s[3], 0xCA62C1D6),
            };
            s = [
                s[0].rotate_left(5).wrapping_add(f).wrapping_add(s[4]).wrapping_add(k).wrapping_add(*wi),
                s[0],
                s[1].rotate_left(30),
                s[2],
                s[3],
            ];
        }
        for (h, x) in self.h.iter_mut().zip(s) {
            *h = h.wrapping_add(x);
        }
    }
    pub fn fin(mut self) -> [u8; 20] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf.len() != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut o = [0u8; 20];
        for (i, h) in self.h.iter_mut().enumerate() {
            o[i * 4..i * 4 + 4].copy_from_slice(&h.to_be_bytes());
        }
        o
    }
}
