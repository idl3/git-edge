//! CONTRACTS.md 1.1: pkt-line, sideband, v2 command parser, receive header parser, report-status.
//! No awaits anywhere in this module. No `worker` import.

use bstr::{BStr, BString, ByteSlice};
use gix_hash::ObjectId;
use gix_packetline::{blocking_io::encode, decode, Channel, PacketLineRef};

use crate::error::Error;

pub mod http;

pub const MAX_PKT_DATA: usize = 65516;
pub const MAX_BAND_DATA: usize = 65515;
const AGENT: &str = "agent=git-edge/0.1";

pub enum Pkt<'a> {
    Data(&'a [u8]),
    Flush,
    Delim,
    ResponseEnd,
}

#[derive(Default)]
pub struct PktReader {
    pub buf: Vec<u8>,
    pos: usize,
}
impl PktReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    /// Ok(None) = need more bytes. `0003` or a length past the buffer is Error::Protocol (rule 1).
    pub fn next(&mut self) -> Result<Option<Pkt<'_>>, Error> {
        let rest = self.buf.get(self.pos..).unwrap_or(&[]);
        let (line, used) = match decode::streaming(rest).map_err(|e| Error::Protocol(format!("pkt-line: {e}")))? {
            decode::Stream::Incomplete { .. } => return Ok(None),
            decode::Stream::Complete { line, bytes_consumed } => (line, bytes_consumed),
        };
        self.pos = self.pos.saturating_add(used);
        Ok(Some(match line {
            PacketLineRef::Data(d) => Pkt::Data(d),
            PacketLineRef::Flush => Pkt::Flush,
            PacketLineRef::Delimiter => Pkt::Delim,
            PacketLineRef::ResponseEnd => Pkt::ResponseEnd,
        }))
    }
    /// Unread bytes after the last returned pkt. Consumes the reader.
    pub fn remainder(&mut self) -> Vec<u8> {
        let out = self.buf.get(self.pos..).unwrap_or(&[]).to_vec();
        self.pos = self.buf.len();
        out
    }
}

#[derive(Default)]
pub struct PktWriter {
    pub out: Vec<u8>,
}
impl PktWriter {
    pub fn data(&mut self, b: &[u8]) -> Result<(), Error> {
        if b.len() > MAX_PKT_DATA {
            return Err(Error::Internal("pkt-line over 65516 bytes".into()));
        }
        encode::data_to_write(b, &mut self.out)
            .map(|_| ())
            .map_err(|e| Error::Internal(e.to_string()))
    }
    pub fn text(&mut self, s: &str) -> Result<(), Error> {
        if s.ends_with('\n') {
            self.data(s.as_bytes())
        } else {
            self.data(format!("{s}\n").as_bytes())
        }
    }
    pub fn flush(&mut self) {
        let _ = encode::flush_to_write(&mut self.out);
    }
    pub fn delim(&mut self) {
        let _ = encode::delim_to_write(&mut self.out);
    }
    pub fn response_end(&mut self) {
        let _ = encode::response_end_to_write(&mut self.out);
    }
}

fn line(w: &mut PktWriter, b: &[u8]) {
    if w.data(b).is_err() {
        let _ = encode::error_to_write(b"line too long", &mut w.out);
    }
}

pub struct Sideband<'w> {
    pub w: &'w mut PktWriter,
}
impl<'w> Sideband<'w> {
    pub fn new(w: &'w mut PktWriter) -> Self {
        Self { w }
    }
    /// Band 1, frames of <= MAX_BAND_DATA (rule 2).
    pub fn data(&mut self, b: &[u8]) {
        for c in b.chunks(MAX_BAND_DATA) {
            let _ = encode::band_to_write(Channel::Data, c, &mut self.w.out);
        }
    }
    pub fn progress(&mut self, s: &str) {
        let _ = encode::band_to_write(
            Channel::Progress,
            s.as_bytes().chunks(MAX_BAND_DATA).next().unwrap_or(b""),
            &mut self.w.out,
        );
    }
    pub fn error(&mut self, s: &str) {
        let _ = encode::band_to_write(
            Channel::Error,
            s.as_bytes().chunks(MAX_BAND_DATA).next().unwrap_or(b""),
            &mut self.w.out,
        );
    }
}

