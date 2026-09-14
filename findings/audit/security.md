# Security audit — git-edge server

Surface: auth, routing/tenant isolation, path parsing, injection, DoS/memory, secrets.
Method: full read of `server/src/` (auth, edge, wire, repo_do, store, pack, jobs,
platform, lib, error) + CONTRACTS.md + conformance harness. No live probes
(no exec tool available); all findings are trace-verified with reproducers.

## Summary

| # | Sev | Finding |
|---|-----|---------|
| 1 | HIGH | Mid-stream fetch error → infinite band-3 ERR loop (never terminates) |
| 2 | HIGH | Ingest accepts commits/tags it cannot parse; fetch hard-fails on them → pushed object permanently wedges the repo's reads |
| 3 | HIGH | Fetch walk memory is count-bounded, not byte-bounded; `load()`/`read_entries`/`entries_of`/`plan_reads` materialize whole rounds → DO isolate OOM |
| 4 | HIGH | Delta-chain resolution holds up to 64 raw entries at once (~1 GiB) → edge OOM on crafted pack |
| 5 | HIGH | Push-path memory: `entries` (~80 MB at cap) + `bases` + `external` map far exceed the 128 MB isolate budget the contract assumed |
| 6 | HIGH | Zero-cost ref spam: ~20k ref creates per packless POST, no per-repo ref cap → unbounded advertisement → all reads OOM/500 |
| 7 | MED | `pushes` table grows forever; `push_begin` full-scans it (no index on state) |
| 8 | MED | Forward REF_DELTA (base later in the same pack) rejected — valid packs refused |
| 9 | MED | v0 `git-upload-pack` POST returns HTTP 200, contract 1.1 rule 7 says 400 |
| 10 | LOW | Raw client bytes echoed into `ERR`/`unpack` lines (ANSI/newline terminal injection); oversized msg breaks the 4-hex length field |
| 11 | LOW | Non-canonical base64 decoder; colonless Basic blob treated as token |
| 12 | LOW | `seg_ok` permits `.`/`..` segments; no repo-creation quota → junk DOs each running a Janitor alarm forever |
| 13 | LOW | Uncharged R2 calls: `mpu.abort()`, `Bucket::delete`/`delete_multiple` |
| 14 | LOW | Janitor marks `swept_at` / deletes pack rows even when the R2 delete failed → permanent R2 orphans |
| 15 | LOW | `Level::Write` arm 500s instead of 401 when `GE_READ_TOKEN` is unset |
| 16 | LOW | `BodyReader::fill` single-chunk overshoot past the 1 MiB caps |

No CRITICALs: no auth bypass, no cross-tenant DO reach, no SQL injection found.
See "Survived attack" for what was verified clean.

---

## HIGH

### 1. Mid-stream fetch error loops forever — `repo_do/mod.rs:644-655`

```rust
let s = stream::unfold(st, |mut st| async move {
    match st.step().await {
        Ok(Some(chunk)) => Some((Ok::<Vec<u8>, Error>(chunk), st)),
        Ok(None) => None,
        Err(e) => {
            // mid-stream: one band-3 ERR frame, then the stream ends (section 10)
            ...
            Some((Ok(w.out), st))   // <-- st returned UNCHANGED
        }
    }
});
```

Invariant violated: section 10 / comment — "then the stream ends". Actual: `st` is
returned unmodified, `unfold` polls `st.step()` again, `self.next` was never
incremented (`FetchStream::step`, repo_do/mod.rs:687-699 — `next += 1` runs only
after success), so the identical operation is retried. A persistent error —
missing R2 key (`read_range`, store/mod.rs:274), exhausted `ReqBudget`
(`budget.charge`, store/mod.rs:267), bad trailer finalize — produces an infinite
stream of ERR frames at CPU speed. If the error came from `read_range`, each retry
is another R2 subrequest.

Expected: emit one band-3 `ERR` frame, then `None` (or set a `done` flag).

Impact: every failed fetch wedges into an unending response; the DO invocation
burns CPU/wall-clock until the platform kills it; the client sees an endless
stream. Combined with finding 2/3 this is trivially triggerable.

Reproducer: push a repo, delete the R2 pack object out-of-band (or force a GC
sweep between plan and stream), then `git clone`. Instead of one ERR + EOF the
response streams ERR frames indefinitely. Trace-only equivalent: any `Err` from
`pack_chunk` (generate.rs:406-426) — e.g. `Error::Budget` — loops forever because
`next` never advances.

