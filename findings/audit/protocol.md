# QA Findings — protocol correctness (wire + edge + DO fetch surface)

Surface: `server/src/wire/mod.rs`, `server/src/edge/mod.rs`, plus the DO-side
`/_do/fetch` response path (`repo_do/mod.rs`, `pack/generate.rs`) where the wire
contract is actually produced.

---

## QA Findings

### Critical (blocks release)

1. **Issue**: `parse_receive_header` is not resumable, but `receive_pack` calls it incrementally — a receive header split across `fill()` boundaries silently drops every command line parsed in earlier calls.
   - **Reproduction**:
     - `edge/mod.rs:282-298`: the loop reuses one `PktReader pr` and calls `wire::parse_receive_header(&mut pr)` after each `body.fill(FILL_STEP)` (64 KiB, `edge/mod.rs:23`).
     - `wire/mod.rs:33-46`: `PktReader::next()` advances `self.pos` permanently for every decoded pkt.
     - `wire/mod.rs:149-160`: `parse_receive_header` builds *fresh* `commands`/`caps`/`shallow`/`first_command` locals per call and returns `Ok(None)` on `Incomplete` (line 163) — discarding the already-consumed lines.
     - So: if the first `fill` delivers ≥64 KiB that does not contain the flush (command section > 64 KiB ≈ ~700 ref commands of ~95 B, or ~1,400 `shallow` lines), every command consumed in call N is absent from the `ReceiveHeader` returned by call N+1.
   - **Concrete reproducer** (documented; no exec available):
     ```python
     import hashlib, struct
     # after a normal push so <target> is live
     cmds = b""
     for i in range(1500):
         line = b"0"*40 + b" " + target + b" refs/heads/r%05d" % i
         if i == 0:
             line += b"\x00report-status side-band-64k"
         cmds += f"{len(line)+5:04x}".encode() + line + b"\n"
     hdr = b"PACK" + struct.pack(">II", 2, 0)
     body = cmds + b"0000" + hdr + hashlib.sha1(hdr).digest()
     # POST body to /o/r/git-receive-pack
     ```
     Then `git ls-remote` — only the refs whose command lines started after the first ~64 KiB exist.
   - **Impact**: (a) pushed refs are silently dropped while the client gets `unpack ok` — silent data loss. (b) `caps` parsed from the real first command line (e.g. `side-band-64k`) are lost → `write_report_status` emits a *raw* pkt-stream to a client that demuxes sideband → client-side framing failure (the A11 class: committed but reads as failure). (c) `first_command` resets per call, so a `\0caps` on a later physical line is wrongly accepted.
   - **Fix**: re-parse from `pos = 0` each call (rebuild a `PktReader` over `pr.buf`, which already retains all pushed bytes — the 1 MiB cap bounds the O(n²) re-scan), or carry the accumulators in `PktReader`/a struct across calls. Also add a conformance case: push with >64 KiB command section.

### High (should fix)

2. **Issue**: a mid-stream error in the v2 fetch response loops forever — the stream never terminates.
   - `repo_do/mod.rs:643-654`: `stream::unfold` arm `Err(e) => { ...; Some((Ok(w.out), st)) }` returns `st` **unmodified** — `FetchStream::step` retries the same `self.next` index → same failing `pack_chunk` → same `Err` → another band-3 `ERR` frame, forever.
   - Contract §10 intent is "one band-3 ERR frame, then end the stream". As written, a persistent error (missing R2 key, budget exhaustion — note `Budget` also hits this arm, and after exhaustion `charge` fails *before* the R2 call so the loop never even awaits productively) yields an unbounded stream of `ERR` frames: the client hangs and the isolate burns its CPU limit.
   - **Fix**: after the error frame, mark the stream finished (e.g. set `st.next = usize::MAX`, or return `Some((Ok(w.out), st))` only once via a flag) so the next poll returns `None`.