#[derive(Clone)]
pub struct RefCommand {
    pub old: ObjectId,
    pub new: ObjectId,
    pub name: BString,
}
pub struct ReceiveCaps {
    pub report_status: bool,
    pub report_status_v2: bool,
    pub side_band_64k: bool,
    pub delete_refs: bool,
    pub quiet: bool,
    pub ofs_delta: bool,
    pub atomic: bool,
    pub agent: Option<BString>,
}
pub struct ReceiveHeader {
    pub commands: Vec<RefCommand>,
    pub caps: ReceiveCaps,
    /// Client-shallow ids sent before a push — parsed for completeness but not needed:
    /// a thin pack's bases resolve against server-live objects either way.
    pub shallow: Vec<ObjectId>,
}

fn oid(hex: &[u8]) -> Result<ObjectId, Error> {
    ObjectId::from_hex(hex).map_err(|_| Error::Protocol(format!("bad oid {}", hex.as_bstr())))
}

/// `shallow <oid>` lines, then `<old> <new> <name>[\0caps]` lines, up to and including the flush.
/// Ok(None) = need more bytes. After Ok(Some), `reader.remainder()` is the PACK (possibly empty).
pub fn parse_receive_header(r: &mut PktReader) -> Result<Option<ReceiveHeader>, Error> {
    let mut commands = Vec::new();
    let mut shallow = Vec::new();
    let mut caps = ReceiveCaps {
        report_status: false,
        report_status_v2: false,
        side_band_64k: false,
        delete_refs: false,
        quiet: false,
        ofs_delta: false,
        atomic: false,
        agent: None,
    };
    let mut first_command = true;
    loop {
        let d = match r.next()? {
            None => return Ok(None),
            Some(Pkt::Flush) => break,
            Some(Pkt::Data(d)) => d.to_vec(),
            Some(Pkt::Delim) | Some(Pkt::ResponseEnd) => {
                return Err(Error::Protocol("delim/response-end in receive header".into()))
            }
        };
        let d = d.strip_suffix(b"\n").map(|s| s.to_vec()).unwrap_or(d);
        if let Some(rest) = d.strip_prefix(b"shallow ") {
            shallow.push(oid(rest)?);
            continue;
        }
        // <old> <new> <name> with an optional \0<capabilities> suffix on the first command line.
        let (mut line, line_caps) = match d.iter().position(|b| *b == 0) {
            Some(z) => {
                if !first_command {
                    return Err(Error::Protocol("capabilities on a later command".into()));
                }
                (d.get(..z).unwrap_or(&[]).to_vec(), Some(d.get(z + 1..).unwrap_or(&[]).to_vec()))
            }
            None => (d.clone(), None),
        };
        if let Some(cs) = line_caps {
            for c in cs.split(|b| *b == b' ') {
                match c {
                    b"report-status" => caps.report_status = true,
                    b"report-status-v2" => caps.report_status_v2 = true,
                    b"side-band-64k" => caps.side_band_64k = true,
                    b"delete-refs" => caps.delete_refs = true,
                    b"quiet" => caps.quiet = true,
                    b"ofs-delta" => caps.ofs_delta = true,
                    b"atomic" => caps.atomic = true,
                    b"no-thin" => {}
                    c if c.starts_with(b"agent=") => caps.agent = Some(c.into()),
                    c if c.starts_with(b"object-format=") => {
                        if c != b"object-format=sha1" {
                            return Err(Error::Protocol("object-format sha256 unsupported".into()));
                        }
                    }
                    _ => {}
                }
            }
        }
        // split on first two spaces: `<old> <new> <name>`
        let sp1 = line.iter().position(|b| *b == b' ').ok_or_else(|| Error::Protocol("bad command".into()))?;
        let mut rest = line.split_off(sp1);
        rest.remove(0); // the space itself
        let old_hex = line;
        let sp2 = rest.iter().position(|b| *b == b' ').ok_or_else(|| Error::Protocol("bad command".into()))?;
        let (new_hex, name) = (rest.get(..sp2).unwrap_or(&[]), rest.get(sp2 + 1..).unwrap_or(&[]));
        if name.is_empty() {
            return Err(Error::Protocol("command without ref name".into()));
        }
        commands.push(RefCommand { old: oid(&old_hex)?, new: oid(new_hex)?, name: name.into() });
        first_command = false;
    }
    // A flush-only request is legal: "everything up-to-date" pushes carry no commands.
    Ok(Some(ReceiveHeader { commands, caps, shallow }))
}