Fix: `Err(e) => { ...emit frame...; None }` — the `Some` arm must not reschedule
`st`.

### 2. Malformed commit/tag poisons a repo permanently — `pack/ingest.rs:415-441` vs `pack/generate.rs:148,204,109`

Ingest is lenient where fetch is fatal:

- `extract_links` (ingest.rs:419-423): `if let Ok(t) = it.tree_id()` — a commit
  whose `tree` header is missing/malformed contributes **no tree link**, so the
  2.5 connectivity check passes it.
- `extract_links` (ingest.rs:434-436): `if let Ok(t) = TagRef::from_bytes(..)` —
  an unparseable tag contributes no link either.
- Trees, by contrast, are strict (ingest.rs:426-431 → `Error::Unpack`).

At fetch, the same parses are hard errors:
- `generate.rs:148` `CommitRefIter::from_bytes(data, ..).tree_id().map_err(Error::Storage)?`
- `generate.rs:109` `TagRef::from_bytes(..).map_err(Error::Storage)?`
- `generate.rs:204-207` edge commits, same.

`commit_push`/`apply_one` (repo_do/mod.rs:502-508) only requires `new` to be a
live object — kind and well-formedness are never checked. So:

1. Attacker (write token) hand-crafts a commit object with no `tree` line
   (a pack entry of kind commit over arbitrary zlib bytes; pass A/B store it
   verbatim — `decode_mini`/`compute_hash` don't validate structure).
2. Pushes `refs/heads/main -> <that oid>`: live-object check passes.
3. Every subsequent `git clone`/`fetch` walks the commit → `tree_id()` →
   `Error::Storage` → whole fetch fails (and per finding 1, the mid-stream arm
   can spin).

The repo is un-cloneable until someone deletes the ref and GC collects the pack
(≥ quiet+grace, ~70 min) — persistent, and not visible in the push response.
Also exploitable as a connectivity hole: a commit's missing tree is never
required to be live (the link was silently dropped at ingest), so a ref can
point at history the object store doesn't contain.

Expected: ingest rejects a commit whose `tree_id()` fails, a tag whose
`TagRef::from_bytes` fails (symmetric with trees), or `apply_one` additionally
verifies `new` resolves to a well-formed object of the expected kind.

Reproducer (documented; no exec): build a commit buffer `"author A <a@b> 0 +0000\n\nmsg\n"`
(no `tree` line), wrap as a full commit entry in a pack v2 (compute its SHA-1
externally), send receive-pack header `<null> <sha> refs/heads/x\0 report-status` +
pack → push returns `ok`, then `git ls-remote`/`clone` fails every time.

### 3. Fetch-side memory: rounds are count-bounded, not byte-bounded — `pack/generate.rs:294-315`, `store/mod.rs:286-326`

`load()` fetches an entire round's objects before the `MAX_MEM` guard runs:

```rust
for (id, entry) in bucket.read_entries(&locs, budget).await? {   // ALL bytes at once
    let (k, data) = codec::decode_entry(&entry)?;
    mem.insert(id, k, data);
}
if mem.bytes > MAX_MEM { Err(...) }   // checked AFTER everything is materialized
```

Two compounding defects:
- `Bucket::read_entries` (store/mod.rs:295-325) accumulates every span's bytes
  into one `out` Vec — total memory = sum of all requested `len`s, unbounded
  within the call.
- The commit queue per level can reach MAX_COMMITS=200,000 (generate.rs:140-144);
  the tree loop chunks at 10,000 (generate.rs:219) but each tree can be 16 MiB.

Realistic worst cases:
- A repo with ~150k ordinary commits (~500 B each) → one BFS level ≈ 75-150 MB
  in `out` + `mem` → over the 128 MB isolate on a *legitimate* large repo.
- Attacker-cheap path: one octopus-merge commit with ~200k parents (the commit
  object is ~8 MB — under the 16 MiB object cap) after a push of 200k tiny
  commits (~100 MB pack, under the 2 GiB cap). `git fetch` on the merge → one
  `load()` of ~200k commits → OOM → every fetch crashes → persistent DoS.
- `plan_reads` (generate.rs:363-380) → `Index::entries_of` (store/mod.rs:694-733)
  builds a Vec of every marked ObjLoc for a pack — up to ~1M × ~80 B ≈ 80-150 MB —
  plus `SendSet.reads`; `seen`/`next` at MAX_OBJECTS=1M add ~50-100 MB more.

The 64 MiB `MAX_MEM` limit (contract 9) exists but is structurally incapable of
firing before the memory is already allocated. Expected: charge `loc.len` into a
byte budget *before* reading, or stream `read_entries` per-span.

### 4. Delta-chain buffer unbounded — `pack/ingest.rs:283-318`

`resolve_at` pushes each delta's full compressed bytes into `chain` until the
base resolves:

```rust
Header::OfsDelta { .. } | Header::RefDelta { .. } => chain.push((off, raw)),
```

`MAX_DEPTH` = 64 (line 32). Each `raw` can be ~16.5 MiB (a delta whose inflated
output is 16 MiB, padded with literal insertions — legal per the pass-A
`decompressed_size ≤ MAX_OBJ` check at ingest.rs:72). Worst case ≈ 64 × 16.5 MiB
≈ 1 GiB held simultaneously → edge isolate OOM. Attacker cost: ~1 GiB of pack
body (under `MAX_PENDING` 2 GiB, ingest.rs:29).

Contract 2.4 budgeted "one base 32 MiB max, one delta result 32 MiB max" — the
`chain` vector of raw entries was never counted. Fix: bound `chain`'s total
bytes (e.g. `chain_bytes > 64 MiB → Unpack`), or decode-and-release the raw
bytes once the base is located.

### 5. Push-path memory far exceeds the isolate — `pack/ingest.rs:27-29`, `pack/run.rs:61-78`

`MAX_ENTRIES` = 2,000,000 (ingest.rs:27, checked at :66 *after* the Vec already
holds that many). `EntryRec` ≈ 40 B (`offset` u64 + `Header` enum ~24 B for the
`RefDelta{ObjectId}` variant + `compressed_len` + `header_len`), not the 24 B
contract 2.4 assumed → `recs` alone ≈ 80 MB. On top:

- `bases: Vec<ObjectId>` ≤ 2M × 20 B ≈ 40 MB (run.rs:61-69)
- `external: HashMap<ObjectId, ObjLoc>` — one entry per *live* ref-delta base
  (run.rs:70-78): ~120 B/entry → another ~60-240 MB when bases exist (attacker
  first lands the bases with a cheap push of small objects)
- pass B: `Cache` 48 MiB (ingest.rs:31), `Window` 8 MiB, part buffer 8 MiB,
  `sink.links` up to MAX_LINKS=1M × 20 B ≈ 20 MB + per-tree bursts between the
  every-10k-rows `post` checks (run.rs:138-140 checks `links.len()` only at post
  time).

A ~40 MB crafted pack of 2M minimal blob entries (≈20 B/entry compressed)
reaches the entry cap with >150 MB live → OOM. Fix: lower `MAX_ENTRIES` to fit
the real struct size (≈1M max even alone), count `external`/`links` bytes into a
single byte budget.

### 6. Zero-cost ref spam → unbounded advertisement → all reads die — `pack/run.rs:34-39`, `repo_do/mod.rs:299-322,502-508`

Creating a ref at an existing live oid needs **no pack**: `run()` returns
`(None, …)` on an empty body (run.rs:34-39). `apply_one` accepts
`old=null, new=<live oid>` after only a liveness check. The command section cap
is 1 MiB (`CMD_CAP`, edge/mod.rs:22) ≈ ~20k `<null> <oid> refs/heads/x<i>`
commands per POST → ~50 requests → 1M refs, zero R2 writes.

Consequences — every read path loads/serializes the whole table:
- `list_refs` (repo_do/mod.rs:299-322) → `to_array` of every row + `refs_json`
  builds ~150 MB JSON inside the DO;
- edge `info_refs` re-parses that JSON and builds the pkt advertisement in
  `w.out` (edge/mod.rs:186-202, wire/mod.rs:412-450) — tens of MB per request;
- `exec_refs_tags` (generate.rs:264-272) scans all tag refs per `include-tag`
  fetch; `ls_refs` similar.

Result: permanent read outage for the repo (likely OOMing the DO isolate),
for ~50 authenticated requests of cost. There is no per-repo ref count limit.
Fix: cap `refs` rows (and/or commands per push) — e.g. `Error::Limit` past
~100k refs — and consider `LIMIT`/prefix-bounded `list_refs`.

## MEDIUM

### 7. `pushes` grows forever; `push_begin` full-scans it — `repo_do/mod.rs:340`, `jobs/janitor.rs`, schema `store/mod.rs:776`

Per A5 the janitor sets `swept_at` and never deletes `pushes` rows.
`push_begin` runs `SELECT COUNT(*) FROM pushes WHERE state='open'` — `state`
is unindexed → O(table) scan on every push. A spammer creates one `committed`
row per POST (a header + flush suffices; no pack needed) → push cost grows
linearly forever. Same for `objects`-unindexed scans is bounded, but `pushes`
is not. Fix: index `(state)` or delete swept rows.

### 8. Forward REF_DELTA rejected — `pack/ingest.rs:372-378` (and `resolve_at` :302-307)

`by_id` is populated only for already-processed entries (`cx.by_id.insert(id, i)`
at :402 runs after resolution). A ref-delta whose base sits *later* in the pack
misses `by_id` → falls to `external()` → `missing base` unpack error. Git's
index-pack resolves forward ref-deltas; such a pack is valid and is refused
here. Real `git push` never emits forward deltas — interop edge → MEDIUM.
Fix: two-pass resolution, or treat a `by_id` miss as "resolve entry i by
scanning for its sha after hashing" — i.e., defer resolution until the base
entry's hash is known.

### 9. v0 `git-upload-pack` POST returns 200 — `edge/mod.rs:228-234`

Contract 1.1 rule 7: "A v0 POST git-upload-pack gets HTTP 400". `git_resp` emits
status 200. Git prints the ERR line either way — protocol deviation only. Align
status or amend the contract.

## LOW

### 10. Client bytes echoed into error text — `wire/mod.rs:143,252-254`; `edge/mod.rs:40`; `repo_do/mod.rs:604`; `error.rs:129-134`

`bad()`/`oid()` embed the raw argument/hex bytes (`as_bstr()`) into `Protocol`
messages; `unsupported content-encoding {e}` echoes a header value; `couldn't
find remote ref {qname}` echoes a `want-ref` name (valid-UTF-8 but control
chars legal). These flow into `ERR <msg>` pkt-lines and `unpack <msg>` —
pkt framing stays correct, but newlines/ANSI escapes are printed to the user's
terminal. Also `format!("{len:04x}")` (error.rs:131) produces >4 digits once
`msg.len()+4 ≥ 0x10000` (reachable via a ~64 KB echoed arg) → malformed first
packet on the error path. Fix: sanitize echoed bytes (e.g. reuse `echo_name`)
and clamp the ERR message length.

### 11. Base64 decoder leniency — `auth/mod.rs:73-105`, `:30-35`

`v(x).unwrap_or(0)` at :92 makes invalid characters in positions 2-4 of a quad
decode as zero bits instead of failing (only positions a and b require valid
input); embedded `=` are likewise absorbed. And a colonless decoded blob is
accepted as the token itself (`None => (s.to_string(), "basic")`, :34), so
`Authorization: Basic base64(<raw token>)` authenticates — no `user:` needed.
Neither grants a bypass (the decoded password must still equal the secret),
but the parser accepts malformed credentials a strict decoder rejects. Fix:
fail on `v(x) == None` in any position; require a `:`.

### 12. `.`/`..`/quota — `wire/http.rs:30-46`, `repo_do/mod.rs:250`

`seg_ok` allows `.` and `..` as owner/repo (`[A-Za-z0-9._-]{1,64}`) → DO names
`o/.`, `o/..`. Harmless for isolation (names aren't paths; R2 keys use
`repo_id` hex), but combined with no repo-creation quota, one token holder can
spawn thousands of junk DOs — each `boot` enqueues a Janitor (repo_do/mod.rs:250)
→ a 15-minute alarm cadence per junk DO forever. Consider rejecting `.`/`..`
and capping repos per token.

### 13. Uncharged storage calls — `store/mod.rs:328-334, 379-381, 500-502`

`Bucket::delete`/`delete_multiple` and both `abort()`s (`RawWriter`,
`PackWriter`) hit R2 without `budget.charge`. 7.1: "every Bucket and stub call
charges first". Small unbilled work on the failure path (one abort per failed
push; a spammer gets ~1 free R2 op per request). Charge them.