3. **Issue**: every post-header push failure leaks an `open` `pushes` row; 64 leaked rows ⇒ all pushes refused for ~1 h.
   - `repo_do/mod.rs:322-356`: `/_do/push/begin` inserts `state='open'` and refuses at `open >= 64` (line 341-343).
   - `edge/mod.rs:303-306`: on `receive_inner` error the edge emits the A2 `unpack <err>` response but **never tells the DO to close the push** — the row stays `open` until the janitor expires it at `began_at + 3_600_000 ms` (`jobs/janitor.rs:17,35-46`).
   - **Reproducer**: run the conformance malformed-pack POST (`tests/conformance/run.sh:69-79`) 64 times → the next legitimate `git push` gets `unpack too many open pushes`. A CI job retrying a failing push DoSes itself without any attacker.
   - **Fix**: on the post-header error arm, call a `/_do/push/abort` route (or fold abort into `push/commit` handling) so failed pushes leave no `open` row.

### Medium (spec violation git tolerates / wrong data)

4. **Issue**: `ready = args.done || args.haves.is_empty() || !acks.is_empty()` sends `ready` + delim + `packfile` in the same response when **any** have acked but the client never sent `done`.
   - `wire/mod.rs:505-521` and the duplicate gate at `repo_do/mod.rs:618`.
   - Real git's `ok_to_give_up` is `done || all-haves-common`; with *partial* acks and no `done` it ends the response after the ACK lines so the client continues negotiation. Here the pack is computed against an incomplete have-set and shipped immediately. Git's client consumes a `ready`→`packfile` sequence fine (valid superset), but multi-round negotiation collapses into round 1 whenever any have matches — needlessly large packs, and semantics diverge from stateless RPC. The `no-done` arg (`wire/mod.rs:345`) makes early answer correct only for that case.
   - **Fix**: `ready = args.done || args.haves.is_empty()` (or `acks.len() == haves.len()`).

5. **Issue**: `done` suppresses the `acknowledgments` section entirely (`wire/mod.rs:506` — `if !args.done { ... }`). Git's `send_acks` still emits `acknowledgments` + `ACK`/`NAK` + `ready` before the delim on `done` requests when haves were sent; fetch-pack uses those ACKs to mark common objects for later rounds. Client tolerates the omission (the section is optional in the grammar) — spec deviation, minor.

6. **Issue**: `parse_v2_command` silently drops every pre-delim data line that isn't `command=`/`object-format=sha256` (`wire/mod.rs:269-278`). A request missing the `0001` delim gets its argument lines discarded instead of rejected: `command=ls-refs` + `ref-prefix refs/tags/` *without* a delim returns the **entire** ref list, not the filtered one. Similarly, a duplicate `command=` line silently overwrites (last wins, line 273-274). Malformed-input hard errors are the spec-consistent response — git dies on unknown/unexpected lines.
   - **Fix**: treat non-capability data lines before the delim, or a second `command=`, as `Error::Protocol`.

7. **Issue**: `include-tag` can emit tag objects whose target commit was never sent.
   - `pack/generate.rs:316-333`: the gate is `depth.contains_key(&peeled)` — but `depth` (`generate.rs:193`) records every *walked* commit, including unsent walls (`is_not`, `since_boundary`; `acks`/`cs` are fine since the client holds those). A tag peeling to an excluded (`deepen-not`) or pre-cutoff (`deepen-since`) commit is marked into the pack while its target is absent → `git fsck` "broken link" on the client.
   - **Fix**: test `sent.contains(&peeled) || acks.contains(&peeled) || cs.contains(&peeled)` — i.e., "commits the client will hold", not "commits we walked".

8. **Issue**: report-status is written unconditionally, ignoring negotiated caps.
   - `edge/mod.rs:400` / `wire/mod.rs:464-493`: the body is emitted whether or not the client sent `report-status`, and always in v1 format even if the client sent only `report-status-v2` (parsed at `wire/mod.rs:189`, never consulted). Per spec, no `report-status` cap ⇒ no report body at all. Stock git always sends `report-status` so this only bites custom/other clients; still wrong data on the wire.
   - Related: `caps.delete_refs` is parsed (`wire/mod.rs:191`) but never enforced — a delete command is honored even when `delete-refs` wasn't negotiated. Advisory only; harmless against the client that sent it.

### Low (nice to fix)

9. **`Git-Protocol: version=1` never emits the `version 1` pkt.** `Service::UploadPack { v1: false }` is hardcoded (`edge/mod.rs:155`); the `v1: true` arm in `write_advertisement_v0` (`wire/mod.rs:426-428`) is dead code. Contract rule 8 half-implemented. Harmless — clients proceed as v0.