pub enum Filter {
    BlobNone,
    BlobLimit(u64),
}
pub struct LsRefsArgs {
    pub symrefs: bool,
    pub peel: bool,
    pub unborn: bool,
    pub prefixes: Vec<BString>,
}
pub struct FetchArgs {
    pub wants: Vec<ObjectId>,
    pub want_refs: Vec<BString>,
    pub haves: Vec<ObjectId>,
    pub done: bool,
    pub thin_pack: bool,
    pub no_progress: bool,
    pub include_tag: bool,
    pub ofs_delta: bool,
    pub deepen: Option<u32>,
    pub deepen_since: Option<i64>,
    /// deepen-not carries ref *names* (not oids) — resolved server-side like want-ref.
    pub deepen_not: Vec<BString>,
    pub deepen_relative: bool,
    pub shallow: Vec<ObjectId>,
    pub filter: Option<Filter>,
    /// `packfile-uris <csv>` — the URI protocols the client accepts (A30). Empty
    /// when the arg wasn't sent; the server then never emits a URIs section.
    /// `None` = arg absent; `Some([])` = client opted in but named no protocols
    /// (real git accepts an empty csv — it just means "never mint me a URI").
    pub packfile_uris: Option<Vec<BString>>,
}
pub enum V2Command {
    LsRefs(LsRefsArgs),
    Fetch(FetchArgs),
}

fn bad(what: &str, x: &[u8]) -> Error {
    Error::Protocol(format!("{what}: unknown argument {}", x.as_bstr()))
}
fn num(b: &[u8]) -> Option<u64> {
    b.to_str().ok()?.parse().ok()
}

/// `command=...`, capability lines, delim, arguments, flush. Unknown command or argument -> Error::Protocol.
pub fn parse_v2_command(body: &[u8]) -> Result<V2Command, Error> {
    let mut r = PktReader::default();
    r.push(body);
    let (mut cmd, mut args, mut in_args) = (None::<Vec<u8>>, Vec::<BString>::new(), false);
    loop {
        match r.next()? {
            None | Some(Pkt::ResponseEnd) => return Err(Error::Protocol("truncated v2 command".into())),
            Some(Pkt::Flush) => break,
            Some(Pkt::Delim) => in_args = true,
            Some(Pkt::Data(d)) => {
                let d = d.strip_suffix(b"\n").unwrap_or(d);
                if in_args {
                    args.push(d.into());
                } else if let Some(c) = d.strip_prefix(b"command=") {
                    if cmd.is_some() {
                        return Err(Error::Protocol("duplicate command line".into()));
                    }
                    cmd = Some(c.to_vec());
                } else if d == b"object-format=sha256" {
                    return Err(Error::Protocol("object-format sha256 unsupported".into()));
                } else if !(d.starts_with(b"agent=")
                    || d.starts_with(b"object-format=")
                    || d.starts_with(b"session-id="))
                {
                    // pre-delim lines are command + capabilities only; an argument line
                    // here means the request lost its delim — reject rather than drop it
                    return Err(Error::Protocol(format!("unexpected pre-delim line {}", d.as_bstr())));
                }
            }
        }
    }
    match cmd.as_deref() {
        Some(b"ls-refs") => Ok(V2Command::LsRefs(parse_ls_refs(&args)?)),
        Some(b"fetch") => Ok(V2Command::Fetch(parse_fetch(&args)?)),
        _ => Err(Error::Protocol("unknown command".into())),
    }
}