### 14. Janitor loses failed R2 deletes — `jobs/janitor.rs:100-101, 118-119`

`let _ = ...inner.delete(key).await` then unconditionally sets `swept_at`
(:101) or deletes the `packs` row (:119). A failed delete leaves the R2 object
orphaned forever (storage leak/billing). Retry on failure or leave a
re-checkable state.

### 15. `Level::Write` 500s when `GE_READ_TOKEN` unset — `auth/mod.rs:53`

`secret(env, "GE_READ_TOKEN")?` propagates `Internal` → HTTP 500 instead of
401 for a wrong token. Deployment-misconfig only.

### 16. Single-chunk cap overshoot — `edge/mod.rs:70-82`, `:250-256`, `:291-294`

`fill` appends a whole stream chunk before the caller's `> CMD_CAP` check runs,
so `body.buf`/`pr.buf`/`body` can exceed the 1 MiB caps by one runtime chunk.
Bounded by workerd's chunking — worth a per-chunk guard if the runtime ever
delivers large single chunks (e.g. via `DecompressionStream` output).

## Survived attack (verified clean)

- **Tenant isolation / DO identity.** `RepoRoute::parse` (wire/http.rs:34-46)
  is the only place DO names form; `seg_ok` rejects `/`, `%`, control bytes and
  non-ASCII; `splitn(3)` + empty-segment rejection blocks `a//b` smuggling;
  `.git` strip is suffix-only and idempotent-consistent. `id_from_name` gets
  `{owner}/{repo}` — a `/` inside a segment is impossible, so `a/b` vs `a/b/c`
  can't collide. All stub calls build *fresh* requests — `internal_request`
  (wire/http.rs:60-69) and the `Request::new_with_init` GETs (edge/mod.rs:176-182,
  211-216) carry only `x-ge-owner`/`x-ge-repo`; **no client header crosses**.
  `boot` (repo_do/mod.rs:261-265) re-verifies headers vs stored meta and R2 keys
  derive from the random `repo_id` — never from name or client input. Residual:
  the DO trusts those headers blindly; safe today because DOs aren't publicly
  reachable, but any future route forwarding a client `Request` to a stub would
  spoof identity *and* poison first-boot meta (:229-258). Defense in depth, not
  a live bug.