10. **`info/refs` errors are wrapped as `ERR <msg>` pkt-lines.** `fetch()` calls `respond(r, true)` for every route (`edge/mod.rs:117`); §10 says plain text for `info/refs`. Cosmetic.

11. **Zero-command push commits a live pack.** A body of just `0000` + a valid PACK yields `ReceiveHeader{commands:[]}` (`wire/mod.rs:161-217` never rejects empty), is ingested, and `commit_push` flips it `live` with zero ref updates — live-but-unreachable objects, servable via `want <oid>` until GC. Git requires ≥1 command. Suggest `Error::Protocol` on empty commands.

12. **Error messages are interpolated into response pkt-lines unsanitized.** `echo_name` (`wire/mod.rs:459-461`) protects ref names, but `unpack {m}` (`wire/mod.rs:473`) and `ERR {m}` (`error.rs:130`) embed message text verbatim; `bad oid {hex}` (`wire/mod.rs:143`) and `couldn't find remote ref {qname}` (`repo_do/mod.rs:604`) carry raw client bytes — a `\n` injects extra lines into the stream. Reachable mostly pre-header (400 path) so it's cosmetic-garbled output today; sanitize anyway.

13. **Minor spec deviations, tolerated**:
    - `shallow` lines are accepted anywhere in the receive header (`wire/mod.rs:171-173`), not only before commands.
    - `hdr.shallow` (receive side) is parsed and never used (`edge/mod.rs` consumes only `hdr.commands`/`hdr.caps`); thin-pack bases resolve by sha anyway, so this is currently harmless.
    - `descend_past_cs` (`pack/generate.rs:207-221`) emits `unshallow` for a client-shallow commit even if one of its parents is then walled by `is_not`/`since_boundary` — `deepen`+`deepen-not`/`deepen-since` combos could unshallow a commit whose parent stays unsent. Exotic; note for a follow-up.

---

## Survived attack (verified clean)

- `PktReader`/`PktWriter`: `0003` and non-hex lengths → `Error::Protocol` (`wire/mod.rs:35`); overruns → `Incomplete` → wait/EOF error; `Delim`/`ResponseEnd` rejected in receive header (166-168) and v2 bodies (266). `remainder()` correctly returns post-flush PACK bytes and `BodyReader::unread` sees an empty buf (`consume(usize::MAX)` each iter, `edge/mod.rs:291-292`).
- `parse_receive_header`: NUL-caps on non-first lines rejected (178-180); missing/empty ref name (212-214), missing spaces (206-211), non-hex/short oids (142-144), `object-format=sha256` (196-200) all rejected. Command section cap at 1 MiB counts only pre-flush bytes (the `> CMD_CAP` check at `edge/mod.rs:293` runs only while no flush was seen, so pack bytes can't trip it).
- `parse_fetch`: empty `want`/`want-ref` sets rejected (355-359); `deepen 0`, negative/oversized `deepen-since`, non-`blob:` filters rejected; unknown args → 400. `Index::lookup` batches `IN` lists at 90 (`store/mod.rs:620`) so large `have`/`want` sets can't hit the 100-param DO limit.
- `upload_pack` body cap: declared `Content-Length` rejected pre-read and decompressed bytes capped streaming (`edge/mod.rs:237-260`) — no gzip bypass.
- A2 boundary: pre-header errors → HTTP 4xx (`receive_pack` `?` propagation); post-header → 200 + `unpack/ng` (`edge/mod.rs:303-306`); `client_message` scrubs Storage/Internal detail (`error.rs:98-103`).
- `write_report_status` honors the inner-flush requirement (A11, `wire/mod.rs:481-487`); double-flush on the non-sideband path is benign.
- `write_fetch_prelude` section order `acknowledgments → shallow-info → wanted-refs → packfile` matches the v2 grammar; `wanted-refs` names can only echo strings that exactly matched stored (validated) ref names (`repo_do/mod.rs:599-606`) — no injection.
- `BodyReader` gzip path, `fill` EOF semantics, and the `unread` splice are correct; `parse_v2_command` parses the fully-buffered body (no incremental bug there).
- v0 refusal is HTTP 200 + `ERR` pkt-line, consistent with A2's ≥300-body-discard rationale (though it contradicts rule 7's "400" text — amendment wins).