fn parse_ls_refs(args: &[BString]) -> Result<LsRefsArgs, Error> {
    let mut a = LsRefsArgs { symrefs: false, peel: false, unborn: false, prefixes: Vec::new() };
    for l in args {
        match l.as_slice() {
            b"symrefs" => a.symrefs = true,
            b"peel" => a.peel = true,
            b"unborn" => a.unborn = true,
            x => {
                if a.prefixes.len() >= 32 {
                    return Err(bad("ls-refs", b"too many ref-prefix")); // a prefix scan is O(refs)
                }
                a.prefixes.push(x.strip_prefix(b"ref-prefix ").ok_or_else(|| bad("ls-refs", x))?.into());
            }
        }
    }
    Ok(a)
}

fn parse_fetch(args: &[BString]) -> Result<FetchArgs, Error> {
    let mut f = FetchArgs {
        wants: vec![],
        want_refs: vec![],
        haves: vec![],
        done: false,
        thin_pack: false,
        no_progress: false,
        include_tag: false,
        ofs_delta: false,
        deepen: None,
        deepen_since: None,
        deepen_not: vec![],
        deepen_relative: false,
        shallow: vec![],
        filter: None,
        packfile_uris: None,
    };
    for l in args {
        let (k, v) = l.split_once_str(" ").unwrap_or((l.as_slice(), b""));
        match k {
            b"want" => f.wants.push(oid(v)?),
            b"want-ref" => {
                if v.is_empty() {
                    return Err(bad("want-ref", v));
                }
                f.want_refs.push(v.into());
            }
            b"have" => f.haves.push(oid(v)?),
            b"shallow" => f.shallow.push(oid(v)?),
            b"done" => f.done = true,
            b"thin-pack" => f.thin_pack = true,
            b"no-progress" => f.no_progress = true,
            b"include-tag" => f.include_tag = true,
            b"ofs-delta" => f.ofs_delta = true,
            b"deepen" => {
                f.deepen = Some(
                    num(v).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| bad("deepen", v))?,
                )
            }
            b"deepen-since" => {
                f.deepen_since = Some(num(v).and_then(|n| i64::try_from(n).ok()).ok_or_else(|| bad("deepen-since", v))?)
            }
            b"deepen-not" => {
                if v.is_empty() {
                    return Err(bad("deepen-not", v));
                }
                f.deepen_not.push(v.into());
            }
            b"deepen-relative" => f.deepen_relative = true,
            b"no-done" => {} // informational: we answer before `done` anyway
            b"packfile-uris" => {
                // spec: at most one such line; the value is a csv of protocols
                // (possibly empty — real git accepts it, we just never mint)
                if f.packfile_uris.is_some() {
                    return Err(bad("packfile-uris", v));
                }
                f.packfile_uris = Some(
                    v.split(|b| *b == b',')
                        .filter(|s| !s.is_empty())
                        .map(BString::from)
                        .collect(),
                );
            }
            b"filter" if v == b"blob:none" => f.filter = Some(Filter::BlobNone),
            b"filter" => {
                f.filter = Some(Filter::BlobLimit(
                    v.strip_prefix(b"blob:limit=").and_then(num).ok_or_else(|| bad("filter", v))?,
                ))
            }
            _ => return Err(bad("fetch", k)),
        }
    }
    // git dies on ambiguous shallow-cut combinations ("deepen and deepen-since (or
    // deepen-not) cannot be used together"; since+not likewise)
    if f.deepen.is_some() && (f.deepen_since.is_some() || !f.deepen_not.is_empty()) {
        return Err(Error::Protocol(
            "deepen and deepen-since (or deepen-not) cannot be used together".into(),
        ));
    }
    if f.deepen_since.is_some() && !f.deepen_not.is_empty() {
        return Err(Error::Protocol("deepen-since and deepen-not cannot be used together".into()));
    }
    if f.wants.is_empty() && f.want_refs.is_empty() {
        Err(Error::Protocol("fetch: no want lines".into()))
    } else {
        Ok(f)
    }
}