- **Auth correctness.** Write token checked before read on every route
  (auth/mod.rs:39-58); read-token-on-write → 403 with no re-challenge, per spec;
  `ct_eq` never short-circuits content (:64-69); auth runs before the DO wakes
  and before the body is read on all four routes (edge/mod.rs:111-115, 159, 209,
  227, 278); `/healthz` and unmatched `rest` are auth-free but touch nothing.
  Duplicate/comma-joined Authorization headers fail safe (decode rejects `,`).
- **Injection.** All SQL is parameterized (`?` / `json_each`) — ref names,
  principal, push ids all bound. `agent=` is stored, never echoed. Report-status
  names pass `echo_name` (wire/mod.rs:459-461 → non-graphic → `?`); `ng` reasons
  are `&'static` via `leak_reason` (edge/mod.rs:372-380). Ref names hitting SQL
  or advertisements are `gix_validate`-clean (repo_do/mod.rs:491-494) and must
  start `refs/`. `wanted-refs` echoes only names that exist in `refs`
  (repo_do/mod.rs:599-606 → validated names). `client_message` strips
  Internal/Storage detail everywhere (error.rs:98-103).
- **Subrequest budget.** Charged before use at every stub call (wire/http.rs:110,
  128), range read (store/mod.rs:267), MPU op (store/mod.rs:349-376, 421-486).
  `plan_reads` projects total reads against the budget *before* streaming
  (generate.rs:374-377) — no mid-stream budget abort possible by design
  (but see finding 1 for what happens when an error does occur).
- **DO span discipline.** `commit_push` is fully sync (repo_do/mod.rs:421-478);
  Storage/Internal propagate out of `fetch` for rollback (:116-118); `changes()`
  is the only CAS oracle.
- **Secrets.** `.dev.vars` is gitignored (server/.gitignore:3) and absent from
  the repo; secret names, not values, appear in errors; nothing logs
  credentials.