pub struct RefRow {
    pub name: BString,
    pub target: ObjectId,
    pub peeled: Option<ObjectId>,
}

/// Rule 6, byte-exact and static.
pub fn write_capability_advertisement_v2(w: &mut PktWriter) {
    for l in [
        "version 2",
        AGENT,
        "ls-refs=unborn",
        "fetch=shallow filter packfile-uris",
        "object-format=sha1",
    ] {
        line(w, format!("{l}\n").as_bytes());
    }
    w.flush();
}

/// HEAD first (meta.head, symbolic, section 3), then refs, flush.
pub fn write_ls_refs(w: &mut PktWriter, a: &LsRefsArgs, head: Option<&BStr>, refs: &[RefRow]) {
    let wanted = |n: &[u8]| a.prefixes.is_empty() || a.prefixes.iter().any(|p| n.starts_with(p));
    let peel =
        |r: &RefRow| if a.peel { r.peeled.map(|p| format!(" peeled:{p}")).unwrap_or_default() } else { String::new() };
    if let Some(h) = head.filter(|_| wanted(b"HEAD")) {
        match refs.iter().find(|r| r.name.as_bstr() == h.as_bstr()) {
            Some(r) => line(
                w,
                format!(
                    "{} HEAD{}{}\n",
                    r.target,
                    if a.symrefs { format!(" symref-target:{}", h.as_bstr()) } else { String::new() },
                    peel(r)
                )
                .as_bytes(),
            ),
            None => {
                if a.unborn {
                    line(w, format!("unborn HEAD symref-target:{}\n", h.as_bstr()).as_bytes())
                }
            }
        }
    }
    for r in refs.iter().filter(|r| wanted(&r.name)) {
        line(w, format!("{} {}{}\n", r.target, r.name.as_bstr(), peel(r)).as_bytes());
    }
    w.flush();
}

pub enum Service {
    UploadPack { v1: bool },
    ReceivePack,
}

/// Rules 5, 7, 8: `# service=`, flush, [`version 1`], first ref with \0caps, the rest, peeled `^{}`, flush.
pub fn write_advertisement_v0(w: &mut PktWriter, service: Service, head: Option<&BStr>, refs: &[RefRow]) {
    let (name, caps) = match service {
        Service::UploadPack { .. } => ("git-upload-pack", format!("object-format=sha1 {AGENT}")),
        Service::ReceivePack => (
            "git-receive-pack",
            // report-status-v2 is deliberately not advertised: its extra option-line
            // section is a strict superset of v1 and every client falls back cleanly.
            format!(
                "report-status delete-refs side-band-64k quiet ofs-delta atomic object-format=sha1 {AGENT}"
            ),
        ),
    };
    line(w, format!("# service={name}\n").as_bytes());
    w.flush();
    if matches!(service, Service::UploadPack { v1: true }) {
        line(w, b"version 1\n");
    }
    let head_row = head.and_then(|h| refs.iter().find(|r| r.name.as_bstr() == h.as_bstr()));
    let mut first = true;
    for (id, n, peeled) in head_row
        .into_iter()
        .map(|r| (r.target, BStr::new("HEAD"), None))
        .chain(refs.iter().map(|r| (r.target, r.name.as_bstr(), r.peeled)))
    {
        if first {
            line(w, format!("{id} {}\0{caps}\n", n.as_bstr()).as_bytes());
            first = false;
        } else {
            line(w, format!("{id} {}\n", n.as_bstr()).as_bytes());
        }
        if let Some(p) = peeled {
            line(w, format!("{p} {}^{{}}\n", n.as_bstr()).as_bytes());
        }
    }
    if first {
        line(w, format!("{} capabilities^{{}}\0{caps}\n", ObjectId::null(gix_hash::Kind::Sha1)).as_bytes());
    }
    w.flush();
}

pub enum RefResult {
    Ok(BString),
    Ng(BString, &'static str),
}

/// A name echoed into report lines may be one the server rejected: non-graphic bytes
/// (control chars, spaces) are replaced so a hostile name cannot inject text.
fn echo_name(n: &[u8]) -> BString {
    n.iter().map(|b| if b.is_ascii_graphic() { *b } else { b'?' }).collect()
}

/// Rule 4: `unpack ok`/`unpack <err>` then `ok <ref>`/`ng <ref> <reason>`, band 1 under side-band-64k.
pub fn write_report_status(
    w: &mut PktWriter,
    unpack: Result<(), &str>,
    results: &[RefResult],
    caps: &ReceiveCaps,
) -> Result<(), Error> {
    let mut body = PktWriter::default();
    match unpack {
        Ok(()) => body.text("unpack ok")?,
        Err(m) => body.text(&format!("unpack {m}"))?,
    }
    for r in results {
        match r {
            RefResult::Ok(n) => body.text(&format!("ok {}", echo_name(n).as_bstr()))?,
            RefResult::Ng(n, why) => body.text(&format!("ng {} {why}", echo_name(n).as_bstr()))?,
        }
    }
    // the inner pkt-stream must end with a flush: without it the client's demuxed
    // reader hits pipe EOF and dies with "the remote end hung up unexpectedly"
    body.flush();
    if caps.side_band_64k {
        let mut sb = Sideband { w };
        sb.data(&body.out);
        w.flush();
    } else {
        w.out.extend_from_slice(&body.out);
        w.flush();
    }
    Ok(())
}

/// Rule 3 and section 9 step 6. Ok(false): response ends here (no pack). Otherwise the caller
/// streams Sideband::data frames, then flush. `uris` is the packfile-uris section payload —
/// each entry a full "<hash> <uri>" line (A30) — emitted between wanted-refs and packfile.
pub fn write_fetch_prelude(
    w: &mut PktWriter,
    args: &FetchArgs,
    acks: &[ObjectId],
    wanted_refs: &[(ObjectId, BString)],
    shallow: &[ObjectId],
    unshallow: &[ObjectId],
    uris: &[String],
) -> Result<bool, Error> {
    let ready = args.done || args.haves.is_empty() || acks.len() >= args.haves.len();
    // acknowledgments + ready only exist when the client negotiated (sent haves) —
    // a clone goes straight to shallow-info/packfile and rejects both outright. The
    // section is omitted entirely once the client sent `done` (rule 3).
    if !args.haves.is_empty() && !args.done {
        w.text("acknowledgments")?;
        if acks.is_empty() {
            w.text("NAK")?;
        } else {
            for a in acks {
                w.text(&format!("ACK {a}"))?;
            }
        }
        if !ready {
            w.flush();
            return Ok(false);
        }
        w.text("ready")?;
        w.delim();
    }
    if !shallow.is_empty() || !unshallow.is_empty() {
        w.text("shallow-info")?;
        for s in shallow {
            w.text(&format!("shallow {s}"))?;
        }
        for u in unshallow {
            w.text(&format!("unshallow {u}"))?;
        }
        w.delim();
    }
    if !wanted_refs.is_empty() {
        w.text("wanted-refs")?;
        for (id, name) in wanted_refs {
            w.text(&format!("{id} {}", name.as_bstr()))?;
        }
        w.delim();
    }
    if !uris.is_empty() {
        w.text("packfile-uris")?;
        for u in uris {
            w.text(u)?;
        }
        w.delim();
    }
    w.text("packfile")?;
    Ok(true)
}
